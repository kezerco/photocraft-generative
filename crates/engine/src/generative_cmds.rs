//! First native Generative Fill backend: masked FLUX.1 Pro Fill via fal.ai.
//! Results never replace the render; a selected-only pixel layer is added in one undo step.

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
    let megapixels = (u64::from(area.width()) * u64::from(area.height())).div_ceil(1_000_000);
    Ok((area.width(), area.height(), megapixels as f64 * 0.05))
}

fn encode_input(doc: &photocraft_doc::Document, selection: &Surface, area: Rect) -> Result<(Vec<u8>, Vec<u8>)> {
    let fmt = PixelFormat::new(ColorMode::Rgb, doc.pixel_format().sample, true);
    let surface = photocraft_compose::flatten_to_surface(doc, fmt, Some([1.0; 3]));
    let pixels = surface.read_region(area);
    let mut rgb = Vec::with_capacity((area.width() as usize) * (area.height() as usize) * 3);
    let mut mask = Vec::with_capacity((area.width() as usize) * (area.height() as usize));
    for (i, px) in pixels.as_chunks::<4>().0.iter().enumerate() {
        let x = area.x0 + (i % area.width() as usize) as i32;
        let y = area.y0 + (i / area.width() as usize) as i32;
        rgb.extend(px[..3].iter().map(|c| (c.clamp(0.0, 1.0) * 255.0).round() as u8));
        mask.push((selection.sample_channel(x, y, 0).clamp(0.0, 1.0) * 255.0).round() as u8);
    }
    let w = area.width();
    let h = area.height();
    let image = Image::from_u8(w, h, ChannelLayout::Rgb, rgb).map_err(|_| bad("could not prepare input image"))?;
    let mask = Image::from_u8(w, h, ChannelLayout::Gray, mask).map_err(|_| bad("could not prepare selection mask"))?;
    let opts = EncodeOptions::default();
    Ok((
        photocraft_codecs::encode(&image, Format::Png, &opts).map_err(|_| bad("could not encode input image"))?,
        photocraft_codecs::encode(&mask, Format::Png, &opts).map_err(|_| bad("could not encode selection mask"))?,
    ))
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
    let apply_selection = selection.clone();
    crate::jobs::run(
        s,
        "Generative Fill",
        true,
        move |ctx| {
            ctx.check()?;
            ctx.progress(0.1, "Preparing selection");
            let (image, mask) = encode_input(&doc, &selection, area)?;
            ctx.check()?;
            ctx.progress(0.3, "Waiting for fal.ai");
            let bytes = crate::fal_client::fill(model, &image, &mask, &prompt).map_err(EngineError::Other)?;
            ctx.check()?;
            let limits = photocraft_codecs::Limits { max_width: 4096, max_height: 4096, max_pixels: 8_000_000, max_alloc: 128 * 1024 * 1024 };
            let image = photocraft_codecs::decode_with(&bytes, &photocraft_codecs::DecodeOptions { limits, ..Default::default() })
                .map_err(|_| bad("fal.ai returned an invalid or oversized image"))?;
            if !matches!(image.layout(), ChannelLayout::Rgb | ChannelLayout::Rgba) {
                return Err(bad("fal.ai returned an unsupported colour format"));
            }
            let pixels = image.to_normalized();
            let channels = image.layout().channels();
            let pixels = if image.width() != area.width() || image.height() != area.height() {
                if model == crate::fal_client::Model::FluxFill {
                    return Err(bad("masked fill returned the wrong size; document unchanged (provider may still charge)"));
                }
                let expected = area.width() as f64 / area.height() as f64;
                let actual = image.width() as f64 / image.height() as f64;
                if (actual / expected - 1.0).abs() > 0.05 {
                    return Err(bad("model changed the crop aspect ratio; document unchanged (provider may still charge)"));
                }
                photocraft_algo::seam::resize_bilinear(
                    image.width() as usize,
                    image.height() as usize,
                    channels,
                    &pixels,
                    area.width() as usize,
                    area.height() as usize,
                )
            } else {
                pixels
            };
            ctx.progress(0.9, "Preparing new layer");
            Ok(pixels)
        },
        move |s, pixels: Vec<f32>| apply_result(s, &apply_selection, area, fmt, active, &pixels),
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
        params: r##"{"model":"flux1Fill|flux2Pro|geminiPro","prompt":text,"confirmCharge":bool=false}"##,
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
            let mut s = Session::new();
            s.execute("file.new", json!({"width": 32, "height": 32, "depth": depth, "background": "white"})).unwrap();
            s.execute("select.rect", json!({"x": 10, "y": 10, "width": 4, "height": 4})).unwrap();
            let doc = &s.active().unwrap().doc;
            let selection = doc.selection.as_ref().unwrap();
            let area = crop(selection, doc.bounds()).unwrap();
            let (image, mask) = encode_input(doc, selection, area).unwrap();
            let image = photocraft_codecs::decode(&image).unwrap();
            let mask = photocraft_codecs::decode(&mask).unwrap();
            assert_eq!((image.width(), image.height()), (mask.width(), mask.height()));
            let at = |x: u32, y: u32| (y * mask.width() + x) as usize;
            assert_eq!(mask.sample_normalized(at(10, 10)), 1.0);
            assert_eq!(mask.sample_normalized(at(0, 0)), 0.0);
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
    fn selection_crop_is_bounded_and_has_context() {
        let canvas = Rect::new(0, 0, 400, 400);
        let mut mask = Surface::new(photocraft_color::PixelFormat::GRAY8);
        mask.write_region(Rect::new(100, 100, 110, 110), &vec![1.0; 100]);
        let area = crop(&mask, canvas).unwrap();
        assert!(area.x0 < 100 && area.y0 < 100 && area.x1 > 110 && area.y1 > 110);
        assert!(crop(&mask, Rect::new(0, 0, 10, 10)).is_err());
    }
}
