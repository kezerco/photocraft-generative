//! Native fal.ai generative fill and guided image editing.
//! Results never replace the render; a soft-edged selection layer is added in one undo step.

use photocraft_codecs::{ChannelLayout, EncodeOptions, Format, Image};
use photocraft_color::{ColorMode, PixelFormat, SampleType};
use photocraft_doc::Layer;
use photocraft_geom::Rect;
use photocraft_raster::Surface;
use serde_json::{Value, json};

use crate::commands::CommandSpec;
use crate::{EngineError, Result, Session};

const MAX_SIDE: u32 = 2048;
const MAX_PIXELS: i64 = 4_000_000;

fn enabled(s: &Session) -> std::result::Result<(), String> {
    let d = s.active().ok_or("open a document first")?;
    if d.doc.pixel_format().mode != ColorMode::Rgb {
        return Err("Generative Fill currently supports RGB documents only".into());
    }
    if d.doc.pixel_format().sample == SampleType::F32 {
        return Err("32-bit HDR/linear documents are not yet colour-managed for cloud editing; export an sRGB PNG copy first".into());
    }
    if d.doc.selection.as_ref().is_none_or(|m| m.content_bounds().intersect(&d.doc.bounds()).is_empty()) {
        return Err("select the area to fill first".into());
    }
    Ok(())
}

fn bad(msg: &str) -> EngineError {
    EngineError::Other(msg.into())
}

/// Include neighbouring pixels so the model can match render lighting and perspective.
fn crop(selection: &Surface, canvas: Rect) -> Result<Rect> {
    let bounds = selection.content_bounds().intersect(&canvas);
    if bounds.is_empty() {
        return Err(bad("select the area to fill first"));
    }
    let side = bounds.width().max(bounds.height());
    let area = bounds.inflate((side / 3).clamp(32, 256) as i32).intersect(&canvas);
    if area.width() > MAX_SIDE || area.height() > MAX_SIDE || i64::from(area.width()) * i64::from(area.height()) > MAX_PIXELS {
        return Err(bad("selection is too large for this first version (crop limit: 2048 px / 4 MP); select a smaller area"));
    }
    Ok(area)
}

/// Advisory only: the provider's live price is authoritative and can change without notice.
pub fn estimated_cost(s: &Session) -> Result<(u32, u32, f64)> {
    enabled(s).map_err(EngineError::Other)?;
    let doc = &s.active().ok_or(EngineError::NoDocument)?.doc;
    let selection = doc.selection.as_ref().ok_or_else(|| bad("select the area to fill first"))?;
    let area = crop(selection, doc.bounds())?;
    // FLUX.1 Fill receives a square padded input to preserve aspect when the provider
    // chooses its own (often different) output resolution.
    let side = area.width().max(area.height());
    let megapixels = (u64::from(side) * u64::from(side)).div_ceil(1_000_000);
    Ok((area.width(), area.height(), megapixels as f64 * 0.05))
}

/// The provider can extend the subject a little beyond the original selection. The input
/// receives a solid expanded mask; the returned layer has a soft edge for blending. At zero
/// pixels, keep the original selection exactly (including its antialiasing).
fn expanded_masks(selection: &Surface, area: Rect, edge: u32) -> (Surface, Surface) {
    if edge == 0 {
        return (selection.clone(), selection.clone());
    }
    let (w, h) = (area.width() as usize, area.height() as usize);
    let original = selection.read_region(area);
    let expanded = photocraft_algo::selection::expand(&original, w, h, edge as f32);
    let blended = photocraft_algo::selection::feather(&expanded, w, h, 2.0);
    let mut input = Surface::new(PixelFormat::GRAY8);
    input.write_region(area, &expanded);
    let mut output = Surface::new(PixelFormat::GRAY8);
    output.write_region(area, &blended);
    (input, output)
}

