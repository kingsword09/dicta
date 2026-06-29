use async_trait::async_trait;
use reqwest::multipart::{Form, Part};
use serde::Deserialize;
use tokio::fs;
use vo_asr::{AsrCapabilities, AsrError, AsrOptions, AsrProvider, AsrResult, Transcript};
use vo_core::AudioInput;

#[derive(Debug, Clone)]
pub struct OpenAiCompatibleConfig {
    pub base_url: String,
    pub api_key: Option<String>,
    pub model: String,
}

#[derive(Debug, Clone)]
pub struct OpenAiCompatibleAsr {
    client: reqwest::Client,
    config: OpenAiCompatibleConfig,
}

impl OpenAiCompatibleAsr {
    pub fn new(config: OpenAiCompatibleConfig) -> AsrResult<Self> {
        if config.base_url.trim().is_empty() {
            return Err(AsrError::Config("api base URL is required".to_owned()));
        }
        if config.model.trim().is_empty() {
            return Err(AsrError::Config("api model is required".to_owned()));
        }
        Ok(Self {
            client: reqwest::Client::new(),
            config,
        })
    }

    pub fn transcriptions_url(&self) -> String {
        let base = self.config.base_url.trim_end_matches('/');
        if base.ends_with("/v1") {
            format!("{base}/audio/transcriptions")
        } else {
            format!("{base}/v1/audio/transcriptions")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn appends_v1_audio_transcriptions_to_plain_base_url() {
        let asr = OpenAiCompatibleAsr::new(OpenAiCompatibleConfig {
            base_url: "https://example.com".to_owned(),
            api_key: None,
            model: "model".to_owned(),
        })
        .unwrap();

        assert_eq!(
            asr.transcriptions_url(),
            "https://example.com/v1/audio/transcriptions"
        );
    }

    #[test]
    fn reuses_base_url_that_already_points_at_v1() {
        let asr = OpenAiCompatibleAsr::new(OpenAiCompatibleConfig {
            base_url: "https://example.com/v1/".to_owned(),
            api_key: None,
            model: "model".to_owned(),
        })
        .unwrap();

        assert_eq!(
            asr.transcriptions_url(),
            "https://example.com/v1/audio/transcriptions"
        );
    }

    #[test]
    fn rejects_empty_model() {
        let err = OpenAiCompatibleAsr::new(OpenAiCompatibleConfig {
            base_url: "https://example.com".to_owned(),
            api_key: None,
            model: "".to_owned(),
        })
        .unwrap_err();

        assert!(matches!(err, AsrError::Config(_)));
    }
}

#[derive(Debug, Deserialize)]
struct JsonTranscriptionResponse {
    text: String,
    #[serde(default)]
    language: Option<String>,
}

#[async_trait]
impl AsrProvider for OpenAiCompatibleAsr {
    async fn transcribe(&self, input: AudioInput, options: AsrOptions) -> AsrResult<Transcript> {
        let filename = input.filename();
        let bytes = match input {
            AudioInput::File(path) => fs::read(&path).await.map_err(|err| {
                AsrError::Input(format!("failed to read {}: {err}", path.display()))
            })?,
            AudioInput::Bytes { data, .. } => data,
        };
        if bytes.is_empty() {
            return Err(AsrError::Input("audio input is empty".to_owned()));
        }

        let mut form = Form::new()
            .part("file", Part::bytes(bytes).file_name(filename))
            .text("model", self.config.model.clone())
            .text("response_format", "json");

        if let Some(language) = options.language {
            form = form.text("language", language);
        }
        if let Some(prompt) = options.prompt {
            form = form.text("prompt", prompt);
        }

        let mut request = self.client.post(self.transcriptions_url()).multipart(form);
        if let Some(api_key) = &self.config.api_key {
            request = request.bearer_auth(api_key);
        }

        let response = request
            .send()
            .await
            .map_err(|err| AsrError::Request(err.to_string()))?;
        let status = response.status();
        let body = response
            .text()
            .await
            .map_err(|err| AsrError::Request(format!("failed to read response body: {err}")))?;

        if !status.is_success() {
            return Err(AsrError::Request(format!("HTTP {status}: {body}")));
        }

        let parsed: JsonTranscriptionResponse = serde_json::from_str(&body)
            .map_err(|err| AsrError::InvalidResponse(format!("{err}: {body}")))?;
        Ok(Transcript {
            text: parsed.text,
            language: parsed.language,
        })
    }

    fn name(&self) -> &'static str {
        "openai-compatible"
    }

    fn capabilities(&self) -> AsrCapabilities {
        AsrCapabilities {
            batch_file: true,
            streaming: false,
            requires_network: true,
        }
    }
}
