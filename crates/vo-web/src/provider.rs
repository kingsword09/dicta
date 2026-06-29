use serde::{Deserialize, Serialize};
use wasm_bindgen::{prelude::wasm_bindgen, JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;
use web_sys::{File, FormData, Headers, Request, RequestInit, Response};

use crate::{error, json};
use vo_core::{TranscriptConfidence, TranscriptSource};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum WebProviderKind {
    OpenAiCompatible,
    DoubaoIme,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WebProviderConfig {
    pub provider: WebProviderKind,
    pub api_base: String,
    #[serde(default)]
    pub api_key: Option<String>,
    pub model: String,
    #[serde(default)]
    pub language: Option<String>,
    #[serde(default)]
    pub prompt: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WebTranscription {
    pub provider: WebProviderKind,
    pub source: TranscriptSource,
    pub raw: serde_json::Value,
}

#[derive(Debug, Deserialize)]
struct ProviderResponse {
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    language: Option<String>,
    #[serde(default)]
    confidence: Option<TranscriptConfidence>,
}

#[wasm_bindgen]
pub fn transcription_url(api_base: &str) -> String {
    openai_compatible_transcription_url(api_base)
}

#[wasm_bindgen]
pub async fn transcribe_file(config: JsValue, file: File) -> Result<JsValue, JsValue> {
    let config: WebProviderConfig = json::from_js(&config)?;
    validate_config(&config)?;

    match config.provider {
        WebProviderKind::OpenAiCompatible => transcribe_openai_compatible(config, file).await,
        WebProviderKind::DoubaoIme => Err(error::message(
            "doubao-ime is not available in browser direct mode; use openai-compatible with a CORS-enabled HTTP endpoint",
        )),
    }
}

async fn transcribe_openai_compatible(
    config: WebProviderConfig,
    file: File,
) -> Result<JsValue, JsValue> {
    let form = FormData::new()?;
    form.append_with_blob_and_filename("file", &file, &file.name())?;
    form.append_with_str("model", config.model.trim())?;
    form.append_with_str("response_format", "json")?;

    if let Some(language) = non_empty(config.language.as_deref()) {
        form.append_with_str("language", language)?;
    }
    if let Some(prompt) = non_empty(config.prompt.as_deref()) {
        form.append_with_str("prompt", prompt)?;
    }

    let headers = Headers::new()?;
    if let Some(api_key) = non_empty(config.api_key.as_deref()) {
        headers.set("Authorization", &format!("Bearer {api_key}"))?;
    }

    let init = RequestInit::new();
    init.set_method("POST");
    init.set_body(&form);
    init.set_headers(&headers);

    let request = Request::new_with_str_and_init(
        &openai_compatible_transcription_url(&config.api_base),
        &init,
    )?;
    let window = web_sys::window().ok_or_else(|| error::message("window is unavailable"))?;
    let response = JsFuture::from(window.fetch_with_request(&request))
        .await?
        .dyn_into::<Response>()
        .map_err(|_| error::message("fetch did not return a Response"))?;

    let status = response.status();
    let raw = JsFuture::from(response.text()?)
        .await?
        .as_string()
        .unwrap_or_default();
    if !response.ok() {
        return Err(error::message(format!("HTTP {status}: {raw}")));
    }

    let raw_json = parse_provider_body(&raw);
    let parsed: ProviderResponse = serde_json::from_value(raw_json.clone())
        .map_err(|err| error::message(format!("invalid transcription response: {err}: {raw}")))?;
    let text = parsed.text.unwrap_or_else(|| raw.clone());
    let lang = parsed
        .language
        .or(config.language)
        .unwrap_or_else(|| "und".to_owned());

    json::to_js(&WebTranscription {
        provider: WebProviderKind::OpenAiCompatible,
        source: TranscriptSource {
            lang,
            text,
            confidence: parsed.confidence,
        },
        raw: raw_json,
    })
}

fn validate_config(config: &WebProviderConfig) -> Result<(), JsValue> {
    if config.model.trim().is_empty() {
        return Err(error::message("model is required"));
    }
    if matches!(config.provider, WebProviderKind::OpenAiCompatible)
        && config.api_base.trim().is_empty()
    {
        return Err(error::message("apiBase is required"));
    }
    Ok(())
}

fn openai_compatible_transcription_url(api_base: &str) -> String {
    let base = api_base.trim().trim_end_matches('/');
    if base.ends_with("/v1") {
        format!("{base}/audio/transcriptions")
    } else {
        format!("{base}/v1/audio/transcriptions")
    }
}

fn parse_provider_body(raw: &str) -> serde_json::Value {
    serde_json::from_str(raw).unwrap_or_else(|_| serde_json::json!({ "text": raw }))
}

fn non_empty(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|value| !value.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn appends_v1_audio_transcriptions_to_plain_base_url() {
        assert_eq!(
            openai_compatible_transcription_url("https://example.com"),
            "https://example.com/v1/audio/transcriptions"
        );
    }

    #[test]
    fn reuses_base_url_that_already_points_at_v1() {
        assert_eq!(
            openai_compatible_transcription_url("https://example.com/v1/"),
            "https://example.com/v1/audio/transcriptions"
        );
    }

    #[test]
    fn parses_plain_text_provider_body_as_transcript_text() {
        assert_eq!(
            parse_provider_body("hello"),
            serde_json::json!({ "text": "hello" })
        );
    }
}