fn encode_input(doc: &photocraft_doc::Document, selection: &Surface, area: Rect, model: crate::fal_client::Model) -> Result<(Vec<u8>, Vec<u8>)> {
    let fmt = PixelFormat::new(ColorMode::Rgb, doc.pixel_format().sample, true);
    let surface = photocraft_compose::flatten_to_surface(doc, fmt, Some([1.0; 3]));
    let pixels = surface.read_region(area);
    let (w, h) = (area.width() as usize, area.height() as usize);
    if w == 0 || h == 0 {
        return Err(bad("cannot send an empty render crop"));
    }
    let side = if model == crate::fal_client::Model::FluxFill { w.max(h) } else { w };
    let (out_w, out_h) = (side, if model == crate::fal_client::Model::FluxFill { side } else { h });
    let (pad_x, pad_y) = ((out_w - w) / 2, (out_h - h) / 2);
    let mut rgb = Vec::with_capacity(out_w * out_h * 3);
    let mut mask = Vec::with_capacity(out_w * out_h);
    for oy in 0..out_h {
        for ox in 0..out_w {
            // Repeat the crop's edge pixels in the padding so the model sees natural context,
            // but never allow it to edit the padding itself.
            let (x, y) = (ox.saturating_sub(pad_x).min(w - 1), oy.saturating_sub(pad_y).min(h - 1));
            let i = (y * w + x) * 4;
            let px = pixels.get(i..i + 3).ok_or_else(|| bad("could not read the render crop"))?;
            rgb.extend(px.iter().map(|c| (c.clamp(0.0, 1.0) * 255.0).round() as u8));
            let inside = ox >= pad_x && ox < pad_x + w && oy >= pad_y && oy < pad_y + h;
            let coverage = if inside { selection.sample_channel(area.x0 + x as i32, area.y0 + y as i32, 0) } else { 0.0 };
            mask.push((coverage.clamp(0.0, 1.0) * 255.0).round() as u8);
        }
    }
    let image = Image::from_u8(out_w as u32, out_h as u32, ChannelLayout::Rgb, rgb).map_err(|_| bad("could not prepare input image"))?;
    let mask = Image::from_u8(out_w as u32, out_h as u32, ChannelLayout::Gray, mask).map_err(|_| bad("could not prepare selection mask"))?;
    let opts = EncodeOptions::default();
    Ok((
        photocraft_codecs::encode(&image, Format::Png, &opts).map_err(|_| bad("could not encode input image"))?,
        photocraft_codecs::encode(&mask, Format::Png, &opts).map_err(|_| bad("could not encode selection mask"))?,
    ))
}

/// Map a provider-selected output resolution back to the *same document coordinates*.
/// Masked fill sends square padded input, so resize to that square before removing padding;
/// never stretch a rectangular returned image into the selection (that would distort faces).
fn align_output(image: &Image, area: Rect, model: crate::fal_client::Model) -> Result<Vec<f32>> {
    if !matches!(image.layout(), ChannelLayout::Rgb | ChannelLayout::Rgba) {
        return Err(bad("fal.ai returned an unsupported colour format"));
    }
    let (w, h, channels) = (area.width() as usize, area.height() as usize, image.layout().channels());
    if w == 0 || h == 0 {
        return Err(bad("cannot place generated pixels into an empty crop"));
    }
    let square = model == crate::fal_client::Model::FluxFill;
    let (target_w, target_h) = if square { (w.max(h), w.max(h)) } else { (w, h) };
    let (iw, ih) = (image.width() as usize, image.height() as usize);
    let expected = target_w as f64 / target_h as f64;
    let actual = iw as f64 / ih as f64;
    if (actual / expected - 1.0).abs() > 0.05 {
        // Some fill backends may discard the padding and return the original crop aspect.
        // That is unambiguous only when the returned ratio agrees with the document crop.
        let crop_ratio = w as f64 / h as f64;
        if square && (actual / crop_ratio - 1.0).abs() <= 0.05 {
            return Ok(photocraft_algo::seam::resize_bilinear(iw, ih, channels, &image.to_normalized(), w, h));
        }
        let shape = if square { "square or the crop's aspect ratio" } else { "the crop's aspect ratio" };
        return Err(bad(&format!(
            "fal.ai returned {iw}×{ih}, but expected {shape} input {target_w}×{target_h} for document crop {w}×{h}; document unchanged (provider may still charge)"
        )));
    }
    let data = image.to_normalized();
    let scaled = if iw == target_w && ih == target_h { data } else { photocraft_algo::seam::resize_bilinear(iw, ih, channels, &data, target_w, target_h) };
    if !square {
        return Ok(scaled);
    }
    let (pad_x, pad_y) = ((target_w - w) / 2, (target_h - h) / 2);
    let mut cropped = Vec::with_capacity(w * h * channels);
    for y in 0..h {
        let start = ((y + pad_y) * target_w + pad_x) * channels;
        cropped.extend_from_slice(scaled.get(start..start + w * channels).ok_or_else(|| bad("fal.ai result could not be aligned with the render"))?);
    }
    Ok(cropped)
}

