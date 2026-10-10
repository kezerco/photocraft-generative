//! Native fal.ai image-editing transport. No API keys, prompts or image bytes are logged.
//! Only a fixed model endpoint and HTTPS image hosts are accepted; do not let model output
//! choose arbitrary URLs on the user's computer.

use std::io::Read;
use std::time::Duration;

use base64::Engine as _;
use serde_json::{Value, json};

const MAX_RESPONSE: u64 = 20 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Model {
    FluxFill,
    Flux2Pro,
    GeminiPro,
}

impl Model {
    pub(crate) fn parse(value: &str) -> Option<Self> {
        match value {
            "flux1Fill" => Some(Self::FluxFill),
            "flux2Pro" => Some(Self::Flux2Pro),
            "geminiPro" => Some(Self::GeminiPro),
            _ => None,
        }
    }

    fn endpoint(self) -> &'static str {
        match self {
            Self::FluxFill => "https://fal.run/fal-ai/flux-pro/v1/fill",
            Self::Flux2Pro => "https://fal.run/fal-ai/flux-2-pro/edit",
            Self::GeminiPro => "https://fal.run/fal-ai/nano-banana-pro/edit",
        }
    }
}

fn request(model: Model, image: &[u8], mask: &[u8], prompt: &str) -> Value {
    let image = image_uri(image);
    let mask = image_uri(mask);
    // Clarify the frequent short prompt without changing custom prompts.
    let instruction = if prompt.eq_ignore_ascii_case("make it realistic") {
        "Make the selected subject photorealistic. Repair visible rendering, texture, face or geometry errors while preserving the subject's recognisable identity, placement, camera perspective and scene lighting."
    } else {
        prompt
    };
    match model {
        Model::FluxFill => json!({"image_url": image, "mask_url": mask, "prompt": instruction, "output_format": "png"}),
        Model::Flux2Pro | Model::GeminiPro => {
            let instruction = format!(
                "Edit image 1. Image 2 is a black and white selection guide: white is the subject to edit, black is context to preserve. Change the selected subject according to: {instruction} Preserve the recognisable subject, original camera, perspective, materials, lighting, and surrounding scene. Return the full edited image with the same composition and aspect ratio."
            );
            json!({"image_urls": [image, mask], "prompt": instruction, "output_format": "png"})
        }
    }
}

fn image_uri(bytes: &[u8]) -> String {
    format!("data:image/png;base64,{}", base64::engine::general_purpose::STANDARD.encode(bytes))
}

fn response_url(body: &Value) -> Result<&str, String> {
    let url = body
        .get("images")
        .and_then(Value::as_array)
        .and_then(|images| images.first())
        .and_then(|image| image.get("url"))
        .and_then(Value::as_str)
        .ok_or("fal.ai returned no image")?;
    // Never fetch an arbitrary HTTP, local-network, file or data URL returned by a service.
    let rest = url.strip_prefix("https://").ok_or("fal.ai returned a non-HTTPS image URL")?;
    let host = rest.split(['/', '?', '#']).next().unwrap_or_default();
    if !matches!(host, "fal.media" | "storage.googleapis.com" | "v3.fal.media") && !host.ends_with(".fal.media") {
        return Err("fal.ai returned an unexpected image host".into());
    }
    Ok(url)
}

pub(crate) fn fill(model: Model, image_png: &[u8], mask_png: &[u8], prompt: &str) -> Result<Vec<u8>, String> {
    let key = crate::fal_key::read_key()?;
    let request = request(model, image_png, mask_png, prompt);
    let agent = ureq::AgentBuilder::new().timeout(Duration::from_secs(180)).redirects(0).build();
    let response = agent.post(model.endpoint()).set("Authorization", &format!("Key {key}")).send_json(request).map_err(|e| match e {
        ureq::Error::Status(code, _) => format!("fal.ai request failed (HTTP {code}); check your API key, balance and model availability"),
        ureq::Error::Transport(_) => "fal.ai connection failed or timed out".to_string(),
    })?;
    let mut data = Vec::new();
    response.into_reader().take(MAX_RESPONSE + 1).read_to_end(&mut data).map_err(|_| "could not read fal.ai response")?;
    if data.len() as u64 > MAX_RESPONSE {
        return Err("fal.ai response is too large".into());
    }
    let body: Value = serde_json::from_slice(&data).map_err(|_| "fal.ai returned invalid JSON")?;
    let url = response_url(&body)?;
    let image = agent.get(url).call().map_err(|_| "could not download generated image")?;
    let mut bytes = Vec::new();
    image.into_reader().take(MAX_RESPONSE + 1).read_to_end(&mut bytes).map_err(|_| "could not read generated image")?;
    if bytes.len() as u64 > MAX_RESPONSE {
        return Err("generated image is too large".into());
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_expected_https_image_hosts() {
        assert!(response_url(&json!({"images":[{"url":"https://fal.media/files/a.png"}]})).is_ok());
        assert!(response_url(&json!({"images":[{"url":"https://storage.googleapis.com/falserverless/a.png"}]})).is_ok());
        for url in ["http://fal.media/a", "https://fal.media.evil.com/a", "https://127.0.0.1/a", "file:///tmp/x", "https://fal.media:443/a"] {
            assert!(response_url(&json!({"images":[{"url":url}]})).is_err(), "{url}");
        }
        assert!(response_url(&json!({"images":[]})).is_err());
    }

    #[test]
    fn model_requests_use_only_fixed_endpoints_and_a_guide_for_maskless_edits() {
        for model in [Model::FluxFill, Model::Flux2Pro, Model::GeminiPro] {
            assert!(model.endpoint().starts_with("https://fal.run/fal-ai/"));
            let body = request(model, b"image", b"mask", "fix the face");
            if model == Model::FluxFill {
                assert!(body["mask_url"].as_str().unwrap().starts_with("data:image/png;base64,"));
            } else {
                assert_eq!(body["image_urls"].as_array().unwrap().len(), 2);
                assert!(body["prompt"].as_str().unwrap().contains("Image 2 is a black and white selection guide"));
            }
        }
        assert!(Model::parse("unknown").is_none());
    }

    #[test]
    fn short_realism_prompt_keeps_the_selected_subject_recognisable() {
        let body = request(Model::FluxFill, b"image", b"mask", "Make it realistic");
        assert!(body["prompt"].as_str().unwrap().contains("preserving the subject's recognisable identity"));
        let custom = request(Model::FluxFill, b"image", b"mask", "Make the tree autumnal");
        assert_eq!(custom["prompt"], "Make the tree autumnal");
    }

    #[test]
    fn images_are_encoded_as_png_data_uris() {
        assert_eq!(image_uri(b"png"), "data:image/png;base64,cG5n");
    }
}
