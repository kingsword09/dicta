use async_trait::async_trait;
use dicta_core::{AudioInput, LiveEvent};
use std::time::Duration;
use thiserror::Error;

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LiveModeKind {
    Streaming,
    Chunked,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveCapabilities {
    pub mode: LiveModeKind,
    pub mic: bool,
    pub speaker: bool,
    pub streaming_audio: bool,
    pub partial_results: bool,
    pub finalized_results: bool,
    pub translation: bool,
    pub voice_processing: bool,
    pub device_selection: bool,
    pub ptt: bool,
    pub requires_network: bool,
    pub expected_latency: Option<Duration>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderCapabilities {
    pub batch: AsrCapabilities,
    pub live: Option<LiveCapabilities>,
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveAsrOptions {
    pub src: Option<String>,
    pub dst: Option<String>,
    pub mic: bool,
    pub speaker: bool,
    pub voice_processing: bool,
    pub select_device: bool,
    pub chunk_duration: Duration,
}

impl Default for LiveAsrOptions {
    fn default() -> Self {
        Self {
            src: None,
            dst: None,
            mic: true,
            speaker: false,
            voice_processing: false,
            select_device: false,
            chunk_duration: Duration::from_secs(5),
        }
    }
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
    fn provider_capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities {
            batch: self.capabilities(),
            live: None,
            notes: Vec::new(),
        }
    }
}

pub type LiveEventCallback<'a> = &'a mut (dyn FnMut(LiveEvent) -> AsrResult<()> + Send);

#[async_trait]
pub trait LiveAsrProvider: Send + Sync {
    async fn run_live(
        &self,
        options: LiveAsrOptions,
        on_event: LiveEventCallback<'_>,
    ) -> AsrResult<()>;

    fn live_name(&self) -> &'static str;
    fn live_capabilities(&self) -> LiveCapabilities;
}