fn run(s: &mut Session, p: &Value) -> Result<Value> {
    enabled(s).map_err(EngineError::Other)?;
    let prompt = p
        .get("prompt")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty() && s.len() <= 2000)
        .ok_or_else(|| bad("prompt must be 1–2000 characters"))?
        .to_string();
    let model_name = match p.get("model") {
        None => "flux1Fill",
        Some(Value::String(s)) => s.as_str(),
        Some(_) => return Err(bad("model must be a string")),
    };
    let model = crate::fal_client::Model::parse(model_name).ok_or_else(|| bad("model must be flux1Fill, flux2Pro or geminiPro"))?;
    let edge = match p.get("edgePixels") {
        None => 12,
        Some(v) => v.as_f64().filter(|n| n.is_finite() && (0.0..=24.0).contains(n)).ok_or_else(|| bad("edgePixels must be 0–24"))?.round() as u32,
    };
    if model != crate::fal_client::Model::FluxFill && p.get("acceptGuidedEdit").and_then(Value::as_bool) != Some(true) {
        return Err(bad("FLUX.2 Pro and Gemini do not support a true input mask; confirm the selection is a guide with acceptGuidedEdit: true"));
    }
    if p.get("confirmCharge").and_then(Value::as_bool) != Some(true) {
        return Err(bad("confirm the paid fal.ai request with confirmCharge: true"));
    }
    if std::env::var("FAL_KEY").is_err() {
        return Err(bad("set FAL_KEY before using Generative Fill"));
    }
    let d = s.active().ok_or(EngineError::NoDocument)?;
    let doc = d.doc.clone();
    let selection = doc.selection.clone().ok_or_else(|| bad("select the area to fill first"))?;
    let area = crop(&selection, doc.bounds())?;
    let fmt = PixelFormat { alpha: true, ..doc.pixel_format() };
    let active = d.active_layer;
    crate::jobs::run(
        s,
        "Generative Fill",
        true,
        move |ctx| {
            ctx.check()?;
            ctx.progress(0.1, "Preparing selection");
            let (model_mask, overlay_mask) = expanded_masks(&selection, area, edge);
            let (image, mask) = encode_input(&doc, &model_mask, area, model)?;
            ctx.check()?;
            ctx.progress(0.3, "Waiting for fal.ai");
            let bytes = crate::fal_client::fill(model, &image, &mask, &prompt).map_err(EngineError::Other)?;
            ctx.check()?;
            let limits = photocraft_codecs::Limits { max_width: 4096, max_height: 4096, max_pixels: 8_000_000, max_alloc: 128 * 1024 * 1024 };
            let image = photocraft_codecs::decode_with(&bytes, &photocraft_codecs::DecodeOptions { limits, ..Default::default() })
                .map_err(|_| bad("fal.ai returned an invalid or oversized image"))?;
            let pixels = align_output(&image, area, model)?;
            ctx.progress(0.9, "Preparing new layer");
            Ok((pixels, overlay_mask))
        },
        move |s, (pixels, overlay_mask): (Vec<f32>, Surface)| apply_result(s, &overlay_mask, area, fmt, active, &pixels),
    )
}

