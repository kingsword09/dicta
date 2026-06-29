use async_trait::async_trait;
use thiserror::Error;
use vo_core::AudioInput;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AsrOptions {
    pub language: Option<String>,
    pub prompt: Option<String>,
    pub response_format: ResponseFormat,
}

impl Default for AsrOptions {
    fn default() -> Self {
        Self {
            language: None,
            prompt: None,
            response_format: ResponseFormat::Json,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResponseFormat {
    Json,
    Text,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Transcript {
    pub text: String,
    pub language: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AsrCapabilities {
    pub batch_file: bool,
    pub streaming: bool,
    pub requires_network: bool,
}

#[derive(Debug, Error)]
pub enum AsrError {
    #[error("ASR configuration error: {0}")]
    Config(String),
    #[error("ASR request failed: {0}")]
    Request(String),
    #[error("ASR provider returned invalid response: {0}")]
    InvalidResponse(String),
    #[error("ASR input error: {0}")]
    Input(String),
}

pub type AsrResult<T> = Result<T, AsrError>;

#[async_trait]
pub trait AsrProvider: Send + Sync {
    async fn transcribe(&self, input: AudioInput, options: AsrOptions) -> AsrResult<Transcript>;
    fn name(&self) -> &'static str;
    fn capabilities(&self) -> AsrCapabilities;
}