fn apply_result(s: &mut Session, selection: &Surface, area: Rect, fmt: PixelFormat, active: Option<photocraft_doc::LayerId>, pixels: &[f32]) -> Result<Value> {
    let count = area.width() as usize * area.height() as usize;
    if count == 0 || !pixels.len().is_multiple_of(count) || !matches!(pixels.len() / count, 3 | 4) {
        return Err(bad("unsupported generated image"));
    }
    let channels = pixels.len() / count;
    let mut out = vec![0.0f32; count * 4];
    for (i, src) in pixels.chunks_exact(channels).enumerate() {
        let x = area.x0 + (i % area.width() as usize) as i32;
        let y = area.y0 + (i / area.width() as usize) as i32;
        let coverage = selection.sample_channel(x, y, 0);
        let dst = &mut out[4 * i..4 * i + 4];
        dst[..3].copy_from_slice(&src[..3]);
        dst[3] = coverage * if channels == 4 { src[3] } else { 1.0 };
    }
    let layer_id = s.edit("Generative Fill", |doc, current| {
        let mut layer = Layer::raster(doc.next_layer_name("Generative Fill"), fmt);
        let surface = layer.surface_mut().ok_or_else(|| bad("could not create image layer"))?;
        surface.write_region(area, &out);
        surface.prune();
        let id = doc.insert_above(active, layer);
        *current = Some(id);
        Ok(id)
    })?;
    Ok(json!({"layer": layer_id, "bounds": [area.x0, area.y0, area.width(), area.height()]}))
}

pub fn specs() -> Vec<CommandSpec> {
    vec![CommandSpec {
        id: "edit.generativeFill",
        label: "Generative Fill…",
        menu: &["Edit"],
        shortcut: None,
        params: r##"{"model":"flux1Fill|flux2Pro|geminiPro","prompt":text,"edgePixels":0..24=12,"acceptGuidedEdit":bool=false,"confirmCharge":bool=false}"##,
        enabled,
        run,
        journal: true,
    }]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paid_requests_require_explicit_consent_and_do_not_mutate_document() {
        let mut s = Session::new();
        s.execute("file.new", json!({"width": 32, "height": 32})).unwrap();
        s.execute("select.rect", json!({"x": 10, "y": 10, "width": 4, "height": 4})).unwrap();
        let before = s.active().unwrap().history.past_len();
        for params in [
            json!({}),
            json!({"prompt": "fix face"}),
            json!({"prompt": "   ", "confirmCharge": true}),
            json!({"model": "unlisted", "prompt": "fix", "confirmCharge": true}),
            json!({"model": 0, "prompt": "fix", "confirmCharge": true}),
            json!({"prompt": "fix", "edgePixels": 100, "confirmCharge": true}),
            json!({"model": "geminiPro", "prompt": "fix", "confirmCharge": true}),
            json!({"model": "flux2Pro", "prompt": "fix", "confirmCharge": true}),
        ] {
            assert!(s.execute("edit.generativeFill", params).is_err());
            assert_eq!(s.active().unwrap().history.past_len(), before);
        }
    }

    #[test]
    fn linear_hdr_is_refused_until_colour_managed() {
        let mut s = Session::new();
        s.execute("file.new", json!({"width": 32, "height": 32, "depth": 32})).unwrap();
        s.execute("select.rect", json!({"x": 10, "y": 10, "width": 4, "height": 4})).unwrap();
        assert!(!s.is_enabled("edit.generativeFill"));
        assert!(s.execute("edit.generativeFill", json!({"prompt": "fix", "confirmCharge": true})).is_err());
    }

    #[test]
    fn request_mask_and_image_have_matching_crop_dimensions() {
        for depth in [8, 16] {
            for (width, height) in [(32, 32), (32, 16), (16, 32)] {
                let mut s = Session::new();
                s.execute("file.new", json!({"width": width, "height": height, "depth": depth, "background": "white"})).unwrap();
                s.execute("select.rect", json!({"x": 10, "y": 10, "width": 4, "height": 4})).unwrap();
                let doc = &s.active().unwrap().doc;
                let selection = doc.selection.as_ref().unwrap();
                let area = crop(selection, doc.bounds()).unwrap();
                let (image, mask) = encode_input(doc, selection, area, crate::fal_client::Model::FluxFill).unwrap();
                let image = photocraft_codecs::decode(&image).unwrap();
                let mask = photocraft_codecs::decode(&mask).unwrap();
                let side = width.max(height) as u32;
                assert_eq!((image.width(), image.height()), (side, side));
                assert_eq!((mask.width(), mask.height()), (side, side));
                let at = |x: u32, y: u32| (y * mask.width() + x) as usize;
                let (pad_x, pad_y) = ((side - width as u32) / 2, (side - height as u32) / 2);
                assert_eq!(mask.sample_normalized(at(10 + pad_x, 10 + pad_y)), 1.0);
                assert_eq!(mask.sample_normalized(at(0, 0)), 0.0);
                let (guided, _) = encode_input(doc, selection, area, crate::fal_client::Model::GeminiPro).unwrap();
                let guided = photocraft_codecs::decode(&guided).unwrap();
                assert_eq!((guided.width(), guided.height()), (width as u32, height as u32));
            }
        }
    }

    #[test]
    fn masked_output_resizes_and_unpads_without_distorting_the_crop() {
        let area = Rect::new(0, 0, 6, 4);
        let side = 6;
        // Provider returns double resolution; mark each input-space row with a different red
        // value. Cropping away the padded top/bottom must keep rows 1–4, in the same order.
        let data: Vec<u8> = (0..12).flat_map(|y| (0..12).flat_map(move |_| [((y / 2) * 30) as u8, 0, 0])).collect();
        let image = Image::from_u8(12, 12, ChannelLayout::Rgb, data).unwrap();
        let result = align_output(&image, area, crate::fal_client::Model::FluxFill).unwrap();
        assert_eq!(result.len(), (area.width() * area.height() * 3) as usize);
        assert!((result[0] - 30.0 / 255.0).abs() < 0.01);
        assert!((result[(3 * 6 * 3) as usize] - 120.0 / 255.0).abs() < 0.01);
        let rectangular = Image::from_u8(12, 8, ChannelLayout::Rgb, vec![0; 12 * 8 * 3]).unwrap();
        assert_eq!(align_output(&rectangular, area, crate::fal_client::Model::FluxFill).unwrap().len(), result.len());
        let invalid = Image::from_u8(12, 7, ChannelLayout::Rgb, vec![0; 12 * 7 * 3]).unwrap();
        let err = align_output(&invalid, area, crate::fal_client::Model::FluxFill).unwrap_err().to_string();
        assert!(err.contains("12×7") && err.contains(&format!("{side}×{side}")), "{err}");
    }

    #[test]
    fn guided_models_accept_resized_output_but_reject_unrelated_aspects() {
        let area = Rect::new(0, 0, 6, 4);
        let good = Image::from_u8(12, 8, ChannelLayout::Rgb, vec![80; 12 * 8 * 3]).unwrap();
        let bad = Image::from_u8(12, 12, ChannelLayout::Rgb, vec![0; 12 * 12 * 3]).unwrap();
        for model in [crate::fal_client::Model::Flux2Pro, crate::fal_client::Model::GeminiPro] {
            let result = align_output(&good, area, model).unwrap();
            assert_eq!(result.len(), 6 * 4 * 3);
            assert!((result[0] - 80.0 / 255.0).abs() < 0.01);
            let error = align_output(&bad, area, model).unwrap_err().to_string();
            assert!(error.contains("12×12") && error.contains("6×4"), "{model:?}: {error}");
        }
    }

    #[test]
    fn generated_layer_respects_selection_and_undo() {
        for depth in [8, 16] {
            let mut s = Session::new();
            s.execute("file.new", json!({"width": 16, "height": 16, "depth": depth, "background": "white"})).unwrap();
            s.execute("select.rect", json!({"x": 6, "y": 6, "width": 2, "height": 2})).unwrap();
            let doc = s.active().unwrap();
            let original = doc.active_layer;
            let selection = doc.doc.selection.clone().unwrap();
            let fmt = PixelFormat { alpha: true, ..doc.doc.pixel_format() };
            let area = Rect::new(4, 4, 10, 10);
            let before = photocraft_compose::flatten_to_surface(&doc.doc, fmt, None);
            let pixels = vec![0.0; 6 * 6 * 3];
            assert!(apply_result(&mut s, &selection, area, fmt, original, &pixels[..2]).is_err());
            let result = apply_result(&mut s, &selection, area, fmt, original, &pixels).unwrap();
            let id = s.active().unwrap().active_layer.unwrap();
            assert_eq!(result["layer"], json!(id));
            let layer = s.active().unwrap().doc.layer(id).unwrap();
            assert_eq!(layer.surface().unwrap().sample_channel(5, 5, 3), 0.0, "depth {depth}");
            assert_eq!(layer.surface().unwrap().sample_channel(6, 6, 3), 1.0, "depth {depth}");
            assert_eq!(layer.surface().unwrap().format().sample, fmt.sample);
            let after = photocraft_compose::flatten_to_surface(&s.active().unwrap().doc, fmt, None);
            assert_eq!(after.pixel(5, 5), before.pixel(5, 5), "outside selection changed at depth {depth}");
            assert_eq!(after.pixel(9, 9), before.pixel(9, 9), "outside selection changed at depth {depth}");
            assert!(after.sample_channel(6, 6, 0) < 0.1, "selected pixel was not replaced at depth {depth}");
            assert!(s.active().unwrap().doc.layer(original.unwrap()).is_some());
            s.undo();
            assert_eq!(s.active().unwrap().active_layer, original);
        }
    }

    #[test]
    fn expanded_selection_gives_subject_room_without_changing_distant_pixels() {
        let area = Rect::new(0, 0, 80, 80);
        let mut original = Surface::new(PixelFormat::GRAY8);
        original.write_region(Rect::new(39, 39, 41, 41), &[1.0; 4]);
        let (input, output) = expanded_masks(&original, area, 12);
        assert_eq!(input.sample_channel(39, 39, 0), 1.0);
        assert!(input.sample_channel(50, 40, 0) > 0.5);
        assert!(output.sample_channel(50, 40, 0) > 0.0);
        assert_eq!(output.sample_channel(0, 0, 0), 0.0);
        let (input, output) = expanded_masks(&original, area, 0);
        assert_eq!(input.sample_channel(50, 40, 0), 0.0);
        assert_eq!(output.sample_channel(39, 39, 0), 1.0);
    }

    #[test]
    fn soft_mask_coverage_is_preserved_on_generated_layer() {
        let mut s = Session::new();
        s.execute("file.new", json!({"width": 8, "height": 8, "depth": 16})).unwrap();
        let fmt = PixelFormat { alpha: true, ..s.active().unwrap().doc.pixel_format() };
        let active = s.active().unwrap().active_layer;
        let mut selection = Surface::new(PixelFormat::GRAY8);
        selection.write_region(Rect::new(2, 2, 4, 3), &[0.5, 1.0]);
        let area = Rect::new(1, 1, 5, 4);
        apply_result(&mut s, &selection, area, fmt, active, &vec![0.0; area.width() as usize * area.height() as usize * 3]).unwrap();
        let st = s.active().unwrap();
        let layer = st.doc.layer(st.active_layer.unwrap()).unwrap();
        let surface = layer.surface().unwrap();
        assert!((surface.sample_channel(2, 2, 3) - 0.5).abs() < 0.01);
        assert_eq!(surface.sample_channel(3, 2, 3), 1.0);
        assert_eq!(surface.sample_channel(1, 2, 3), 0.0);
    }

    #[test]
    fn selection_crop_is_bounded_and_has_context() {
        let canvas = Rect::new(0, 0, 400, 400);
        let mut mask = Surface::new(photocraft_color::PixelFormat::GRAY8);
        mask.write_region(Rect::new(100, 100, 110, 110), &vec![1.0; 100]);
        let area = crop(&mask, canvas).unwrap();
        assert!(area.x0 < 100 && area.y0 < 100 && area.x1 > 110 && area.y1 > 110);
        assert!(crop(&mask, Rect::new(0, 0, 10, 10)).is_err());
    }
}
