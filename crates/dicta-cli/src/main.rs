use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, Multipart, State, multipart::MultipartRejection},
    http::{HeaderValue, Method, StatusCode, header},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64_STANDARD};
use clap::{Args, Parser, Subcommand, ValueEnum};
use dicta_asr::{
    AsrCapabilities, AsrOptions, AsrProvider, LiveAsrOptions, LiveAsrProvider, LiveCapabilities,
    LiveEventCallback, LiveModeKind, ProviderCapabilities, ResponseFormat,
};
use dicta_asr_native_adapter::{
    NativeAdapterAsr, NativeAdapterConfig, native_adapter_capabilities,
    native_adapter_live_capabilities,
};
use dicta_asr_openai_compatible::{
    OpenAiCompatibleAsr, OpenAiCompatibleConfig, openai_compatible_capabilities,
};
use dicta_core::{
    AudioChannel, AudioInput, EventTimestamp, LiveEvent, LiveMetaEvent, LiveStatusEvent,
    LiveStatusPhase, LiveTranslatedEvent, LiveVolatileEvent, TranscriptEvent, TranscriptSource,
    TranscriptTarget,
};
use flate2::read::GzDecoder;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha512};
use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::{self, IsTerminal, Write};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use std::{cmp, env};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::time::{self, Instant as TokioInstant};
use tower_http::cors::{Any, CorsLayer};

const APP_NAME: &str = "dicta";
const CONFIG_DIR_NAME: &str = "dicta";
const PROVIDER_PROTOCOL: &str = "dicta-provider-jsonl-v1";
const DEFAULT_NPM_REGISTRY: &str = "https://registry.npmjs.org";
const DEFAULT_PROVIDER_SCOPE: &str = "dicta-asr";
const DEFAULT_PROVIDER_KEYWORD: &str = "dicta-provider";
const PROVIDER_INSTALL_METADATA_FILE: &str = ".dicta-provider-install.json";

#[derive(Debug, Clone, Parser)]
#[command(name = "dicta")]
#[command(version)]
#[command(about = "Cross-platform transcription CLI with pluggable ASR providers")]
struct Cli {
    #[arg(long, value_enum, default_value_t = AsrBackend::Auto, env = "DICTA_ASR_BACKEND", help = "ASR backend to use")]
    asr: AsrBackend,

    #[arg(
        long = "api-base",
        env = "DICTA_ASR_API_BASE",
        help = "Provider API base URL"
    )]
    api_base: Option<String>,

    #[arg(long = "api-key", env = "DICTA_ASR_API_KEY", help = "Provider API key")]
    api_key: Option<String>,

    #[arg(
        long = "api-model",
        env = "DICTA_ASR_API_MODEL",
        help = "Provider model id"
    )]
    api_model: Option<String>,

    #[arg(
        long,
        env = "DICTA_PROVIDER",
        help = "Named provider profile from built-ins or provider config"
    )]
    provider: Option<String>,

    #[arg(
        long = "provider-config",
        env = "DICTA_PROVIDER_CONFIG",
        help = "Path to provider profiles TOML"
    )]
    provider_config: Option<PathBuf>,

    #[arg(long, env = "DICTA_SRC", help = "Source language/locale hint")]
    src: Option<String>,

    #[arg(
        long,
        env = "DICTA_DST",
        help = "Target language/locale for Apple on-device live translation"
    )]
    dst: Option<String>,

    #[arg(
        long = "native-adapter",
        env = "DICTA_NATIVE_ADAPTER",
        help = "Path to the native adapter binary used for platform on-device ASR"
    )]
    native_adapter: Option<PathBuf>,

    #[arg(long, help = "Audio file to transcribe")]
    input: Option<PathBuf>,

    #[arg(
        long,
        help = "Record the default microphone for N seconds before transcribing"
    )]
    mic_duration: Option<f64>,

    #[arg(
        long,
        help = "Run live transcription; default when no --input or --mic-duration is given"
    )]
    live: bool,

    #[arg(
        long = "live-chunk",
        env = "DICTA_LIVE_CHUNK",
        help = "Chunk duration in seconds for chunked live providers"
    )]
    live_chunk: Option<f64>,

    #[arg(long = "no-mic", help = "Disable microphone capture in live mode")]
    no_mic: bool,

    #[arg(
        long = "no-speaker",
        help = "Disable system audio capture when live backend supports it"
    )]
    no_speaker: bool,

    #[arg(
        long = "voice-processing",
        help = "Enable voice processing when live backend supports it"
    )]
    voice_processing: bool,

    #[arg(
        long = "select-device",
        help = "Interactively select capture devices when live backend supports it"
    )]
    select_device: bool,

    #[arg(long, help = "Emit one JSON object for the finalized transcript")]
    json: bool,

    #[arg(long, help = "Write the finalized transcript to this path")]
    transcript: Option<PathBuf>,

    #[arg(long, help = "Print environment and backend diagnostics")]
    doctor: bool,

    #[arg(long, help = "Print ASR provider capability diagnostics")]
    capabilities: bool,

    #[arg(long, help = "Launch the status bar UI for live provider switching")]
    ui: bool,

    #[arg(long = "provider-state", env = "DICTA_PROVIDER_STATE", hide = true)]
    provider_state: Option<PathBuf>,

    #[arg(long = "provider-dir", env = "DICTA_PROVIDER_DIR", hide = true)]
    provider_dir: Option<PathBuf>,

    #[command(subcommand)]
    command: Option<Command>,
}

impl Cli {
    fn with_provider_name(&self, provider: Option<String>) -> Self {
        Self {
            asr: self.asr,
            api_base: self.api_base.clone(),
            api_key: self.api_key.clone(),
            api_model: self.api_model.clone(),
            provider,
            provider_config: self.provider_config.clone(),
            src: self.src.clone(),
            dst: self.dst.clone(),
            native_adapter: self.native_adapter.clone(),
            input: self.input.clone(),
            mic_duration: self.mic_duration,
            live: self.live,
            live_chunk: self.live_chunk,
            no_mic: self.no_mic,
            no_speaker: self.no_speaker,
            voice_processing: self.voice_processing,
            select_device: self.select_device,
            json: self.json,
            transcript: self.transcript.clone(),
            doctor: self.doctor,
            capabilities: self.capabilities,
            ui: self.ui,
            provider_state: self.provider_state.clone(),
            provider_dir: self.provider_dir.clone(),
            command: self.command.clone(),
        }
    }
}

#[derive(Debug, Clone, Subcommand)]
enum Command {
    #[command(about = "Serve an OpenAI-compatible ASR HTTP API")]
    Serve(ServeCommand),
    #[command(about = "Manage named ASR provider selections")]
    Provider(ProviderCommand),
    #[command(about = "Update installed dicta binaries from GitHub Releases")]
    Update(UpdateCommand),
    #[command(about = "Uninstall installed dicta binaries")]
    Uninstall(UninstallCommand),
}

#[derive(Debug, Clone, Args)]
struct ServeCommand {
    #[arg(
        long,
        env = "DICTA_SERVE_HOST",
        default_value_t = IpAddr::V4(Ipv4Addr::LOCALHOST),
        help = "Host/IP to bind the HTTP server"
    )]
    host: IpAddr,

    #[arg(
        long,
        env = "DICTA_SERVE_PORT",
        default_value_t = 4777,
        help = "Port to bind the HTTP server"
    )]
    port: u16,

    #[arg(
        long = "cors-origin",
        env = "DICTA_SERVE_CORS_ORIGIN",
        value_delimiter = ',',
        help = "Allowed browser CORS origin; pass '*' for local development"
    )]
    cors_origins: Vec<String>,

    #[arg(
        long = "max-upload-mb",
        env = "DICTA_SERVE_MAX_UPLOAD_MB",
        default_value_t = 25,
        help = "Maximum multipart upload size in MiB"
    )]
    max_upload_mb: usize,
}

#[derive(Debug, Clone, Args)]
struct UpdateCommand {
    #[arg(
        long,
        env = "DICTA_VERSION",
        help = "Release version or tag, default: latest"
    )]
    version: Option<String>,

    #[arg(
        long = "install-dir",
        env = "DICTA_INSTALL_DIR",
        help = "Install directory, default: directory containing the current dicta binary"
    )]
    install_dir: Option<PathBuf>,
}

#[derive(Debug, Clone, Args)]
struct UninstallCommand {
    #[arg(
        long = "install-dir",
        env = "DICTA_INSTALL_DIR",
        help = "Install directory, default: directory containing the current dicta binary"
    )]
    install_dir: Option<PathBuf>,

    #[arg(short = 'y', long, help = "Do not ask for confirmation")]
    yes: bool,
}

#[derive(Debug, Clone, Args)]
struct ProviderCommand {
    #[command(subcommand)]
    action: ProviderAction,
}

#[derive(Debug, Clone, Subcommand)]
enum ProviderAction {
    #[command(about = "List built-in and configured providers")]
    List,
    #[command(about = "List installable provider packages from the npm registry")]
    Available(ProviderAvailableCommand),
    #[command(about = "Show the provider selected for --provider active")]
    Current,
    #[command(about = "Set the provider selected by --provider active")]
    Set { name: String },
    #[command(about = "Install an ASR provider package without npm install")]
    Install(ProviderInstallCommand),
    #[command(about = "Update one installed provider or all installed providers")]
    Update(ProviderUpdateCommand),
    #[command(visible_alias = "uninstall", about = "Remove an installed provider")]
    Remove(ProviderRemoveCommand),
}

#[derive(Debug, Clone, Args)]
struct ProviderAvailableCommand {
    #[arg(
        long,
        default_value = DEFAULT_PROVIDER_SCOPE,
        help = "npm scope to search for installable provider packages"
    )]
    scope: String,

    #[arg(
        long,
        default_value = DEFAULT_PROVIDER_KEYWORD,
        help = "npm keyword required for installable provider packages"
    )]
    keyword: String,

    #[arg(
        long,
        default_value = DEFAULT_NPM_REGISTRY,
        help = "npm-compatible registry used for package discovery"
    )]
    registry: String,

    #[arg(
        long,
        default_value_t = 50,
        help = "Maximum number of packages to show"
    )]
    limit: usize,
}

#[derive(Debug, Clone, Args)]
struct ProviderInstallCommand {
    #[arg(help = "Provider package name, local provider directory, or .tgz package")]
    package: String,

    #[arg(long, help = "Provider version or npm dist-tag, default: latest")]
    version: Option<String>,

    #[arg(
        long,
        default_value = DEFAULT_NPM_REGISTRY,
        help = "npm-compatible registry used for package names"
    )]
    registry: String,

    #[arg(long, help = "Replace an existing installed provider")]
    force: bool,
}

#[derive(Debug, Clone, Args)]
struct ProviderUpdateCommand {
    #[arg(
        help = "Installed provider id or npm package name; omit to update all installed providers"
    )]
    name: Option<String>,

    #[arg(long, help = "Provider version or npm dist-tag, default: latest")]
    version: Option<String>,

    #[arg(
        long,
        default_value = DEFAULT_NPM_REGISTRY,
        help = "npm-compatible registry used for updates"
    )]
    registry: String,

    #[arg(
        long,
        help = "Reinstall even when the requested version is already installed"
    )]
    force: bool,
}

#[derive(Debug, Clone, Args)]
struct ProviderRemoveCommand {
    #[arg(help = "Installed provider id or npm package name")]
    name: String,

    #[arg(short = 'y', long, help = "Do not ask for confirmation")]
    yes: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum AsrBackend {
    Auto,
    OpenaiCompatible,
    Apple,
    #[value(skip)]
    External,
}

impl AsrBackend {
    fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::OpenaiCompatible => "openai-compatible",
            Self::Apple => "apple",
            Self::External => "external",
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
struct ProviderProfilesFile {
    #[serde(default)]
    providers: BTreeMap<String, ProviderProfile>,
}

#[derive(Debug, Clone, Deserialize)]
struct ProviderProfile {
    kind: ProfileProviderKind,
    #[serde(default)]
    api_base: Option<String>,
    #[serde(default)]
    default_model: Option<String>,
    #[serde(default)]
    api_key: Option<String>,
    #[serde(default)]
    api_key_env: Option<String>,
    #[serde(default)]
    batch_file: Option<bool>,
    #[serde(default)]
    streaming: Option<bool>,
    #[serde(default)]
    requires_network: Option<bool>,
    #[serde(default)]
    live_enabled: Option<bool>,
    #[serde(default)]
    notes: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum ProfileProviderKind {
    OpenaiCompatible,
    Apple,
    External,
}

impl ProfileProviderKind {
    fn backend(self) -> AsrBackend {
        match self {
            Self::OpenaiCompatible => AsrBackend::OpenaiCompatible,
            Self::Apple => AsrBackend::Apple,
            Self::External => AsrBackend::External,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::OpenaiCompatible => "openai-compatible",
            Self::Apple => "apple",
            Self::External => "external",
        }
    }
}

#[derive(Debug, Clone)]
struct ResolvedProviderProfile {
    name: String,
    profile: ProviderProfile,
    installed: Option<InstalledProvider>,
}

#[derive(Debug, Clone)]
struct InstalledProvider {
    root: PathBuf,
    manifest: ProviderPackageManifest,
    install_metadata: Option<ProviderInstallMetadata>,
}

impl InstalledProvider {
    fn id(&self) -> &str {
        &self.manifest.id
    }

    fn command_path(&self) -> PathBuf {
        self.root.join(&self.manifest.command)
    }

    fn profile(&self) -> ProviderProfile {
        ProviderProfile {
            kind: ProfileProviderKind::External,
            api_base: None,
            default_model: Some(self.manifest.model()),
            api_key: None,
            api_key_env: None,
            batch_file: Some(self.manifest.batch.file),
            streaming: Some(self.manifest.batch.streaming),
            requires_network: Some(
                self.manifest.batch.requires_network
                    || self
                        .manifest
                        .live
                        .as_ref()
                        .is_some_and(|live| live.requires_network),
            ),
            live_enabled: Some(self.manifest.live.is_some()),
            notes: Vec::new(),
        }
    }

    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities {
            batch: AsrCapabilities {
                batch_file: self.manifest.batch.file,
                streaming: self.manifest.batch.streaming,
                requires_network: self.manifest.batch.requires_network,
            },
            live: self
                .manifest
                .live
                .as_ref()
                .map(ProviderLiveManifest::capabilities),
            notes: self.manifest.notes.clone(),
        }
    }

    fn source_package(&self) -> Option<&str> {
        self.install_metadata
            .as_ref()
            .and_then(|metadata| metadata.package.as_deref())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ProviderInstallMetadata {
    source: ProviderInstallSourceKind,
    #[serde(default)]
    package: Option<String>,
    #[serde(default)]
    registry: Option<String>,
    #[serde(default)]
    version: Option<String>,
    installed_at_ms: u128,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum ProviderInstallSourceKind {
    Npm,
    Directory,
    Tarball,
}

#[derive(Debug, Clone, Deserialize)]
struct ProviderPackageManifest {
    id: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    version: Option<String>,
    protocol: String,
    command: PathBuf,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    batch: ProviderBatchManifest,
    #[serde(default)]
    live: Option<ProviderLiveManifest>,
    #[serde(default)]
    notes: Vec<String>,
}

impl ProviderPackageManifest {
    fn model(&self) -> String {
        non_empty_string(self.model.as_deref())
            .unwrap_or(&self.id)
            .to_owned()
    }
}

#[derive(Debug, Clone, Deserialize)]
struct ProviderBatchManifest {
    #[serde(default = "default_true")]
    file: bool,
    #[serde(default)]
    streaming: bool,
    #[serde(default)]
    requires_network: bool,
}

impl Default for ProviderBatchManifest {
    fn default() -> Self {
        Self {
            file: true,
            streaming: false,
            requires_network: false,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
struct ProviderLiveManifest {
    #[serde(default = "default_streaming_mode")]
    mode: ProviderLiveModeManifest,
    #[serde(default = "default_true")]
    mic: bool,
    #[serde(default)]
    speaker: bool,
    #[serde(default)]
    streaming_audio: bool,
    #[serde(default)]
    partial_results: bool,
    #[serde(default = "default_true")]
    finalized_results: bool,
    #[serde(default)]
    translation: bool,
    #[serde(default)]
    voice_processing: bool,
    #[serde(default)]
    device_selection: bool,
    #[serde(default)]
    requires_network: bool,
    #[serde(default)]
    expected_latency_ms: Option<u64>,
}

impl ProviderLiveManifest {
    fn capabilities(&self) -> LiveCapabilities {
        LiveCapabilities {
            mode: self.mode.into(),
            mic: self.mic,
            speaker: self.speaker,
            streaming_audio: self.streaming_audio,
            partial_results: self.partial_results,
            finalized_results: self.finalized_results,
            translation: self.translation,
            voice_processing: self.voice_processing,
            device_selection: self.device_selection,
            requires_network: self.requires_network,
            expected_latency: self.expected_latency_ms.map(Duration::from_millis),
        }
    }
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum ProviderLiveModeManifest {
    Streaming,
    Chunked,
}

impl From<ProviderLiveModeManifest> for LiveModeKind {
    fn from(value: ProviderLiveModeManifest) -> Self {
        match value {
            ProviderLiveModeManifest::Streaming => LiveModeKind::Streaming,
            ProviderLiveModeManifest::Chunked => LiveModeKind::Chunked,
        }
    }
}

fn default_true() -> bool {
    true
}

fn default_streaming_mode() -> ProviderLiveModeManifest {
    ProviderLiveModeManifest::Streaming
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ActiveProviderState {
    provider: Option<String>,
    updated_at_ms: u128,
}

#[derive(Debug, Clone, Serialize)]
struct ProviderListReport {
    current: Option<String>,
    state_path: Option<String>,
    provider_config: Option<String>,
    providers: Vec<ProviderListEntry>,
}

#[derive(Debug, Clone, Serialize)]
struct ProviderListEntry {
    name: String,
    kind: String,
    built_in: bool,
    installed: bool,
    install_path: Option<String>,
    installed_version: Option<String>,
    source_package: Option<String>,
    selected: bool,
    model: String,
    batch_file: bool,
    live: bool,
    local_config_ok: bool,
    local_config_error: Option<String>,
    notes: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
struct CurrentProviderReport {
    provider: Option<String>,
    state_path: Option<String>,
    resolved: Option<String>,
    kind: Option<String>,
    model: Option<String>,
    live: bool,
    local_config_ok: bool,
    local_config_error: Option<String>,
    install_path: Option<String>,
}

#[derive(Debug, Clone)]
struct EffectiveProvider {
    backend: AsrBackend,
    profile: Option<ResolvedProviderProfile>,
    capabilities: ProviderCapabilities,
    config_error: Option<String>,
}

impl EffectiveProvider {
    fn profile_name(&self) -> Option<&str> {
        self.profile.as_ref().map(|profile| profile.name.as_str())
    }

    fn profile_kind(&self) -> Option<&'static str> {
        self.profile
            .as_ref()
            .map(|profile| profile.profile.kind.as_str())
    }

    fn installed_provider(&self) -> Option<&InstalledProvider> {
        self.profile
            .as_ref()
            .and_then(|profile| profile.installed.as_ref())
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    if cli.ui {
        run_ui(&cli).await?;
        return Ok(());
    }

    if let Some(command) = &cli.command {
        run_command(&cli, command).await?;
        return Ok(());
    }

    if cli.capabilities {
        run_capabilities(&cli)?;
        return Ok(());
    }

    if cli.doctor {
        run_doctor(&cli)?;
        return Ok(());
    }

    if should_run_live(&cli) {
        run_live(&cli).await?;
        return Ok(());
    }

    validate_batch_options(&cli)?;

    let audio_source = resolve_audio_source(&cli)?;
    if let Some(path) = &cli.transcript {
        validate_transcript_path(path)?;
    }

    let backend = resolve_backend(&cli)?;
    let provider = build_provider(backend, &cli)?;

    let transcript = provider
        .transcribe(
            AudioInput::File(audio_source.path.clone()),
            AsrOptions {
                language: cli.src.clone(),
                ..AsrOptions::default()
            },
        )
        .await
        .with_context(|| format!("{} transcription failed", provider.name()))?;

    let payload = OutputPayload::new(transcript, audio_source.channel, cli.src.clone());
    if let Some(path) = &cli.transcript {
        write_transcript(path, &payload, cli.json)?;
    }
    if cli.json {
        println!("{}", payload.jsonl()?);
    } else {
        println!("{}", payload.text);
    }

    audio_source.cleanup();
    Ok(())
}

async fn run_command(cli: &Cli, command: &Command) -> Result<()> {
    match command {
        Command::Serve(command) => run_serve(cli, command).await,
        Command::Provider(command) => run_provider_command(cli, command).await,
        Command::Update(command) => run_update_command(command).await,
        Command::Uninstall(command) => run_uninstall_command(command),
    }
}

async fn run_serve(cli: &Cli, command: &ServeCommand) -> Result<()> {
    let max_upload_bytes = max_upload_bytes(command.max_upload_mb)?;
    let addr = SocketAddr::new(command.host, command.port);
    let state = ServeState {
        cli: Arc::new(cli.clone()),
    };
    let app = serve_router(state, command, max_upload_bytes)?;
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .with_context(|| format!("failed to bind dicta serve on {addr}"))?;
    let local_addr = listener
        .local_addr()
        .context("failed to read dicta serve listen address")?;

    eprintln!("dicta serve listening on http://{local_addr}");
    eprintln!("  POST http://{local_addr}/v1/audio/transcriptions");
    axum::serve(listener, app)
        .await
        .context("dicta serve HTTP server failed")
}

fn serve_router(
    state: ServeState,
    command: &ServeCommand,
    max_upload_bytes: usize,
) -> Result<Router> {
    let mut router = Router::new()
        .route("/health", get(serve_health))
        .route("/v1/models", get(serve_models))
        .route("/v1/audio/transcriptions", post(serve_transcriptions))
        .layer(DefaultBodyLimit::max(max_upload_bytes))
        .with_state(state);

    if !command.cors_origins.is_empty() {
        router = router.layer(serve_cors_layer(&command.cors_origins)?);
    }

    Ok(router)
}

fn max_upload_bytes(max_upload_mb: usize) -> Result<usize> {
    if max_upload_mb == 0 {
        bail!("--max-upload-mb must be greater than zero");
    }
    max_upload_mb
        .checked_mul(1024 * 1024)
        .context("--max-upload-mb is too large")
}

fn serve_cors_layer(origins: &[String]) -> Result<CorsLayer> {
    let layer = CorsLayer::new()
        .allow_methods([Method::GET, Method::POST])
        .allow_headers([header::AUTHORIZATION, header::CONTENT_TYPE]);
    let origins = origins
        .iter()
        .filter_map(|origin| non_empty_string(Some(origin)).map(ToOwned::to_owned))
        .collect::<Vec<_>>();

    if origins.iter().any(|origin| origin == "*") {
        return Ok(layer.allow_origin(Any));
    }

    let origins = origins
        .iter()
        .map(|origin| {
            HeaderValue::from_str(origin)
                .with_context(|| format!("invalid --cors-origin value: {origin}"))
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(layer.allow_origin(origins))
}

#[derive(Clone)]
struct ServeState {
    cli: Arc<Cli>,
}

#[derive(Debug, Serialize)]
struct ServeHealth {
    status: &'static str,
    version: &'static str,
    backend: Option<String>,
    provider: Option<String>,
    model: String,
    local_config_ok: bool,
    local_config_error: Option<String>,
}

#[derive(Debug, Serialize)]
struct ServeModelList {
    object: &'static str,
    data: Vec<ServeModel>,
}

#[derive(Debug, Serialize)]
struct ServeModel {
    id: String,
    object: &'static str,
    owned_by: &'static str,
}

#[derive(Debug, Serialize)]
struct ServeTranscriptionResponse {
    text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    language: Option<String>,
}

#[derive(Debug)]
struct ServeTranscriptionRequest {
    audio: ServeUploadedAudio,
    model: Option<String>,
    language: Option<String>,
    prompt: Option<String>,
    response_format: ServeResponseFormat,
}

#[derive(Debug)]
struct ServeUploadedAudio {
    data: Vec<u8>,
    filename: String,
    mime_type: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ServeResponseFormat {
    Json,
    Text,
}

impl ServeResponseFormat {
    fn as_asr_response_format(self) -> ResponseFormat {
        match self {
            Self::Json => ResponseFormat::Json,
            Self::Text => ResponseFormat::Text,
        }
    }
}

#[derive(Debug, Serialize)]
struct ServeErrorResponse {
    error: ServeErrorBody,
}

#[derive(Debug, Serialize)]
struct ServeErrorBody {
    message: String,
    #[serde(rename = "type")]
    error_type: &'static str,
    param: Option<String>,
    code: Option<String>,
}

#[derive(Debug)]
struct ServeApiError {
    status: StatusCode,
    message: String,
    error_type: &'static str,
    param: Option<String>,
    code: Option<String>,
}

impl ServeApiError {
    fn bad_request(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            message: message.into(),
            error_type: "invalid_request_error",
            param: None,
            code: None,
        }
    }

    fn invalid_param(param: &str, message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            message: message.into(),
            error_type: "invalid_request_error",
            param: Some(param.to_owned()),
            code: None,
        }
    }

    fn server_config(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            message: message.into(),
            error_type: "server_error",
            param: None,
            code: None,
        }
    }

    fn provider(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_GATEWAY,
            message: message.into(),
            error_type: "provider_error",
            param: None,
            code: None,
        }
    }

    fn from_multipart_rejection(err: MultipartRejection) -> Self {
        Self {
            status: err.status(),
            message: format!("invalid multipart form: {err}"),
            error_type: "invalid_request_error",
            param: None,
            code: None,
        }
    }

    fn from_asr_error(err: dicta_asr::AsrError) -> Self {
        match err {
            dicta_asr::AsrError::Input(message) => Self::bad_request(message),
            dicta_asr::AsrError::Config(message) => Self::server_config(message),
            dicta_asr::AsrError::Request(message)
            | dicta_asr::AsrError::InvalidResponse(message) => Self::provider(message),
        }
    }
}

impl IntoResponse for ServeApiError {
    fn into_response(self) -> Response {
        let status = self.status;
        let body = Json(ServeErrorResponse {
            error: ServeErrorBody {
                message: self.message,
                error_type: self.error_type,
                param: self.param,
                code: self.code,
            },
        });
        (status, body).into_response()
    }
}

async fn serve_health(State(state): State<ServeState>) -> Json<ServeHealth> {
    Json(serve_health_report(&state.cli))
}

async fn serve_models(State(state): State<ServeState>) -> Json<ServeModelList> {
    let report = gather_capabilities_report(&state.cli);
    let mut data = vec![ServeModel {
        id: "dicta".to_owned(),
        object: "model",
        owned_by: "dicta",
    }];
    if report.model != "dicta" {
        data.push(ServeModel {
            id: report.model,
            object: "model",
            owned_by: "dicta",
        });
    }

    Json(ServeModelList {
        object: "list",
        data,
    })
}

async fn serve_transcriptions(
    State(state): State<ServeState>,
    multipart: std::result::Result<Multipart, MultipartRejection>,
) -> std::result::Result<Response, ServeApiError> {
    let request =
        parse_transcription_multipart(multipart.map_err(ServeApiError::from_multipart_rejection)?)
            .await?;
    let response_format = request.response_format;
    let transcript = transcribe_serve_request(&state.cli, request).await?;
    Ok(transcription_response(transcript, response_format))
}

fn serve_health_report(cli: &Cli) -> ServeHealth {
    let report = gather_capabilities_report(cli);
    let local_config_error = report.local_config_error.or(report.resolution_error);
    ServeHealth {
        status: if report.local_config_ok {
            "ok"
        } else {
            "degraded"
        },
        version: env!("CARGO_PKG_VERSION"),
        backend: report.resolved,
        provider: report.provider,
        model: report.model,
        local_config_ok: report.local_config_ok,
        local_config_error,
    }
}

async fn parse_transcription_multipart(
    mut multipart: Multipart,
) -> std::result::Result<ServeTranscriptionRequest, ServeApiError> {
    let mut audio = None;
    let mut model = None;
    let mut language = None;
    let mut prompt = None;
    let mut response_format = ServeResponseFormat::Json;

    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|err| ServeApiError::bad_request(format!("invalid multipart field: {err}")))?
    {
        let Some(name) = field.name().map(ToOwned::to_owned) else {
            continue;
        };
        match name.as_str() {
            "file" => {
                if audio.is_some() {
                    return Err(ServeApiError::invalid_param(
                        "file",
                        "only one audio file is supported",
                    ));
                }
                audio = Some(read_uploaded_audio(field).await?);
            }
            "model" => {
                model = non_empty_string(Some(&read_multipart_text(field).await?))
                    .map(ToOwned::to_owned);
            }
            "language" => {
                language = non_empty_string(Some(&read_multipart_text(field).await?))
                    .map(ToOwned::to_owned);
            }
            "prompt" => {
                prompt = non_empty_string(Some(&read_multipart_text(field).await?))
                    .map(ToOwned::to_owned);
            }
            "response_format" => {
                let value = read_multipart_text(field).await?;
                response_format = parse_serve_response_format(Some(&value))?;
            }
            "stream" => {
                let value = read_multipart_text(field).await?;
                if parse_form_bool(&value, "stream")? {
                    return Err(ServeApiError::invalid_param(
                        "stream",
                        "streaming transcription is not supported by dicta serve yet",
                    ));
                }
            }
            "timestamp_granularities" | "timestamp_granularities[]" => {
                let _ = read_multipart_text(field).await?;
                return Err(ServeApiError::invalid_param(
                    "timestamp_granularities",
                    "timestamp granularities are not supported by dicta serve yet",
                ));
            }
            "temperature" => {
                let _ = read_multipart_text(field).await?;
            }
            _ => {
                let _ = read_multipart_text(field).await;
            }
        }
    }

    let audio = audio.ok_or_else(|| {
        ServeApiError::invalid_param("file", "multipart field 'file' is required")
    })?;
    if audio.data.is_empty() {
        return Err(ServeApiError::invalid_param(
            "file",
            "uploaded audio file is empty",
        ));
    }

    Ok(ServeTranscriptionRequest {
        audio,
        model,
        language,
        prompt,
        response_format,
    })
}

async fn read_uploaded_audio(
    field: axum::extract::multipart::Field<'_>,
) -> std::result::Result<ServeUploadedAudio, ServeApiError> {
    let filename = field
        .file_name()
        .and_then(|name| non_empty_string(Some(name)).map(ToOwned::to_owned))
        .unwrap_or_else(|| "audio".to_owned());
    let mime_type = field
        .content_type()
        .and_then(|mime| non_empty_string(Some(mime)).map(ToOwned::to_owned));
    let data = field
        .bytes()
        .await
        .map_err(|err| ServeApiError::bad_request(format!("failed to read uploaded file: {err}")))?
        .to_vec();

    Ok(ServeUploadedAudio {
        data,
        filename,
        mime_type,
    })
}

async fn read_multipart_text(
    field: axum::extract::multipart::Field<'_>,
) -> std::result::Result<String, ServeApiError> {
    field
        .text()
        .await
        .map_err(|err| ServeApiError::bad_request(format!("failed to read multipart field: {err}")))
}

async fn transcribe_serve_request(
    cli: &Cli,
    request: ServeTranscriptionRequest,
) -> std::result::Result<dicta_asr::Transcript, ServeApiError> {
    let mut request_cli = cli.clone();
    if let Some(model) = request.model.as_deref().and_then(serve_model_override) {
        request_cli.api_model = Some(model);
    }
    let backend = resolve_backend(&request_cli).map_err(|err| {
        ServeApiError::server_config(format!("ASR backend resolution failed: {err}"))
    })?;
    let provider = build_provider(backend, &request_cli)
        .map_err(|err| ServeApiError::server_config(format!("ASR provider setup failed: {err}")))?;
    let options = AsrOptions {
        language: request.language.or_else(|| request_cli.src.clone()),
        prompt: request.prompt,
        response_format: request.response_format.as_asr_response_format(),
    };

    if matches!(backend, AsrBackend::Apple | AsrBackend::External) {
        let path = temp_upload_path(&request.audio.filename);
        tokio::fs::write(&path, &request.audio.data)
            .await
            .map_err(|err| {
                ServeApiError::bad_request(format!(
                    "failed to stage uploaded audio at {}: {err}",
                    path.display()
                ))
            })?;
        let result = provider
            .transcribe(AudioInput::File(path.clone()), options)
            .await
            .map_err(ServeApiError::from_asr_error);
        let _ = tokio::fs::remove_file(path).await;
        return result;
    }

    provider
        .transcribe(
            AudioInput::Bytes {
                data: request.audio.data,
                filename: request.audio.filename,
                mime_type: request.audio.mime_type,
            },
            options,
        )
        .await
        .map_err(ServeApiError::from_asr_error)
}

fn transcription_response(
    transcript: dicta_asr::Transcript,
    response_format: ServeResponseFormat,
) -> Response {
    match response_format {
        ServeResponseFormat::Json => Json(ServeTranscriptionResponse {
            text: transcript.text,
            language: transcript.language,
        })
        .into_response(),
        ServeResponseFormat::Text => (
            [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
            transcript.text,
        )
            .into_response(),
    }
}

fn parse_serve_response_format(
    value: Option<&str>,
) -> std::result::Result<ServeResponseFormat, ServeApiError> {
    match value.map(str::trim).filter(|value| !value.is_empty()) {
        None | Some("json") => Ok(ServeResponseFormat::Json),
        Some("text") => Ok(ServeResponseFormat::Text),
        Some(other) => Err(ServeApiError::invalid_param(
            "response_format",
            format!("response_format '{other}' is not supported; use 'json' or 'text'"),
        )),
    }
}

fn serve_model_override(model: &str) -> Option<String> {
    non_empty_string(Some(model))
        .filter(|model| !matches!(*model, "dicta" | "default"))
        .map(ToOwned::to_owned)
}

fn parse_form_bool(value: &str, param: &str) -> std::result::Result<bool, ServeApiError> {
    match value.trim().to_ascii_lowercase().as_str() {
        "" | "false" | "0" | "no" | "off" => Ok(false),
        "true" | "1" | "yes" | "on" => Ok(true),
        _ => Err(ServeApiError::invalid_param(
            param,
            format!("{param} must be true or false"),
        )),
    }
}

fn temp_upload_path(filename: &str) -> PathBuf {
    let extension = safe_upload_extension(filename).unwrap_or_else(|| "audio".to_owned());
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    std::env::temp_dir().join(format!(
        "dicta-serve-{}-{nanos}.{extension}",
        std::process::id()
    ))
}

fn safe_upload_extension(filename: &str) -> Option<String> {
    Path::new(filename)
        .extension()
        .and_then(|extension| extension.to_str())
        .map(str::trim)
        .filter(|extension| !extension.is_empty())
        .map(|extension| {
            extension
                .chars()
                .filter(|ch| ch.is_ascii_alphanumeric())
                .take(16)
                .collect::<String>()
        })
        .filter(|extension| !extension.is_empty())
}

async fn run_ui(cli: &Cli) -> Result<()> {
    if let Some(provider) = non_empty(&cli.provider).filter(|provider| provider != "active") {
        let profiles = available_provider_profiles(cli)?;
        if !profiles.contains_key(&provider) {
            bail!(
                "provider '{provider}' was not found; run `dicta provider list` to see available providers"
            );
        }
        write_active_provider_name(cli, &provider)?;
    }

    let dicta_bin =
        std::env::current_exe().context("failed to resolve current dicta executable")?;
    let launcher = resolve_tray_launcher().context(
        "could not find dicta-tray; install the release companion binary or run `cargo build -p dicta-tray`",
    )?;
    let description = launcher.description();
    let mut command = launcher.command();
    command.env("DICTA_BIN", &dicta_bin);
    if let Some(config) = configured_provider_config_path(cli) {
        command.env("DICTA_PROVIDER_CONFIG", config);
    }
    if let Some(state) = provider_state_path(cli) {
        command.env("DICTA_PROVIDER_STATE", state);
    }
    command.env("DICTA_UI_LIVE_ARGS", live_args_for_ui(cli).join("\n"));
    command.env("DICTA_UI_AUTOSTART", if cli.live { "1" } else { "0" });
    command
        .status()
        .await
        .with_context(|| format!("failed to launch tray UI with {description}"))
        .and_then(|status| {
            if status.success() {
                Ok(())
            } else {
                bail!("tray UI exited with status {status}")
            }
        })
}

#[derive(Debug, Clone)]
enum TrayLauncher {
    Binary(PathBuf),
    CargoRun { repo_root: PathBuf },
}

impl TrayLauncher {
    fn command(&self) -> tokio::process::Command {
        match self {
            Self::Binary(path) => tokio::process::Command::new(path),
            Self::CargoRun { repo_root } => {
                let mut command = tokio::process::Command::new("cargo");
                command
                    .arg("run")
                    .arg("-p")
                    .arg("dicta-tray")
                    .arg("--bin")
                    .arg("dicta-tray")
                    .arg("--")
                    .current_dir(repo_root);
                command
            }
        }
    }

    fn description(&self) -> String {
        match self {
            Self::Binary(path) => path.display().to_string(),
            Self::CargoRun { repo_root } => {
                format!("cargo run -p dicta-tray in {}", repo_root.display())
            }
        }
    }
}

fn resolve_tray_launcher() -> Option<TrayLauncher> {
    let exe = std::env::current_exe().ok()?;
    let exe_dir = exe.parent()?;
    let tray_binary = exe_dir.join(tray_binary_name());
    if tray_binary.is_file() {
        return Some(TrayLauncher::Binary(tray_binary));
    }

    if let Some(repo_root) = find_repo_root_from(exe_dir) {
        let target_binary = repo_root
            .join("target")
            .join(debug_profile_dir())
            .join(tray_binary_name());
        if target_binary.is_file() {
            return Some(TrayLauncher::Binary(target_binary));
        }
        if repo_root.join("crates/dicta-tray/Cargo.toml").is_file() {
            return Some(TrayLauncher::CargoRun { repo_root });
        }
    }

    find_executable_in_path(tray_binary_name()).map(TrayLauncher::Binary)
}

fn find_repo_root_from(start: &std::path::Path) -> Option<PathBuf> {
    let mut current = Some(start);
    while let Some(dir) = current {
        if dir.join("crates/dicta-tray/Cargo.toml").is_file() && dir.join("Cargo.toml").is_file() {
            return Some(dir.to_path_buf());
        }
        current = dir.parent();
    }
    None
}

fn find_executable_in_path(name: &str) -> Option<PathBuf> {
    let paths = env::var_os("PATH")?;
    env::split_paths(&paths).find_map(|dir| {
        let candidate = dir.join(name);
        candidate.is_file().then_some(candidate)
    })
}

fn tray_binary_name() -> &'static str {
    if cfg!(windows) {
        "dicta-tray.exe"
    } else {
        "dicta-tray"
    }
}

fn debug_profile_dir() -> &'static str {
    if cfg!(debug_assertions) {
        "debug"
    } else {
        "release"
    }
}

fn live_args_for_ui(cli: &Cli) -> Vec<String> {
    let mut args = Vec::new();
    args.push("--provider".to_owned());
    args.push("active".to_owned());
    args.push("--live".to_owned());
    if let Some(src) = non_empty(&cli.src) {
        args.push("--src".to_owned());
        args.push(src);
    }
    if let Some(dst) = non_empty(&cli.dst) {
        args.push("--dst".to_owned());
        args.push(dst);
    }
    if let Some(chunk) = cli.live_chunk {
        args.push("--live-chunk".to_owned());
        args.push(chunk.to_string());
    }
    if cli.no_mic {
        args.push("--no-mic".to_owned());
    }
    if cli.no_speaker {
        args.push("--no-speaker".to_owned());
    }
    if cli.voice_processing {
        args.push("--voice-processing".to_owned());
    }
    if cli.select_device {
        args.push("--select-device".to_owned());
    }
    if cli.json {
        args.push("--json".to_owned());
    }
    if let Some(path) = &cli.transcript {
        args.push("--transcript".to_owned());
        args.push(path.display().to_string());
    }
    if let Some(path) = &cli.native_adapter {
        args.push("--native-adapter".to_owned());
        args.push(path.display().to_string());
    }
    args
}

fn should_run_live(cli: &Cli) -> bool {
    cli.live || (cli.input.is_none() && cli.mic_duration.is_none())
}

fn validate_batch_options(cli: &Cli) -> Result<()> {
    if cli.dst.is_some() {
        bail!("--dst is only supported with --live Apple mode");
    }
    if cli.no_mic || cli.no_speaker || cli.voice_processing || cli.select_device {
        bail!("--no-mic, --no-speaker, --voice-processing, and --select-device require --live");
    }
    Ok(())
}

async fn run_live(cli: &Cli) -> Result<()> {
    let backend = resolve_live_backend(cli)?;

    match backend {
        AsrBackend::Apple => {
            let provider = build_native_adapter(cli)?;
            run_live_provider(cli, &provider).await
        }
        AsrBackend::External => {
            let provider = build_external_provider(cli)?;
            run_live_provider(cli, &provider).await
        }
        AsrBackend::OpenaiCompatible | AsrBackend::Auto => {
            bail!(
                "interactive live mode requires Apple on-device ASR or an installed live provider"
            )
        }
    }
}

fn validate_live_options(
    cli: &Cli,
    provider_name: &str,
    capabilities: &LiveCapabilities,
) -> Result<()> {
    if cli.input.is_some() || cli.mic_duration.is_some() {
        bail!("--live cannot be combined with --input or --mic-duration");
    }
    if cli.no_mic && cli.no_speaker {
        bail!("--live cannot disable both --no-mic and --no-speaker");
    }
    if let Some(seconds) = cli.live_chunk {
        if !seconds.is_finite() || seconds <= 0.0 {
            bail!("--live-chunk must be greater than zero seconds");
        }
        if capabilities.mode != LiveModeKind::Chunked {
            bail!("--live-chunk requires a chunked live provider");
        }
    }
    if let Some(path) = &cli.transcript {
        validate_transcript_path(path)?;
    }
    if cli.dst.is_some() && !capabilities.translation {
        bail!("--dst requires a live provider with translation support");
    }
    if cli.no_mic && !capabilities.speaker {
        bail!(
            "{provider_name} live mode requires microphone input; speaker capture is not supported"
        );
    }
    if cli.voice_processing && !capabilities.voice_processing {
        bail!("--voice-processing is not supported by {provider_name} live mode");
    }
    if cli.select_device && !capabilities.device_selection {
        bail!("--select-device is not supported by {provider_name} live mode");
    }
    Ok(())
}

fn resolve_live_backend(cli: &Cli) -> Result<AsrBackend> {
    let support = apple_support();
    resolve_live_backend_for(cli, &support)
}

fn resolve_live_backend_for(cli: &Cli, apple_support: &AppleSupport) -> Result<AsrBackend> {
    if let Some(profile) = resolve_provider_profile_for(cli, apple_support)? {
        let effective = effective_provider_for(cli, apple_support, true)?;
        if effective.capabilities.live.is_some() {
            return Ok(effective.backend);
        }
        bail!(
            "provider profile '{}' ({}) does not support live mode; use --input or --mic-duration for batch transcription",
            profile.name,
            profile.profile.kind.as_str()
        );
    }

    match cli.asr {
        AsrBackend::Auto | AsrBackend::Apple => {
            if apple_support.supported {
                Ok(AsrBackend::Apple)
            } else {
                bail!(
                    "interactive live mode requires Apple on-device ASR or an installed live provider: {}",
                    apple_support.reason
                )
            }
        }
        AsrBackend::External => unreachable!("external backend is selected through --provider"),
        AsrBackend::OpenaiCompatible => {
            bail!("interactive live mode requires --asr apple or an installed live provider")
        }
    }
}

async fn run_live_provider<P>(cli: &Cli, provider: &P) -> Result<()>
where
    P: LiveAsrProvider,
{
    let capabilities = provider.live_capabilities();
    let provider_name = provider.live_name();
    validate_live_options(cli, provider_name, &capabilities)?;
    let options = live_options_from_cli(cli, &capabilities);
    let mut renderer = LiveRenderer::new(
        cli.json,
        cli.transcript.clone(),
        options.mic && options.speaker,
        cli.src.clone(),
        cli.dst.clone(),
    )?;

    provider
        .run_live(options, &mut |event| {
            renderer.handle_live_event(event).map_err(|err| {
                dicta_asr::AsrError::Request(format!(
                    "failed to render {provider_name} live event: {err}"
                ))
            })
        })
        .await
        .with_context(|| format!("{provider_name} live transcription failed"))?;

    renderer.finalize_session_log()?;
    renderer.print_summary();
    Ok(())
}

fn live_options_from_cli(cli: &Cli, capabilities: &LiveCapabilities) -> LiveAsrOptions {
    LiveAsrOptions {
        src: cli.src.clone(),
        dst: cli.dst.clone().filter(|_| capabilities.translation),
        mic: !cli.no_mic && capabilities.mic,
        speaker: !cli.no_speaker && capabilities.speaker,
        voice_processing: cli.voice_processing && capabilities.voice_processing,
        select_device: cli.select_device && capabilities.device_selection,
        chunk_duration: cli
            .live_chunk
            .map(Duration::from_secs_f64)
            .or(capabilities.expected_latency)
            .unwrap_or_else(|| Duration::from_secs(5)),
    }
}

struct OutputPayload {
    text: String,
    event: TranscriptEvent,
}

impl OutputPayload {
    fn new(
        transcript: dicta_asr::Transcript,
        channel: AudioChannel,
        src_hint: Option<String>,
    ) -> Self {
        let lang = transcript
            .language
            .clone()
            .or(src_hint)
            .unwrap_or_else(|| "und".to_owned());
        let text = transcript.text;
        let event = TranscriptEvent {
            seq: 0,
            channel,
            timestamp: EventTimestamp::now(),
            audio: None,
            src: TranscriptSource {
                lang,
                text: text.clone(),
                confidence: None,
            },
            dst: None,
        };

        Self { text, event }
    }

    fn jsonl(&self) -> Result<String> {
        Ok(self.event.jsonl()?)
    }
}

struct LiveRenderer {
    json_mode: bool,
    banner_printed: bool,
    backend: String,
    show_channel_label: bool,
    src_lang: String,
    dst_lang: Option<String>,
    session_log: Option<LiveSessionLog>,
    count: u64,
    started_at: SystemTime,
    pending: Vec<TranscriptEvent>,
    status: Option<LiveStatusEvent>,
    volatile: Vec<LiveVolatileEvent>,
    live_region_lines: usize,
}

impl LiveRenderer {
    fn new(
        json_forced: bool,
        transcript: Option<PathBuf>,
        show_channel_label: bool,
        src: Option<String>,
        dst: Option<String>,
    ) -> Result<Self> {
        let json_mode = json_forced || !std::io::stdout().is_terminal();
        let session_log = LiveSessionLog::open(transcript, json_mode)?;
        Ok(Self {
            json_mode,
            banner_printed: false,
            backend: String::new(),
            show_channel_label,
            src_lang: src.unwrap_or_else(|| "und".to_owned()),
            dst_lang: dst,
            session_log,
            count: 0,
            started_at: SystemTime::now(),
            pending: Vec::new(),
            status: None,
            volatile: Vec::new(),
            live_region_lines: 0,
        })
    }

    fn print_banner_header(&mut self, backend: &str, mic: bool, speaker: bool, translating: bool) {
        self.backend = backend.to_owned();
        let channels = [mic.then_some("mic"), speaker.then_some("speaker")]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(" + ");
        let langs = if let Some(dst) = &self.dst_lang {
            format!("{} -> {dst}", self.src_lang)
        } else {
            self.src_lang.clone()
        };
        let provider = match backend {
            "apple" => String::new(),
            other => format!(" via {other}"),
        };
        let suffix = if translating { " translation" } else { "" };
        println!(
            "dicta {} - listening on {channels}{provider}{suffix} ({langs})",
            env!("CARGO_PKG_VERSION")
        );
        self.banner_printed = true;
    }

    fn print_meta_banner(&mut self, meta: &LiveMetaEvent) {
        if self.json_mode || self.banner_printed {
            return;
        }
        self.print_banner_header(&meta.backend, meta.mic, meta.speaker, meta.dst.is_some());
        let label_width = if meta.mic && meta.speaker {
            "speaker".len()
        } else {
            0
        };
        for device in &meta.devices {
            let channel = match device.channel {
                AudioChannel::Mic => "mic",
                AudioChannel::Speaker => "speaker",
                AudioChannel::File => "file",
            };
            let note = if device.pinned {
                "[pinned]"
            } else {
                "(default)"
            };
            println!(
                "  {}  {} {}",
                ansi256(
                    channel_color(device.channel),
                    &format!("{channel:<label_width$}")
                ),
                device.name,
                ansi256(244, note)
            );
        }
        println!();
    }

    #[cfg(test)]
    fn emit_event(&mut self, event: &TranscriptEvent) -> Result<()> {
        self.handle_live_event(LiveEvent::Finalized(event.clone()))
    }

    fn handle_live_event(&mut self, event: LiveEvent) -> Result<()> {
        match event {
            LiveEvent::Meta(meta) => {
                self.apply_meta(meta);
                Ok(())
            }
            LiveEvent::Status(status) => {
                if self.json_mode && matches!(status.phase, LiveStatusPhase::Recovering) {
                    eprintln!("dicta: {}", status_text(&status));
                }
                self.set_status(status);
                self.redraw_live_region();
                Ok(())
            }
            LiveEvent::Volatile(volatile) => {
                self.set_volatile(volatile);
                self.redraw_live_region();
                Ok(())
            }
            LiveEvent::Finalized(event) => {
                self.status = None;
                self.clear_volatile(event.channel);
                if event.dst.is_some() || self.dst_lang.is_none() {
                    self.commit_event(event)
                } else {
                    self.pending.push(event);
                    self.redraw_live_region();
                    Ok(())
                }
            }
            LiveEvent::Translated(translated) => {
                self.status = None;
                self.apply_translation(translated)
            }
            LiveEvent::Eof => {
                self.status = None;
                self.drain_pending_without_translation()?;
                self.clear_live_region();
                Ok(())
            }
        }
    }

    fn commit_event(&mut self, event: TranscriptEvent) -> Result<()> {
        let jsonl = event.jsonl()?;
        if let Some(session_log) = self.session_log.as_mut() {
            session_log.append(&jsonl)?;
        }

        if self.json_mode {
            println!("{jsonl}");
        } else {
            self.clear_live_region();
            for line in self.tty_lines(event) {
                println!("{line}");
            }
            self.redraw_live_region();
        }
        self.count += 1;
        Ok(())
    }

    fn apply_meta(&mut self, meta: LiveMetaEvent) {
        self.backend = meta.backend.clone();
        self.src_lang = meta.src.clone();
        self.dst_lang = meta.dst.clone();
        self.show_channel_label = meta.mic && meta.speaker;
        self.print_meta_banner(&meta);
    }

    fn set_volatile(&mut self, volatile: LiveVolatileEvent) {
        if let Some(existing) = self
            .volatile
            .iter_mut()
            .find(|entry| entry.channel == volatile.channel)
        {
            *existing = volatile;
        } else {
            self.volatile.push(volatile);
        }
    }

    fn clear_volatile(&mut self, channel: AudioChannel) {
        self.volatile.retain(|entry| entry.channel != channel);
    }

    fn set_status(&mut self, status: LiveStatusEvent) {
        self.status = Some(status);
    }

    fn apply_translation(&mut self, translated: LiveTranslatedEvent) -> Result<()> {
        if let Some(index) = self
            .pending
            .iter()
            .position(|event| event.seq == translated.seq)
        {
            let mut event = self.pending.remove(index);
            event.dst = Some(TranscriptTarget {
                lang: translated
                    .lang
                    .or_else(|| self.dst_lang.clone())
                    .unwrap_or_else(|| "und".to_owned()),
                text: translated.text,
            });
            self.commit_event(event)?;
        }
        self.redraw_live_region();
        Ok(())
    }

    fn drain_pending_without_translation(&mut self) -> Result<()> {
        let pending = std::mem::take(&mut self.pending);
        for event in pending {
            self.commit_event(event)?;
        }
        Ok(())
    }

    fn tty_lines(&self, event: TranscriptEvent) -> Vec<String> {
        let header = self.tty_header(&event);
        let mut lines = vec![format!("{header}{}", event.src.text)];
        if let Some(dst) = &event.dst {
            if !is_passthrough_translation(&event.src.lang, &dst.lang, &event.src.text, &dst.text) {
                let pad = self.source_column_pad();
                lines.push(format!("{:pad$}{}", "", ansi256(244, &dst.text), pad = pad));
            }
        }
        lines
    }

    fn tty_header(&self, event: &TranscriptEvent) -> String {
        let timestamp = event.timestamp.format_local("%H:%M:%S");
        let color = channel_color(event.channel);
        let timestamp = ansi256(color, &timestamp);
        if self.show_channel_label {
            format!(
                "{timestamp} {}  ",
                ansi256(color, channel_label(event.channel))
            )
        } else {
            format!("{timestamp}   ")
        }
    }

    fn live_region_lines(&self) -> Vec<String> {
        if self.json_mode {
            return Vec::new();
        }
        let mut lines = Vec::new();
        for event in &self.pending {
            lines.extend(self.tty_lines(event.clone()));
            let pad = self.source_column_pad();
            lines.push(format!(
                "{:pad$}{}",
                "",
                ansi256(240, "(translating...)"),
                pad = pad
            ));
        }
        if let Some(status) = &self.status {
            let pad = self.source_column_pad();
            lines.push(format!(
                "{:pad$}{}",
                "",
                ansi256(244, &status_text(status)),
                pad = pad
            ));
        }
        for volatile in &self.volatile {
            if volatile.text.trim().is_empty() {
                continue;
            }
            let pad = if self.show_channel_label {
                self.channel_column_pad()
            } else {
                self.source_column_pad()
            };
            let label = if self.show_channel_label {
                format!(
                    "{}  ",
                    ansi256(
                        channel_color(volatile.channel),
                        channel_label(volatile.channel)
                    )
                )
            } else {
                String::new()
            };
            lines.push(format!(
                "{:pad$}{}{}{}",
                "",
                label,
                ansi256(240, "... "),
                ansi256(244, &volatile.text),
                pad = pad
            ));
        }
        lines
    }

    fn redraw_live_region(&mut self) {
        if self.json_mode {
            return;
        }
        self.clear_live_region();
        let lines = self.live_region_lines();
        let width = terminal_width();
        for line in &lines {
            println!("{line}");
        }
        self.live_region_lines = lines.iter().map(|line| physical_rows(line, width)).sum();
    }

    fn clear_live_region(&mut self) {
        if self.json_mode || self.live_region_lines == 0 {
            return;
        }
        for _ in 0..self.live_region_lines {
            print!("\r\x1b[A\x1b[2K");
        }
        let _ = io::stdout().flush();
        self.live_region_lines = 0;
    }

    fn source_column_pad(&self) -> usize {
        if self.show_channel_label { 16 } else { 11 }
    }

    fn channel_column_pad(&self) -> usize {
        9
    }

    fn print_summary(&self) {
        if self.json_mode {
            return;
        }
        let elapsed = self
            .started_at
            .elapsed()
            .unwrap_or_else(|_| Duration::from_secs(0));
        let mins = elapsed.as_secs() / 60;
        let secs = elapsed.as_secs() % 60;
        let noun = if self.count == 1 {
            "utterance"
        } else {
            "utterances"
        };
        println!();
        println!("{} {noun} in {mins}m {secs}s", self.count);
    }

    fn finalize_session_log(&mut self) -> Result<()> {
        let Some(session_log) = self.session_log.take() else {
            return Ok(());
        };
        if let Some(status) = session_log.finish(self.json_mode)? {
            if self.json_mode {
                eprintln!("{status}");
            } else {
                println!("{status}");
            }
        }
        Ok(())
    }
}

struct LiveSessionLog {
    path: PathBuf,
    suggested_path: PathBuf,
    explicit: bool,
    file: std::fs::File,
    has_content: bool,
}

impl LiveSessionLog {
    fn open(explicit_path: Option<PathBuf>, json_mode: bool) -> Result<Option<Self>> {
        if let Some(path) = explicit_path {
            validate_transcript_path(&path)?;
            if path.is_dir() {
                bail!("transcript path is a directory: {}", path.display());
            }
            if path.exists() && can_prompt_for_log() {
                if !confirm_overwrite(&path)? {
                    bail!("aborted: {} already exists", path.display());
                }
            }
            let file = OpenOptions::new()
                .create(true)
                .truncate(true)
                .write(true)
                .open(&path)
                .with_context(|| format!("failed to open transcript {}", path.display()))?;
            return Ok(Some(Self {
                path: path.clone(),
                suggested_path: path,
                explicit: true,
                file,
                has_content: false,
            }));
        }

        if json_mode || !can_prompt_for_log() {
            return Ok(None);
        }

        let path = std::env::temp_dir().join(format!(
            "dicta-{}-{}.jsonl",
            transcript_stamp(),
            std::process::id()
        ));
        let file = OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(&path)
            .with_context(|| format!("failed to open temporary transcript {}", path.display()))?;
        Ok(Some(Self {
            path,
            suggested_path: PathBuf::from(format!("./dicta-{}.jsonl", transcript_stamp())),
            explicit: false,
            file,
            has_content: false,
        }))
    }

    fn append(&mut self, jsonl: &str) -> Result<()> {
        writeln!(self.file, "{jsonl}")?;
        self.file.flush()?;
        self.has_content = true;
        Ok(())
    }

    fn finish(mut self, json_mode: bool) -> Result<Option<String>> {
        self.file.flush()?;
        drop(self.file);

        if self.explicit {
            if self.has_content {
                return Ok(Some(format!("Saved transcript: {}", self.path.display())));
            }
            return Ok(Some(format!(
                "Saved transcript: {} (no utterances)",
                self.path.display()
            )));
        }

        if !self.has_content {
            let _ = std::fs::remove_file(&self.path);
            return Ok(None);
        }

        if json_mode || !can_prompt_for_log() {
            let _ = std::fs::remove_file(&self.path);
            return Ok(None);
        }

        loop {
            let Some(target) = prompt_for_log_path(&self.suggested_path)? else {
                let _ = std::fs::remove_file(&self.path);
                return Ok(None);
            };
            if target.is_dir() {
                println!(
                    "{} is a directory; choose a file path instead.",
                    target.display()
                );
                continue;
            }
            if target.exists() && !confirm_overwrite(&target)? {
                continue;
            }
            if let Some(parent) = target.parent() {
                if !parent.as_os_str().is_empty() && !parent.exists() {
                    println!(
                        "Failed to save transcript to {}: parent directory does not exist",
                        target.display()
                    );
                    continue;
                }
            }
            std::fs::rename(&self.path, &target).with_context(|| {
                format!(
                    "failed to save transcript from {} to {}",
                    self.path.display(),
                    target.display()
                )
            })?;
            return Ok(Some(format!("Saved transcript: {}", target.display())));
        }
    }
}

fn transcript_stamp() -> String {
    EventTimestamp::local_stamp_now()
}

fn can_prompt_for_log() -> bool {
    std::io::stdin().is_terminal() && std::io::stdout().is_terminal()
}

fn confirm_overwrite(path: &PathBuf) -> Result<bool> {
    print!("{} already exists. Overwrite? [y/N]: ", path.display());
    io::stdout().flush()?;
    let mut line = String::new();
    io::stdin().read_line(&mut line)?;
    let trimmed = line.trim().to_ascii_lowercase();
    Ok(trimmed == "y" || trimmed == "yes")
}

fn prompt_for_log_path(default_path: &PathBuf) -> Result<Option<PathBuf>> {
    println!();
    print!(
        "Save transcript to {}? [Y/n/<path>]: ",
        default_path.display()
    );
    io::stdout().flush()?;
    let mut line = String::new();
    io::stdin().read_line(&mut line)?;
    let trimmed = line.trim();
    if trimmed.is_empty()
        || trimmed.eq_ignore_ascii_case("y")
        || trimmed.eq_ignore_ascii_case("yes")
    {
        return Ok(Some(default_path.clone()));
    }
    if trimmed.eq_ignore_ascii_case("n") || trimmed.eq_ignore_ascii_case("no") {
        return Ok(None);
    }
    Ok(Some(PathBuf::from(trimmed)))
}

fn terminal_width() -> usize {
    env::var("COLUMNS")
        .ok()
        .and_then(|value| value.parse().ok())
        .filter(|width| *width > 0)
        .unwrap_or(80)
}

fn physical_rows(line: &str, terminal_width: usize) -> usize {
    let terminal_width = cmp::max(1, terminal_width);
    let width = display_width(&strip_ansi(line));
    cmp::max(1, width.div_ceil(terminal_width))
}

fn strip_ansi(line: &str) -> String {
    let mut output = String::new();
    let mut chars = line.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\x1b' && chars.peek() == Some(&'[') {
            let _ = chars.next();
            for next in chars.by_ref() {
                if ('@'..='~').contains(&next) {
                    break;
                }
            }
            continue;
        }
        output.push(ch);
    }
    output
}

fn display_width(text: &str) -> usize {
    text.chars()
        .map(|ch| if is_wide_char(ch) { 2 } else { 1 })
        .sum()
}

fn is_wide_char(ch: char) -> bool {
    let value = ch as u32;
    (0x1100..=0x115F).contains(&value)
        || (0x2E80..=0x303E).contains(&value)
        || (0x3041..=0x33FF).contains(&value)
        || (0x3400..=0x4DBF).contains(&value)
        || (0x4E00..=0x9FFF).contains(&value)
        || (0xA000..=0xA4CF).contains(&value)
        || (0xAC00..=0xD7A3).contains(&value)
        || (0xF900..=0xFAFF).contains(&value)
        || (0xFE30..=0xFE4F).contains(&value)
        || (0xFF00..=0xFF60).contains(&value)
        || (0xFFE0..=0xFFE6).contains(&value)
        || (0x1F300..=0x1F6FF).contains(&value)
        || (0x1F900..=0x1F9FF).contains(&value)
}

fn ansi256(code: u8, text: &str) -> String {
    format!("\x1b[38;5;{code}m{text}\x1b[0m")
}

fn channel_color(channel: AudioChannel) -> u8 {
    match channel {
        AudioChannel::Mic => 130,
        AudioChannel::Speaker => 24,
        AudioChannel::File => 67,
    }
}

fn channel_label(channel: AudioChannel) -> &'static str {
    match channel {
        AudioChannel::Mic => "[mic]",
        AudioChannel::Speaker => "[spk]",
        AudioChannel::File => "[file]",
    }
}

fn status_text(status: &LiveStatusEvent) -> String {
    match &status.detail {
        Some(detail) if !detail.trim().is_empty() => {
            format!("{}: {}", status.message, detail.trim())
        }
        _ => status.message.clone(),
    }
}

fn is_passthrough_translation(
    src_lang: &str,
    dst_lang: &str,
    src_text: &str,
    dst_text: &str,
) -> bool {
    src_text == dst_text && primary_language_subtag(src_lang) == primary_language_subtag(dst_lang)
}

fn primary_language_subtag(lang: &str) -> &str {
    lang.split(['-', '_'])
        .next()
        .filter(|part| !part.is_empty())
        .unwrap_or(lang)
}

fn write_transcript(path: &PathBuf, payload: &OutputPayload, json: bool) -> Result<()> {
    validate_transcript_path(path)?;
    let mut content = if json {
        payload.jsonl()?
    } else {
        payload.text.clone()
    };
    content.push('\n');
    std::fs::write(path, content)
        .with_context(|| format!("failed to write transcript to {}", path.display()))
}

fn validate_transcript_path(path: &PathBuf) -> Result<()> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() && !parent.exists() {
            bail!("transcript directory does not exist: {}", parent.display());
        }
    }
    Ok(())
}

#[derive(Debug, Serialize)]
struct DoctorReport {
    system: SystemReport,
    backend: BackendReport,
    capabilities: CapabilitiesReport,
    live: LiveReport,
    audio: AudioReport,
    runtime: RuntimeReport,
}

#[derive(Debug, Serialize)]
struct SystemReport {
    os: &'static str,
    arch: &'static str,
    macos_version: Option<String>,
    apple_on_device_supported: bool,
    apple_on_device_reason: String,
}

#[derive(Debug, Serialize)]
struct BackendReport {
    requested: String,
    resolved: Option<String>,
    provider: Option<String>,
    provider_kind: Option<String>,
    provider_config: Option<String>,
    api_base_configured: bool,
    api_key_configured: bool,
    model: String,
    native_adapter: Option<String>,
    error: Option<String>,
}

#[derive(Debug, Serialize)]
struct LiveReport {
    backend: Option<String>,
    mode: Option<String>,
    mic: bool,
    speaker: bool,
    streaming_audio: bool,
    partial_results: bool,
    finalized_results: bool,
    translation: bool,
    voice_processing: bool,
    device_selection: bool,
    requires_network: bool,
    expected_latency: Option<String>,
    error: Option<String>,
}

#[derive(Debug, Serialize)]
struct CapabilitiesReport {
    requested: String,
    resolved: Option<String>,
    provider: Option<String>,
    provider_kind: Option<String>,
    provider_config: Option<String>,
    model: String,
    local_config_ok: bool,
    local_config_error: Option<String>,
    resolution_error: Option<String>,
    batch: Option<BatchCapabilitiesReport>,
    live: Option<LiveCapabilitiesReport>,
    notes: Vec<String>,
}

#[derive(Debug, Serialize)]
struct BatchCapabilitiesReport {
    batch_file: bool,
    streaming: bool,
    requires_network: bool,
}

#[derive(Debug, Serialize)]
struct LiveCapabilitiesReport {
    mode: String,
    mic: bool,
    speaker: bool,
    streaming_audio: bool,
    partial_results: bool,
    finalized_results: bool,
    translation: bool,
    voice_processing: bool,
    device_selection: bool,
    requires_network: bool,
    expected_latency: Option<String>,
}

#[derive(Debug, Serialize)]
struct AudioReport {
    default_input_available: bool,
    name: Option<String>,
    sample_rate: Option<u32>,
    channels: Option<u16>,
    sample_format: Option<String>,
    error: Option<String>,
}

#[derive(Debug, Serialize)]
struct RuntimeReport {
    python_sidecar_required: bool,
    web_direct_available: bool,
    native_adapter_supported_os: bool,
}

fn run_doctor(cli: &Cli) -> Result<()> {
    let report = gather_doctor_report(cli);
    if cli.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print_doctor_text(&report);
    }
    Ok(())
}

fn run_capabilities(cli: &Cli) -> Result<()> {
    let report = gather_capabilities_report(cli);
    if cli.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print_capabilities_text(&report);
    }
    Ok(())
}

async fn run_provider_command(cli: &Cli, command: &ProviderCommand) -> Result<()> {
    match &command.action {
        ProviderAction::List => run_provider_list(cli),
        ProviderAction::Available(command) => run_provider_available(cli, command).await,
        ProviderAction::Current => run_provider_current(cli),
        ProviderAction::Set { name } => run_provider_set(cli, name),
        ProviderAction::Install(command) => run_provider_install(cli, command).await,
        ProviderAction::Update(command) => run_provider_update(cli, command).await,
        ProviderAction::Remove(command) => run_provider_remove(cli, command),
    }
}

fn run_provider_list(cli: &Cli) -> Result<()> {
    let report = gather_provider_list_report(cli)?;
    if cli.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print_provider_list_text(&report);
    }
    Ok(())
}

fn run_provider_current(cli: &Cli) -> Result<()> {
    let report = gather_current_provider_report(cli)?;
    if cli.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print_current_provider_text(&report);
    }
    Ok(())
}

fn run_provider_set(cli: &Cli, name: &str) -> Result<()> {
    let name = name.trim();
    if name.is_empty() || name == "active" {
        bail!("provider name must be a concrete built-in or configured provider");
    }
    let profiles = available_provider_profiles(cli)?;
    if !profiles.contains_key(name) {
        bail!(
            "provider '{name}' was not found; run `dicta provider list` to see available providers"
        );
    }
    write_active_provider_name(cli, name)?;
    if cli.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&gather_current_provider_report(cli)?)?
        );
    } else {
        println!("Active provider: {name}");
    }
    Ok(())
}

#[derive(Debug, Serialize)]
struct ProviderInstallReport {
    id: String,
    name: Option<String>,
    version: Option<String>,
    protocol: String,
    source: String,
    source_package: Option<String>,
    install_path: String,
    command: String,
}

#[derive(Debug, Serialize)]
struct ProviderAvailableReport {
    registry: String,
    scope: String,
    keyword: String,
    packages: Vec<ProviderAvailableEntry>,
}

#[derive(Debug, Serialize)]
struct ProviderAvailableEntry {
    package: String,
    provider_id: String,
    latest_version: String,
    description: Option<String>,
    installed: bool,
    installed_version: Option<String>,
    update_available: bool,
}

#[derive(Debug, Serialize)]
struct ProviderUpdateReport {
    registry: String,
    target: Option<String>,
    providers: Vec<ProviderUpdateEntry>,
}

#[derive(Debug, Serialize)]
struct ProviderUpdateEntry {
    id: String,
    package: Option<String>,
    previous_version: Option<String>,
    latest_version: Option<String>,
    changed: bool,
    skipped: bool,
    install_path: Option<String>,
    message: String,
}

#[derive(Debug, Serialize)]
struct ProviderRemoveReport {
    id: String,
    install_path: String,
    active_provider_cleared: bool,
}

async fn run_provider_install(cli: &Cli, command: &ProviderInstallCommand) -> Result<()> {
    let source = ProviderInstallSource::resolve(command).await?;
    let report = install_provider_from_source(cli, source, command.force)?;

    if cli.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print_provider_install_report(&report);
    }

    Ok(())
}

fn install_provider_from_source(
    cli: &Cli,
    source: ProviderInstallSource,
    force: bool,
) -> Result<ProviderInstallReport> {
    let providers_dir = provider_install_dir(cli)
        .context("could not determine provider install directory; set DICTA_PROVIDER_DIR")?;
    fs::create_dir_all(&providers_dir).with_context(|| {
        format!(
            "failed to create provider install directory {}",
            providers_dir.display()
        )
    })?;

    let staging = temp_provider_staging_dir(&providers_dir);
    if staging.exists() {
        fs::remove_dir_all(&staging).with_context(|| {
            format!(
                "failed to remove stale provider staging directory {}",
                staging.display()
            )
        })?;
    }
    fs::create_dir_all(&staging).with_context(|| {
        format!(
            "failed to create provider staging directory {}",
            staging.display()
        )
    })?;

    let install_result = install_provider_to_staging(&source, &staging).and_then(|manifest| {
        finish_provider_install(force, &providers_dir, &staging, manifest, &source.metadata)
    });
    if install_result.is_err() {
        let _ = fs::remove_dir_all(&staging);
    }
    let (installed, final_path) = install_result?;

    Ok(ProviderInstallReport {
        id: installed.id().to_owned(),
        name: installed.manifest.name.clone(),
        version: installed.manifest.version.clone(),
        protocol: installed.manifest.protocol.clone(),
        source: source.description,
        source_package: source.metadata.package.clone(),
        install_path: final_path.display().to_string(),
        command: installed.command_path().display().to_string(),
    })
}

fn print_provider_install_report(report: &ProviderInstallReport) {
    println!("Installed provider: {}", report.id);
    if let Some(name) = &report.name {
        println!("  Name: {name}");
    }
    if let Some(version) = &report.version {
        println!("  Version: {version}");
    }
    if let Some(package) = &report.source_package {
        println!("  Package: {package}");
    }
    println!("  Path: {}", report.install_path);
    println!("  Command: {}", report.command);
    println!("  Protocol: {}", report.protocol);
}

async fn run_provider_available(cli: &Cli, command: &ProviderAvailableCommand) -> Result<()> {
    let report = gather_provider_available_report(cli, command).await?;
    if cli.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print_provider_available_text(&report);
    }
    Ok(())
}

async fn gather_provider_available_report(
    cli: &Cli,
    command: &ProviderAvailableCommand,
) -> Result<ProviderAvailableReport> {
    let registry = command.registry.trim().trim_end_matches('/').to_owned();
    let scope = command.scope.trim().trim_start_matches('@').to_owned();
    let keyword = command.keyword.trim().to_owned();
    let packages = search_npm_provider_packages(&registry, &scope, &keyword, command.limit).await?;
    let installed = installed_providers(cli)?;
    let entries = packages
        .into_iter()
        .map(|package| {
            let provider_id = provider_id_from_package_name(&package.name).to_owned();
            let installed_provider = installed.get(&provider_id);
            let installed_version =
                installed_provider.and_then(|provider| provider.manifest.version.clone());
            let update_available = installed_version
                .as_ref()
                .is_some_and(|version| version != &package.version);
            ProviderAvailableEntry {
                package: package.name,
                provider_id,
                latest_version: package.version,
                description: package.description,
                installed: installed_provider.is_some(),
                installed_version,
                update_available,
            }
        })
        .collect();

    Ok(ProviderAvailableReport {
        registry,
        scope,
        keyword,
        packages: entries,
    })
}

fn print_provider_available_text(report: &ProviderAvailableReport) {
    println!("Installable providers");
    println!("  Registry: {}", report.registry);
    println!("  Scope: @{}", report.scope);
    println!();
    for package in &report.packages {
        let status = if package.update_available {
            "update available"
        } else if package.installed {
            "installed"
        } else {
            "not installed"
        };
        println!(
            "{} ({}, latest {}, {})",
            package.provider_id, package.package, package.latest_version, status
        );
        if let Some(version) = &package.installed_version {
            println!("    Installed version: {version}");
        }
        if let Some(description) = &package.description {
            println!("    {description}");
        }
    }
}

async fn run_provider_update(cli: &Cli, command: &ProviderUpdateCommand) -> Result<()> {
    let report = update_provider_packages(cli, command).await?;
    if cli.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print_provider_update_text(&report);
    }
    Ok(())
}

async fn update_provider_packages(
    cli: &Cli,
    command: &ProviderUpdateCommand,
) -> Result<ProviderUpdateReport> {
    let registry = command.registry.trim().trim_end_matches('/').to_owned();
    let installed = installed_providers(cli)?;
    let targets = provider_update_targets(&installed, command.name.as_deref())?;
    let mut entries = Vec::new();

    for provider in targets {
        let id = provider.id().to_owned();
        let package = provider_update_package(&provider);
        let previous_version = provider.manifest.version.clone();
        let Some(package) = package else {
            entries.push(ProviderUpdateEntry {
                id,
                package: None,
                previous_version,
                latest_version: None,
                changed: false,
                skipped: true,
                install_path: Some(provider.root.display().to_string()),
                message:
                    "provider was installed from a local path; no npm package metadata is available"
                        .to_owned(),
            });
            continue;
        };

        let update_registry = provider
            .install_metadata
            .as_ref()
            .and_then(|metadata| metadata.registry.clone())
            .unwrap_or_else(|| registry.clone());
        let latest_version =
            resolve_npm_package_version(&package, command.version.as_deref(), &update_registry)
                .await?;
        if !command.force && previous_version.as_deref() == Some(latest_version.as_str()) {
            entries.push(ProviderUpdateEntry {
                id,
                package: Some(package),
                previous_version,
                latest_version: Some(latest_version),
                changed: false,
                skipped: true,
                install_path: Some(provider.root.display().to_string()),
                message: "already up to date".to_owned(),
            });
            continue;
        }

        let source = ProviderInstallSource::resolve_npm(
            &package,
            command.version.as_deref(),
            &update_registry,
        )
        .await?;
        let install_report = install_provider_from_source(cli, source, true)?;
        entries.push(ProviderUpdateEntry {
            id: install_report.id,
            package: install_report.source_package,
            previous_version,
            latest_version: install_report.version,
            changed: true,
            skipped: false,
            install_path: Some(install_report.install_path),
            message: "updated".to_owned(),
        });
    }

    Ok(ProviderUpdateReport {
        registry,
        target: command.name.clone(),
        providers: entries,
    })
}

fn provider_update_targets(
    installed: &BTreeMap<String, InstalledProvider>,
    target: Option<&str>,
) -> Result<Vec<InstalledProvider>> {
    if let Some(target) = target.and_then(|value| non_empty_string(Some(value))) {
        return installed
            .values()
            .find(|provider| provider_matches_name_or_package(provider, target))
            .cloned()
            .map(|provider| vec![provider])
            .with_context(|| format!("installed provider '{target}' was not found"));
    }
    Ok(installed.values().cloned().collect())
}

fn provider_update_package(provider: &InstalledProvider) -> Option<String> {
    match provider.install_metadata.as_ref() {
        Some(metadata) if metadata.source == ProviderInstallSourceKind::Npm => metadata
            .package
            .clone()
            .or_else(|| Some(format!("@{DEFAULT_PROVIDER_SCOPE}/{}", provider.id()))),
        Some(_) => None,
        None => Some(format!("@{DEFAULT_PROVIDER_SCOPE}/{}", provider.id())),
    }
}

fn provider_matches_name_or_package(provider: &InstalledProvider, value: &str) -> bool {
    let value = value.trim();
    if value == provider.id() || provider_id_from_package_name(value) == provider.id() {
        return true;
    }
    let canonical = canonical_provider_package_name(value);
    provider.source_package().is_some_and(|package| {
        package == value || package == canonical || provider_id_from_package_name(package) == value
    })
}

fn print_provider_update_text(report: &ProviderUpdateReport) {
    println!("Provider updates");
    println!("  Registry: {}", report.registry);
    for provider in &report.providers {
        let marker = if provider.changed {
            "updated"
        } else if provider.skipped {
            "skipped"
        } else {
            "checked"
        };
        println!("  {}: {}", provider.id, marker);
        if let Some(package) = &provider.package {
            println!("    Package: {package}");
        }
        if let Some(previous) = &provider.previous_version {
            println!("    Previous: {previous}");
        }
        if let Some(latest) = &provider.latest_version {
            println!("    Latest: {latest}");
        }
        println!("    {}", provider.message);
    }
}

fn run_provider_remove(cli: &Cli, command: &ProviderRemoveCommand) -> Result<()> {
    let installed = installed_providers(cli)?;
    let provider = installed
        .values()
        .find(|provider| provider_matches_name_or_package(provider, &command.name))
        .cloned()
        .with_context(|| format!("installed provider '{}' was not found", command.name.trim()))?;

    if !command.yes && !confirm_provider_remove(&provider)? {
        println!("dicta provider remove: cancelled");
        return Ok(());
    }

    let active_provider_cleared = read_active_provider_name(cli)?.as_deref() == Some(provider.id());
    fs::remove_dir_all(&provider.root).with_context(|| {
        format!(
            "failed to remove provider directory {}",
            provider.root.display()
        )
    })?;
    if active_provider_cleared {
        clear_active_provider_name(cli)?;
    }

    let report = ProviderRemoveReport {
        id: provider.id().to_owned(),
        install_path: provider.root.display().to_string(),
        active_provider_cleared,
    };
    if cli.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!("Removed provider: {}", report.id);
        println!("  Path: {}", report.install_path);
        if report.active_provider_cleared {
            println!("  Active provider selection was cleared");
        }
    }
    Ok(())
}

fn confirm_provider_remove(provider: &InstalledProvider) -> Result<bool> {
    if !io::stdin().is_terminal() {
        bail!(
            "refusing to remove provider without confirmation on non-interactive stdin; pass --yes"
        );
    }
    eprint!(
        "Remove provider '{}' from {}? [y/N] ",
        provider.id(),
        provider.root.display()
    );
    io::stderr().flush().ok();
    let mut input = String::new();
    io::stdin()
        .read_line(&mut input)
        .context("failed to read provider removal confirmation")?;
    Ok(is_uninstall_confirmation(&input))
}

struct ProviderInstallSource {
    description: String,
    package: ProviderInstallPackage,
    metadata: ProviderInstallMetadata,
}

enum ProviderInstallPackage {
    Directory(PathBuf),
    Tgz(Vec<u8>),
}

impl ProviderInstallSource {
    async fn resolve(command: &ProviderInstallCommand) -> Result<Self> {
        let package = command.package.trim();
        if package.is_empty() {
            bail!("provider package is required");
        }

        let local = PathBuf::from(package);
        if local.is_dir() {
            return Ok(Self {
                description: local.display().to_string(),
                package: ProviderInstallPackage::Directory(local),
                metadata: provider_install_metadata(
                    ProviderInstallSourceKind::Directory,
                    None,
                    None,
                    None,
                ),
            });
        }
        if local.is_file() {
            let data = fs::read(&local)
                .with_context(|| format!("failed to read provider package {}", local.display()))?;
            return Ok(Self {
                description: local.display().to_string(),
                package: ProviderInstallPackage::Tgz(data),
                metadata: provider_install_metadata(
                    ProviderInstallSourceKind::Tarball,
                    None,
                    None,
                    None,
                ),
            });
        }

        Self::resolve_npm(package, command.version.as_deref(), &command.registry).await
    }

    async fn resolve_npm(package: &str, version: Option<&str>, registry: &str) -> Result<Self> {
        let npm = fetch_npm_package(package, version, registry).await?;
        Ok(Self {
            description: npm.description,
            package: ProviderInstallPackage::Tgz(npm.data),
            metadata: provider_install_metadata(
                ProviderInstallSourceKind::Npm,
                Some(npm.package_name),
                Some(registry.trim().trim_end_matches('/').to_owned()),
                Some(npm.version),
            ),
        })
    }
}

struct DownloadedNpmPackage {
    description: String,
    package_name: String,
    version: String,
    data: Vec<u8>,
}

#[derive(Debug, Deserialize)]
struct NpmPackageMetadata {
    name: String,
    #[serde(rename = "dist-tags", default)]
    dist_tags: BTreeMap<String, String>,
    #[serde(default)]
    versions: BTreeMap<String, NpmPackageVersion>,
}

#[derive(Debug, Deserialize)]
struct NpmSearchResponse {
    #[serde(default)]
    objects: Vec<NpmSearchObject>,
}

#[derive(Debug, Deserialize)]
struct NpmSearchObject {
    package: NpmSearchPackage,
}

#[derive(Debug, Deserialize)]
struct NpmSearchPackage {
    name: String,
    version: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    keywords: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct NpmPackageVersion {
    #[serde(default)]
    os: Vec<String>,
    #[serde(default)]
    cpu: Vec<String>,
    #[serde(rename = "optionalDependencies", default)]
    optional_dependencies: BTreeMap<String, String>,
    dist: NpmPackageDist,
}

#[derive(Debug, Deserialize)]
struct NpmPackageDist {
    tarball: String,
    integrity: Option<String>,
}

#[derive(Debug, Clone)]
struct NpmProviderPackageSummary {
    name: String,
    version: String,
    description: Option<String>,
}

async fn search_npm_provider_packages(
    registry: &str,
    scope: &str,
    keyword: &str,
    limit: usize,
) -> Result<Vec<NpmProviderPackageSummary>> {
    let registry = registry.trim().trim_end_matches('/');
    let scope_prefix = format!("@{}/", scope.trim().trim_start_matches('@'));
    let text = format!(
        "scope:{} keywords:{}",
        scope.trim().trim_start_matches('@'),
        keyword
    );
    let client = reqwest::Client::new();
    let search: NpmSearchResponse = client
        .get(format!("{registry}/-/v1/search"))
        .query(&[("text", text.as_str()), ("size", &limit.max(1).to_string())])
        .send()
        .await
        .with_context(|| "failed to query npm provider search")?
        .error_for_status()
        .with_context(|| "npm provider search returned an error")?
        .json()
        .await
        .with_context(|| "failed to parse npm provider search response")?;

    Ok(search
        .objects
        .into_iter()
        .filter_map(|object| {
            let package = object.package;
            (package.name.starts_with(&scope_prefix)
                && package.keywords.iter().any(|value| value == keyword))
            .then_some(NpmProviderPackageSummary {
                name: package.name,
                version: package.version,
                description: package.description,
            })
        })
        .collect())
}

async fn resolve_npm_package_version(
    package_name: &str,
    requested: Option<&str>,
    registry: &str,
) -> Result<String> {
    let registry = registry.trim().trim_end_matches('/');
    let client = reqwest::Client::new();
    let metadata = fetch_npm_metadata(
        &client,
        registry,
        &canonical_provider_package_name(package_name),
    )
    .await?;
    resolve_npm_version(&metadata, requested.unwrap_or("latest"))
}

async fn fetch_npm_package(
    package_name: &str,
    requested: Option<&str>,
    registry: &str,
) -> Result<DownloadedNpmPackage> {
    let registry = registry.trim().trim_end_matches('/');
    let requested_name = canonical_provider_package_name(package_name);
    let requested = requested.unwrap_or("latest");
    let client = reqwest::Client::new();
    let metadata = fetch_npm_metadata(&client, registry, &requested_name).await?;
    let version = resolve_npm_version(&metadata, requested)?;
    let package = metadata.versions.get(&version).with_context(|| {
        format!(
            "npm package '{}' metadata is missing version '{version}'",
            metadata.name
        )
    })?;

    if let Some(platform) =
        resolve_optional_platform_package(&client, registry, &metadata.name, package).await?
    {
        let platform_metadata = fetch_npm_metadata(&client, registry, &platform.name).await?;
        let platform_version = resolve_npm_version(&platform_metadata, &platform.version)?;
        let platform_package = platform_metadata
            .versions
            .get(&platform_version)
            .with_context(|| {
                format!(
                    "npm package '{}' metadata is missing version '{platform_version}'",
                    platform_metadata.name
                )
            })?;
        let data = download_npm_package(&client, platform_package).await?;
        return Ok(DownloadedNpmPackage {
            description: format!(
                "{}@{} -> {}@{}",
                metadata.name, version, platform_metadata.name, platform_version
            ),
            package_name: metadata.name,
            version,
            data,
        });
    }

    if !npm_package_version_matches_current_platform(package) {
        bail!(
            "npm package '{}@{}' does not support this platform ({} / {})",
            metadata.name,
            version,
            npm_current_os(),
            npm_current_cpu()
        );
    }

    let data = download_npm_package(&client, package).await?;
    Ok(DownloadedNpmPackage {
        description: format!("{}@{}", metadata.name, version),
        package_name: metadata.name,
        version,
        data,
    })
}

async fn fetch_npm_metadata(
    client: &reqwest::Client,
    registry: &str,
    name: &str,
) -> Result<NpmPackageMetadata> {
    let metadata_url = format!("{registry}/{}", npm_package_url_name(name));
    client
        .get(&metadata_url)
        .send()
        .await
        .with_context(|| format!("failed to query npm registry metadata: {metadata_url}"))?
        .error_for_status()
        .with_context(|| format!("npm registry returned an error for {metadata_url}"))?
        .json::<NpmPackageMetadata>()
        .await
        .with_context(|| format!("failed to parse npm registry metadata for {name}"))
}

fn resolve_npm_version(metadata: &NpmPackageMetadata, requested: &str) -> Result<String> {
    let requested = requested.trim();
    if requested == "*" {
        return metadata
            .dist_tags
            .get("latest")
            .cloned()
            .with_context(|| format!("npm package '{}' has no latest dist-tag", metadata.name));
    }
    let normalized = requested
        .strip_prefix('v')
        .or_else(|| requested.strip_prefix('='))
        .or_else(|| requested.strip_prefix('^'))
        .or_else(|| requested.strip_prefix('~'))
        .unwrap_or(requested);
    metadata
        .versions
        .get(normalized)
        .map(|_| normalized.to_owned())
        .or_else(|| metadata.dist_tags.get(requested).cloned())
        .with_context(|| {
            format!(
                "npm package '{}' has no version or dist-tag '{requested}'",
                metadata.name
            )
        })
}

struct OptionalPlatformPackage {
    name: String,
    version: String,
}

async fn resolve_optional_platform_package(
    client: &reqwest::Client,
    registry: &str,
    root_name: &str,
    package: &NpmPackageVersion,
) -> Result<Option<OptionalPlatformPackage>> {
    for (dependency, version) in &package.optional_dependencies {
        if !is_probable_provider_platform_package(root_name, dependency) {
            continue;
        }
        let metadata = fetch_npm_metadata(client, registry, dependency).await?;
        let resolved_version = resolve_npm_version(&metadata, version)?;
        let Some(candidate) = metadata.versions.get(&resolved_version) else {
            continue;
        };
        if npm_package_version_matches_current_platform(candidate) {
            return Ok(Some(OptionalPlatformPackage {
                name: metadata.name,
                version: resolved_version,
            }));
        }
    }
    Ok(None)
}

fn is_probable_provider_platform_package(root_name: &str, dependency: &str) -> bool {
    dependency.starts_with(&format!("{root_name}-"))
        || dependency
            .rsplit_once('/')
            .map(|(_, name)| {
                name.starts_with(&format!("{}-", provider_id_from_package_name(root_name)))
            })
            .unwrap_or(false)
}

async fn download_npm_package(
    client: &reqwest::Client,
    package: &NpmPackageVersion,
) -> Result<Vec<u8>> {
    let bytes = client
        .get(&package.dist.tarball)
        .send()
        .await
        .with_context(|| format!("failed to download npm tarball {}", package.dist.tarball))?
        .error_for_status()
        .with_context(|| {
            format!(
                "npm registry returned an error for {}",
                package.dist.tarball
            )
        })?
        .bytes()
        .await
        .with_context(|| format!("failed to read npm tarball {}", package.dist.tarball))?
        .to_vec();

    if let Some(integrity) = &package.dist.integrity {
        verify_npm_integrity(&bytes, integrity)?;
    }

    Ok(bytes)
}

fn canonical_provider_package_name(name: &str) -> String {
    let name = name.trim();
    if name.starts_with('@') {
        name.to_owned()
    } else {
        format!("@{DEFAULT_PROVIDER_SCOPE}/{name}")
    }
}

fn npm_package_url_name(name: &str) -> String {
    if let Some((scope, package)) = name.split_once('/') {
        format!("{}%2F{}", scope, package)
    } else {
        name.to_owned()
    }
}

fn provider_id_from_package_name(name: &str) -> &str {
    name.rsplit_once('/').map(|(_, name)| name).unwrap_or(name)
}

fn npm_package_version_matches_current_platform(package: &NpmPackageVersion) -> bool {
    npm_list_allows(&package.os, npm_current_os())
        && npm_list_allows(&package.cpu, npm_current_cpu())
}

fn npm_list_allows(values: &[String], current: &str) -> bool {
    if values.iter().any(|value| value == &format!("!{current}")) {
        return false;
    }
    let positives = values
        .iter()
        .filter(|value| !value.starts_with('!'))
        .collect::<Vec<_>>();
    positives.is_empty() || positives.iter().any(|value| value.as_str() == current)
}

fn npm_current_os() -> &'static str {
    if cfg!(target_os = "macos") {
        "darwin"
    } else if cfg!(windows) {
        "win32"
    } else {
        std::env::consts::OS
    }
}

fn npm_current_cpu() -> &'static str {
    match std::env::consts::ARCH {
        "aarch64" => "arm64",
        "x86_64" => "x64",
        "x86" => "ia32",
        other => other,
    }
}

fn provider_install_metadata(
    source: ProviderInstallSourceKind,
    package: Option<String>,
    registry: Option<String>,
    version: Option<String>,
) -> ProviderInstallMetadata {
    ProviderInstallMetadata {
        source,
        package,
        registry,
        version,
        installed_at_ms: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_millis())
            .unwrap_or(0),
    }
}

fn verify_npm_integrity(data: &[u8], integrity: &str) -> Result<()> {
    let Some(encoded) = integrity.strip_prefix("sha512-") else {
        bail!("unsupported npm integrity algorithm: {integrity}");
    };
    let expected = BASE64_STANDARD
        .decode(encoded)
        .with_context(|| "failed to decode npm sha512 integrity")?;
    let actual = Sha512::digest(data);
    if expected.as_slice() != actual.as_slice() {
        bail!("npm tarball integrity check failed");
    }
    Ok(())
}

fn install_provider_to_staging(
    source: &ProviderInstallSource,
    staging: &Path,
) -> Result<ProviderPackageManifest> {
    match &source.package {
        ProviderInstallPackage::Directory(path) => copy_provider_directory(path, staging)?,
        ProviderInstallPackage::Tgz(data) => unpack_provider_tgz(data, staging)?,
    }
    let manifest = read_provider_manifest(staging)?;
    validate_provider_manifest(staging, &manifest)?;
    Ok(manifest)
}

fn finish_provider_install(
    force: bool,
    providers_dir: &Path,
    staging: &Path,
    manifest: ProviderPackageManifest,
    metadata: &ProviderInstallMetadata,
) -> Result<(InstalledProvider, PathBuf)> {
    let final_path = providers_dir.join(&manifest.id);
    if final_path.exists() {
        if !force {
            bail!(
                "provider '{}' is already installed at {}; pass --force to replace it",
                manifest.id,
                final_path.display()
            );
        }
        fs::remove_dir_all(&final_path).with_context(|| {
            format!(
                "failed to replace existing provider directory {}",
                final_path.display()
            )
        })?;
    }
    fs::rename(staging, &final_path).with_context(|| {
        format!(
            "failed to move provider from {} to {}",
            staging.display(),
            final_path.display()
        )
    })?;
    ensure_provider_command_executable(&final_path, &manifest)?;
    write_provider_install_metadata(&final_path, metadata)?;
    let installed = InstalledProvider {
        root: final_path.clone(),
        manifest,
        install_metadata: Some(metadata.clone()),
    };
    Ok((installed, final_path))
}

fn write_provider_install_metadata(root: &Path, metadata: &ProviderInstallMetadata) -> Result<()> {
    let content = serde_json::to_string_pretty(metadata)?;
    fs::write(
        root.join(PROVIDER_INSTALL_METADATA_FILE),
        format!("{content}\n"),
    )
    .with_context(|| {
        format!(
            "failed to write provider install metadata in {}",
            root.display()
        )
    })
}

fn read_provider_install_metadata(root: &Path) -> Option<ProviderInstallMetadata> {
    fs::read_to_string(root.join(PROVIDER_INSTALL_METADATA_FILE))
        .ok()
        .and_then(|content| serde_json::from_str(&content).ok())
}

fn ensure_provider_command_executable(
    root: &Path,
    manifest: &ProviderPackageManifest,
) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let command = root.join(&manifest.command);
        let metadata = fs::metadata(&command)
            .with_context(|| format!("failed to stat provider command {}", command.display()))?;
        let mut mode = metadata.permissions().mode();
        mode |= 0o755;
        fs::set_permissions(&command, fs::Permissions::from_mode(mode)).with_context(|| {
            format!(
                "failed to make provider command executable {}",
                command.display()
            )
        })?;
    }
    Ok(())
}

fn copy_provider_directory(source: &Path, staging: &Path) -> Result<()> {
    for entry in fs::read_dir(source)
        .with_context(|| format!("failed to read provider directory {}", source.display()))?
    {
        let entry = entry?;
        let target = staging.join(entry.file_name());
        copy_provider_entry(&entry.path(), &target)?;
    }
    Ok(())
}

fn copy_provider_entry(source: &Path, target: &Path) -> Result<()> {
    let metadata =
        fs::metadata(source).with_context(|| format!("failed to stat {}", source.display()))?;
    if metadata.is_dir() {
        fs::create_dir_all(target)
            .with_context(|| format!("failed to create directory {}", target.display()))?;
        for entry in fs::read_dir(source)
            .with_context(|| format!("failed to read directory {}", source.display()))?
        {
            let entry = entry?;
            copy_provider_entry(&entry.path(), &target.join(entry.file_name()))?;
        }
    } else if metadata.is_file() {
        fs::copy(source, target).with_context(|| {
            format!(
                "failed to copy provider file {} to {}",
                source.display(),
                target.display()
            )
        })?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(
                target,
                fs::Permissions::from_mode(metadata.permissions().mode()),
            )
            .with_context(|| format!("failed to preserve permissions for {}", target.display()))?;
        }
    }
    Ok(())
}

fn unpack_provider_tgz(data: &[u8], staging: &Path) -> Result<()> {
    let decoder = GzDecoder::new(data);
    let mut archive = tar::Archive::new(decoder);
    for entry in archive
        .entries()
        .with_context(|| "failed to read provider package archive")?
    {
        let mut entry = entry.with_context(|| "failed to read provider package entry")?;
        let path = entry
            .path()
            .with_context(|| "failed to read provider package entry path")?;
        let Some(relative) = sanitize_provider_archive_path(&path) else {
            continue;
        };
        let entry_type = entry.header().entry_type();
        if !(entry_type.is_file() || entry_type.is_dir()) {
            continue;
        }
        let target = staging.join(relative);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("failed to create directory {}", parent.display()))?;
        }
        entry
            .unpack(&target)
            .with_context(|| format!("failed to unpack provider file {}", target.display()))?;
    }
    Ok(())
}

fn sanitize_provider_archive_path(path: &Path) -> Option<PathBuf> {
    let mut parts = path.components().peekable();
    if matches!(
        parts.peek(),
        Some(std::path::Component::Normal(value)) if *value == std::ffi::OsStr::new("package")
    ) {
        let _ = parts.next();
    }
    let mut clean = PathBuf::new();
    for component in parts {
        match component {
            std::path::Component::Normal(part) => clean.push(part),
            std::path::Component::CurDir => {}
            _ => return None,
        }
    }
    (!clean.as_os_str().is_empty()).then_some(clean)
}

fn read_provider_manifest(root: &Path) -> Result<ProviderPackageManifest> {
    let path = root.join("provider.toml");
    let content = fs::read_to_string(&path)
        .with_context(|| format!("failed to read provider manifest {}", path.display()))?;
    toml::from_str(&content)
        .with_context(|| format!("failed to parse provider manifest {}", path.display()))
}

fn validate_provider_manifest(root: &Path, manifest: &ProviderPackageManifest) -> Result<()> {
    if manifest.id.trim().is_empty() {
        bail!("provider manifest id is required");
    }
    if manifest.id.contains('/') || manifest.id.contains('\\') || manifest.id == "." {
        bail!("provider manifest id must be a simple directory name");
    }
    if manifest.protocol != PROVIDER_PROTOCOL {
        bail!(
            "provider '{}' uses unsupported protocol '{}'; expected '{}'",
            manifest.id,
            manifest.protocol,
            PROVIDER_PROTOCOL
        );
    }
    if manifest.command.as_os_str().is_empty() || manifest.command.is_absolute() {
        bail!("provider '{}' command must be a relative path", manifest.id);
    }
    let command = root.join(&manifest.command);
    if !command.is_file() {
        bail!(
            "provider '{}' command does not exist: {}",
            manifest.id,
            command.display()
        );
    }
    Ok(())
}

async fn run_update_command(command: &UpdateCommand) -> Result<()> {
    let install_dir = command_install_dir(command.install_dir.as_ref())?;
    let mut process = tokio::process::Command::new("sh");
    process
        .arg("-c")
        .arg(update_installer_command())
        .env("DICTA_INSTALL_DIR", &install_dir);
    if let Some(version) = non_empty(&command.version) {
        process.env("DICTA_VERSION", version);
    }
    let status = process.status().await.with_context(
        || "failed to run installer; install curl or update manually with install.sh",
    )?;
    if !status.success() {
        bail!("dicta update failed with status {status}");
    }
    Ok(())
}

fn update_installer_command() -> &'static str {
    r#"tmp="${TMPDIR:-/tmp}/dicta-install.$$"; trap 'rm -f "$tmp"' EXIT; if command -v curl >/dev/null 2>&1; then curl -fsSL https://raw.githubusercontent.com/kingsword09/dicta/main/install.sh -o "$tmp"; elif command -v wget >/dev/null 2>&1; then wget -qO "$tmp" https://raw.githubusercontent.com/kingsword09/dicta/main/install.sh; else echo 'dicta update: required command not found: curl or wget' >&2; exit 1; fi && sh "$tmp""#
}

fn run_uninstall_command(command: &UninstallCommand) -> Result<()> {
    let install_dir = command_install_dir(command.install_dir.as_ref())?;
    if !command.yes && !confirm_uninstall(&install_dir)? {
        println!("dicta uninstall: cancelled");
        return Ok(());
    }

    let mut removed_any = false;
    for name in installed_binary_names() {
        let path = install_dir.join(name);
        if path.exists() {
            fs::remove_file(&path)
                .with_context(|| format!("failed to remove {}", path.display()))?;
            println!("dicta uninstall: removed {}", path.display());
            removed_any = true;
        } else {
            println!("dicta uninstall: {} is not installed", path.display());
        }
    }
    if !removed_any {
        println!(
            "dicta uninstall: no installed dicta binaries found in {}",
            install_dir.display()
        );
    }
    println!("dicta uninstall: configuration files were left in place");
    Ok(())
}

fn command_install_dir(explicit: Option<&PathBuf>) -> Result<PathBuf> {
    if let Some(path) = explicit {
        return Ok(path.clone());
    }
    let exe = env::current_exe().context("failed to resolve current dicta executable")?;
    exe.parent()
        .map(std::path::Path::to_path_buf)
        .context("failed to determine install directory from current executable")
}

fn installed_binary_names() -> [&'static str; 3] {
    if cfg!(windows) {
        [
            "dicta.exe",
            "dicta-tray.exe",
            "dicta-adapter-apple-speech.exe",
        ]
    } else {
        ["dicta", "dicta-tray", "dicta-adapter-apple-speech"]
    }
}

fn confirm_uninstall(install_dir: &std::path::Path) -> Result<bool> {
    if !io::stdin().is_terminal() {
        bail!("refusing to uninstall without confirmation on non-interactive stdin; pass --yes");
    }
    eprint!(
        "Remove dicta binaries from {}? [y/N] ",
        install_dir.display()
    );
    io::stderr().flush().ok();
    let mut input = String::new();
    io::stdin()
        .read_line(&mut input)
        .context("failed to read uninstall confirmation")?;
    Ok(is_uninstall_confirmation(&input))
}

fn is_uninstall_confirmation(input: &str) -> bool {
    matches!(input.trim().to_lowercase().as_str(), "y" | "yes")
}

fn gather_provider_list_report(cli: &Cli) -> Result<ProviderListReport> {
    let apple_support = apple_support();
    let current = active_or_default_provider_name(cli, &apple_support)?;
    let profiles = available_provider_profiles(cli)?;
    let mut providers = Vec::new();
    for (name, profile) in profiles {
        let profile_cli = cli.with_provider_name(Some(name.clone()));
        let report = gather_capabilities_report_with_support(&profile_cli, &apple_support);
        let effective = effective_provider_for(&profile_cli, &apple_support, true).ok();
        let installed_provider = effective
            .as_ref()
            .and_then(|provider| provider.installed_provider());
        providers.push(ProviderListEntry {
            name: name.clone(),
            kind: profile.kind.as_str().to_owned(),
            built_in: builtin_provider_profile(&name).is_some(),
            installed: installed_provider.is_some(),
            install_path: installed_provider.map(|provider| provider.root.display().to_string()),
            installed_version: installed_provider
                .and_then(|provider| provider.manifest.version.clone()),
            source_package: installed_provider
                .and_then(|provider| provider.source_package().map(ToOwned::to_owned)),
            selected: current.as_deref() == Some(name.as_str()),
            model: report.model,
            batch_file: report
                .batch
                .as_ref()
                .is_some_and(|capabilities| capabilities.batch_file),
            live: report.live.is_some(),
            local_config_ok: report.local_config_ok,
            local_config_error: report.local_config_error.or(report.resolution_error),
            notes: report.notes,
        });
    }

    Ok(ProviderListReport {
        current,
        state_path: provider_state_path(cli).map(|path| path.display().to_string()),
        provider_config: provider_config_path(cli).map(|path| path.display().to_string()),
        providers,
    })
}

fn gather_current_provider_report(cli: &Cli) -> Result<CurrentProviderReport> {
    let apple_support = apple_support();
    let current = active_or_default_provider_name(cli, &apple_support)?;
    let Some(provider) = current else {
        return Ok(CurrentProviderReport {
            provider: None,
            state_path: provider_state_path(cli).map(|path| path.display().to_string()),
            resolved: None,
            kind: None,
            model: None,
            live: false,
            local_config_ok: false,
            local_config_error: Some(
                "no active provider is set; run `dicta provider set <name>`".to_owned(),
            ),
            install_path: None,
        });
    };
    let current_cli = cli.with_provider_name(Some(provider.clone()));
    let report = gather_capabilities_report(&current_cli);
    let effective = effective_provider_for(&current_cli, &apple_support, true).ok();
    Ok(CurrentProviderReport {
        provider: Some(provider),
        state_path: provider_state_path(cli).map(|path| path.display().to_string()),
        resolved: report.resolved,
        kind: report.provider_kind,
        model: Some(report.model),
        live: report.live.is_some(),
        local_config_ok: report.local_config_ok,
        local_config_error: report.local_config_error.or(report.resolution_error),
        install_path: effective
            .and_then(|provider| provider.installed_provider().cloned())
            .map(|provider| provider.root.display().to_string()),
    })
}

fn print_provider_list_text(report: &ProviderListReport) {
    println!("Providers");
    if let Some(current) = &report.current {
        println!("  Current: {current}");
    } else {
        println!("  Current: none");
    }
    if let Some(path) = &report.provider_config {
        println!("  Config: {path}");
    }
    if let Some(path) = &report.state_path {
        println!("  State: {path}");
    }
    println!();
    for provider in &report.providers {
        let marker = if provider.selected { "*" } else { " " };
        let source = if provider.built_in {
            "built-in"
        } else if provider.installed {
            "installed"
        } else {
            "custom"
        };
        println!(
            "{marker} {} ({}, {}, model {})",
            provider.name, provider.kind, source, provider.model
        );
        println!("    Batch file: {}", yes_no(provider.batch_file));
        println!("    Live: {}", yes_no(provider.live));
        println!("    Local config ok: {}", yes_no(provider.local_config_ok));
        if let Some(version) = &provider.installed_version {
            println!("    Installed version: {version}");
        }
        if let Some(package) = &provider.source_package {
            println!("    Package: {package}");
        }
        if let Some(path) = &provider.install_path {
            println!("    Installed at: {path}");
        }
        if let Some(error) = &provider.local_config_error {
            println!("    Error: {error}");
        }
        for note in &provider.notes {
            println!("    Note: {note}");
        }
    }
}

fn print_current_provider_text(report: &CurrentProviderReport) {
    println!("Current provider");
    if let Some(provider) = &report.provider {
        println!("  Provider: {provider}");
    } else {
        println!("  Provider: none");
    }
    if let Some(resolved) = &report.resolved {
        println!("  Resolved: {resolved}");
    }
    if let Some(kind) = &report.kind {
        println!("  Kind: {kind}");
    }
    if let Some(model) = &report.model {
        println!("  Model: {model}");
    }
    println!("  Live: {}", yes_no(report.live));
    println!("  Local config ok: {}", yes_no(report.local_config_ok));
    if let Some(error) = &report.local_config_error {
        println!("  Error: {error}");
    }
    if let Some(path) = &report.install_path {
        println!("  Installed at: {path}");
    }
    if let Some(path) = &report.state_path {
        println!("  State: {path}");
    }
}

fn gather_doctor_report(cli: &Cli) -> DoctorReport {
    gather_doctor_report_with_audio(cli, default_audio_report())
}

fn default_audio_report() -> AudioReport {
    match dicta_audio::default_input_device_info() {
        Ok(info) => AudioReport {
            default_input_available: true,
            name: Some(info.name),
            sample_rate: Some(info.sample_rate),
            channels: Some(info.channels),
            sample_format: Some(info.sample_format),
            error: None,
        },
        Err(err) => AudioReport {
            default_input_available: false,
            name: None,
            sample_rate: None,
            channels: None,
            sample_format: None,
            error: Some(err.to_string()),
        },
    }
}

fn gather_doctor_report_with_audio(cli: &Cli, audio: AudioReport) -> DoctorReport {
    let apple_support = apple_support();
    let effective_result = effective_provider_for(cli, &apple_support, false);
    let backend_result = effective_result.as_ref().map(|provider| provider.backend);
    let resolved = backend_result
        .as_ref()
        .ok()
        .map(|backend| backend.as_str().to_owned());
    let error = match &effective_result {
        Ok(provider) => provider.config_error.clone(),
        Err(err) => Some(err.to_string()),
    };
    let model = non_empty(&cli.api_model).unwrap_or_else(|| {
        effective_result
            .as_ref()
            .ok()
            .and_then(|provider| {
                if provider.backend == AsrBackend::OpenaiCompatible {
                    Some(openai_compatible_model(cli, provider.profile.as_ref()))
                } else if let Some(profile) = provider.profile.as_ref() {
                    profile
                        .profile
                        .default_model
                        .as_deref()
                        .and_then(|model| non_empty_string(Some(model)))
                        .map(ToOwned::to_owned)
                } else {
                    resolved
                        .as_deref()
                        .map(default_model_for_name)
                        .map(ToOwned::to_owned)
                }
            })
            .unwrap_or_else(|| "whisper-1".to_owned())
    });
    let provider_name = effective_result
        .as_ref()
        .ok()
        .and_then(|provider| provider.profile_name().map(ToOwned::to_owned));
    let provider_kind = effective_result
        .as_ref()
        .ok()
        .and_then(|provider| provider.profile_kind().map(ToOwned::to_owned));
    let live = gather_live_report(cli, &apple_support);
    let capabilities = gather_capabilities_report_with_support(cli, &apple_support);

    DoctorReport {
        system: SystemReport {
            os: std::env::consts::OS,
            arch: std::env::consts::ARCH,
            macos_version: apple_support.version,
            apple_on_device_supported: apple_support.supported,
            apple_on_device_reason: apple_support.reason,
        },
        backend: BackendReport {
            requested: cli.asr.as_str().to_owned(),
            resolved,
            provider: provider_name,
            provider_kind,
            provider_config: provider_config_path(cli).map(|path| path.display().to_string()),
            api_base_configured: configured_api_base(cli, effective_result.as_ref().ok()).is_some(),
            api_key_configured: configured_api_key(cli, effective_result.as_ref().ok()).is_some(),
            model,
            native_adapter: resolve_native_adapter(cli)
                .as_ref()
                .map(|path| path.display().to_string()),
            error,
        },
        capabilities,
        live,
        audio,
        runtime: RuntimeReport {
            python_sidecar_required: false,
            web_direct_available: true,
            native_adapter_supported_os: true,
        },
    }
}

fn gather_capabilities_report(cli: &Cli) -> CapabilitiesReport {
    let apple_support = apple_support();
    gather_capabilities_report_with_support(cli, &apple_support)
}

fn gather_capabilities_report_with_support(
    cli: &Cli,
    apple_support: &AppleSupport,
) -> CapabilitiesReport {
    let effective_result = effective_provider_for(cli, apple_support, true);
    let backend_result = effective_result.as_ref().map(|provider| provider.backend);
    let resolved = backend_result
        .as_ref()
        .ok()
        .map(|backend| backend.as_str().to_owned());
    let resolution_error = effective_result.as_ref().err().map(ToString::to_string);
    let local_config_error = effective_result
        .as_ref()
        .ok()
        .and_then(|provider| provider.config_error.clone());
    let model = non_empty(&cli.api_model).unwrap_or_else(|| {
        effective_result
            .as_ref()
            .ok()
            .and_then(|provider| {
                if provider.backend == AsrBackend::OpenaiCompatible {
                    Some(openai_compatible_model(cli, provider.profile.as_ref()))
                } else if let Some(profile) = provider.profile.as_ref() {
                    profile
                        .profile
                        .default_model
                        .as_deref()
                        .and_then(|model| non_empty_string(Some(model)))
                        .map(ToOwned::to_owned)
                } else {
                    resolved
                        .as_deref()
                        .map(default_model_for_name)
                        .map(ToOwned::to_owned)
                }
            })
            .unwrap_or_else(|| "whisper-1".to_owned())
    });

    let profile = effective_result
        .as_ref()
        .ok()
        .and_then(|provider| provider.profile_name().map(ToOwned::to_owned));
    let provider_kind = effective_result
        .as_ref()
        .ok()
        .and_then(|provider| provider.profile_kind().map(ToOwned::to_owned));
    let (batch, live, notes) = effective_result
        .as_ref()
        .ok()
        .map(|capabilities| {
            (
                Some(batch_capabilities_report(
                    capabilities.capabilities.batch.clone(),
                )),
                capabilities
                    .capabilities
                    .live
                    .clone()
                    .map(live_capabilities_report),
                capabilities.capabilities.notes.clone(),
            )
        })
        .unwrap_or((None, None, Vec::new()));

    CapabilitiesReport {
        requested: cli.asr.as_str().to_owned(),
        resolved,
        provider: profile,
        provider_kind,
        provider_config: provider_config_path(cli).map(|path| path.display().to_string()),
        model,
        local_config_ok: resolution_error.is_none() && local_config_error.is_none(),
        local_config_error,
        resolution_error,
        batch,
        live,
        notes,
    }
}

fn provider_capabilities_for_backend(backend: AsrBackend) -> ProviderCapabilities {
    match backend {
        AsrBackend::OpenaiCompatible => openai_compatible_capabilities(),
        AsrBackend::Apple => native_adapter_capabilities(),
        AsrBackend::External => ProviderCapabilities {
            batch: AsrCapabilities {
                batch_file: false,
                streaming: false,
                requires_network: false,
            },
            live: None,
            notes: vec!["External provider capabilities are loaded from provider.toml.".to_owned()],
        },
        AsrBackend::Auto => unreachable!("backend must be resolved first"),
    }
}

fn effective_provider_for(
    cli: &Cli,
    apple_support: &AppleSupport,
    capability_mode: bool,
) -> Result<EffectiveProvider> {
    let profile = resolve_provider_profile_for(cli, apple_support)?;
    let backend = if let Some(profile) = &profile {
        profile.profile.kind.backend()
    } else if capability_mode {
        match cli.asr {
            AsrBackend::Auto => resolve_backend_for(cli, apple_support)?,
            AsrBackend::OpenaiCompatible | AsrBackend::Apple => cli.asr,
            AsrBackend::External => {
                unreachable!("external backend is selected through --provider")
            }
        }
    } else {
        resolve_backend_for(cli, apple_support)?
    };
    let capabilities = profile
        .as_ref()
        .and_then(|profile| {
            profile
                .installed
                .as_ref()
                .map(InstalledProvider::capabilities)
        })
        .unwrap_or_else(|| provider_capabilities_for_backend(backend));
    let (capabilities, profile_error) = if let Some(profile) = &profile {
        effective_capabilities_for_profile(capabilities, profile)
    } else {
        (capabilities, None)
    };
    let config_error =
        profile_error.or_else(|| validate_capability_config(backend, cli, apple_support));

    Ok(EffectiveProvider {
        backend,
        profile,
        capabilities,
        config_error,
    })
}

fn effective_capabilities_for_profile(
    mut capabilities: ProviderCapabilities,
    profile: &ResolvedProviderProfile,
) -> (ProviderCapabilities, Option<String>) {
    if let Some(value) = profile.profile.batch_file {
        capabilities.batch.batch_file = capabilities.batch.batch_file && value;
    }
    if let Some(value) = profile.profile.streaming {
        capabilities.batch.streaming = capabilities.batch.streaming && value;
    }
    if let Some(value) = profile.profile.requires_network {
        capabilities.batch.requires_network = capabilities.batch.requires_network || value;
    }

    let mut error = None;
    if profile.profile.live_enabled == Some(true) && capabilities.live.is_none() {
        error = Some(format!(
            "provider profile '{}' enables live mode, but {} does not support live mode",
            profile.name,
            profile.profile.kind.as_str()
        ));
    }
    if profile.profile.live_enabled == Some(false) {
        capabilities.live = None;
    }

    capabilities.notes.extend(profile.profile.notes.clone());
    (capabilities, error)
}

fn batch_capabilities_report(capabilities: AsrCapabilities) -> BatchCapabilitiesReport {
    BatchCapabilitiesReport {
        batch_file: capabilities.batch_file,
        streaming: capabilities.streaming,
        requires_network: capabilities.requires_network,
    }
}

fn live_capabilities_report(capabilities: LiveCapabilities) -> LiveCapabilitiesReport {
    LiveCapabilitiesReport {
        mode: live_mode_name(capabilities.mode).to_owned(),
        mic: capabilities.mic,
        speaker: capabilities.speaker,
        streaming_audio: capabilities.streaming_audio,
        partial_results: capabilities.partial_results,
        finalized_results: capabilities.finalized_results,
        translation: capabilities.translation,
        voice_processing: capabilities.voice_processing,
        device_selection: capabilities.device_selection,
        requires_network: capabilities.requires_network,
        expected_latency: capabilities.expected_latency.map(format_duration),
    }
}

fn gather_live_report(cli: &Cli, apple_support: &AppleSupport) -> LiveReport {
    match resolve_live_backend_for(cli, apple_support) {
        Ok(backend) => {
            let capabilities = if backend == AsrBackend::External {
                effective_provider_for(cli, apple_support, true)
                    .ok()
                    .and_then(|provider| provider.capabilities.live)
                    .unwrap_or_else(|| live_capabilities_for_backend(backend))
            } else {
                live_capabilities_for_backend(backend)
            };
            LiveReport {
                backend: Some(backend.as_str().to_owned()),
                mode: Some(live_mode_name(capabilities.mode).to_owned()),
                mic: capabilities.mic,
                speaker: capabilities.speaker,
                streaming_audio: capabilities.streaming_audio,
                partial_results: capabilities.partial_results,
                finalized_results: capabilities.finalized_results,
                translation: capabilities.translation,
                voice_processing: capabilities.voice_processing,
                device_selection: capabilities.device_selection,
                requires_network: capabilities.requires_network,
                expected_latency: live_expected_latency(cli, &capabilities).map(format_duration),
                error: None,
            }
        }
        Err(err) => LiveReport {
            backend: None,
            mode: None,
            mic: false,
            speaker: false,
            streaming_audio: false,
            partial_results: false,
            finalized_results: false,
            translation: false,
            voice_processing: false,
            device_selection: false,
            requires_network: false,
            expected_latency: None,
            error: Some(err.to_string()),
        },
    }
}

fn live_expected_latency(cli: &Cli, capabilities: &LiveCapabilities) -> Option<Duration> {
    cli.live_chunk
        .filter(|seconds| seconds.is_finite() && *seconds > 0.0)
        .map(Duration::from_secs_f64)
        .or(capabilities.expected_latency)
}

fn live_capabilities_for_backend(backend: AsrBackend) -> LiveCapabilities {
    match backend {
        AsrBackend::Apple => native_adapter_live_capabilities(),
        AsrBackend::Auto | AsrBackend::OpenaiCompatible | AsrBackend::External => {
            LiveCapabilities {
                mode: LiveModeKind::Chunked,
                mic: false,
                speaker: false,
                streaming_audio: false,
                partial_results: false,
                finalized_results: false,
                translation: false,
                voice_processing: false,
                device_selection: false,
                requires_network: false,
                expected_latency: None,
            }
        }
    }
}

fn live_mode_name(mode: LiveModeKind) -> &'static str {
    match mode {
        LiveModeKind::Streaming => "streaming",
        LiveModeKind::Chunked => "chunked",
    }
}

fn format_duration(duration: Duration) -> String {
    if duration.subsec_millis() == 0 {
        format!("~{}s", duration.as_secs())
    } else {
        format!("~{:.1}s", duration.as_secs_f64())
    }
}

#[derive(Debug, Clone)]
struct AppleSupport {
    version: Option<String>,
    supported: bool,
    reason: String,
}

fn apple_support() -> AppleSupport {
    if !cfg!(target_os = "macos") {
        return AppleSupport {
            version: None,
            supported: false,
            reason: "Apple on-device ASR requires macOS 26+".to_owned(),
        };
    }

    let version = macos_version();
    let major = version
        .as_deref()
        .and_then(|v| v.split('.').next())
        .and_then(|major| major.parse::<u32>().ok());

    match (version, major) {
        (Some(version), Some(major)) if major >= 26 => AppleSupport {
            version: Some(version),
            supported: true,
            reason: "macOS 26+ detected; Apple Speech adapter can provide on-device mode"
                .to_owned(),
        },
        (Some(version), Some(_)) => AppleSupport {
            version: Some(version.clone()),
            supported: false,
            reason: format!(
                "macOS {version} is below 26; use an HTTP provider or install a live provider"
            ),
        },
        (version, _) => AppleSupport {
            version,
            supported: false,
            reason: "could not determine macOS major version".to_owned(),
        },
    }
}

fn macos_version() -> Option<String> {
    if !cfg!(target_os = "macos") {
        return None;
    }
    let output = std::process::Command::new("sw_vers")
        .arg("-productVersion")
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let version = String::from_utf8(output.stdout).ok()?.trim().to_owned();
    if version.is_empty() {
        None
    } else {
        Some(version)
    }
}

fn print_doctor_text(report: &DoctorReport) {
    println!("System");
    println!("  OS: {}", report.system.os);
    println!("  Arch: {}", report.system.arch);
    if let Some(version) = &report.system.macos_version {
        println!("  macOS version: {version}");
    }
    println!(
        "  Apple on-device supported: {}",
        yes_no(report.system.apple_on_device_supported)
    );
    println!(
        "  Apple on-device reason: {}",
        report.system.apple_on_device_reason
    );

    println!("\nBackend");
    println!("  Requested: {}", report.backend.requested);
    if let Some(resolved) = &report.backend.resolved {
        println!("  Resolved: {resolved}");
    }
    if let Some(provider) = &report.backend.provider {
        println!("  Provider profile: {provider}");
    }
    if let Some(kind) = &report.backend.provider_kind {
        println!("  Provider kind: {kind}");
    }
    if let Some(config) = &report.backend.provider_config {
        println!("  Provider config: {config}");
    }
    if let Some(error) = &report.backend.error {
        println!("  Error: {error}");
    }
    print_doctor_backend_details(report);

    println!();
    print_capabilities_text(&report.capabilities);

    println!("\nAudio");
    if report.audio.default_input_available {
        println!(
            "  Default input: {}",
            report.audio.name.as_deref().unwrap_or("unknown")
        );
        println!(
            "  Sample rate: {}",
            report.audio.sample_rate.unwrap_or_default()
        );
        println!("  Channels: {}", report.audio.channels.unwrap_or_default());
        println!(
            "  Sample format: {}",
            report.audio.sample_format.as_deref().unwrap_or("unknown")
        );
    } else {
        println!("  Default input: unavailable");
        if let Some(error) = &report.audio.error {
            println!("  Error: {error}");
        }
        if let Some(hint) = doctor_audio_hint(report) {
            println!("  Hint: {hint}");
        }
    }
}

fn print_capabilities_text(report: &CapabilitiesReport) {
    println!("Capabilities");
    println!("  Requested: {}", report.requested);
    if let Some(resolved) = &report.resolved {
        println!("  Resolved: {resolved}");
    }
    if let Some(provider) = &report.provider {
        println!("  Provider profile: {provider}");
    }
    if let Some(kind) = &report.provider_kind {
        println!("  Provider kind: {kind}");
    }
    if let Some(config) = &report.provider_config {
        println!("  Provider config: {config}");
    }
    println!("  Model: {}", report.model);
    println!("  Local config ok: {}", yes_no(report.local_config_ok));
    if let Some(error) = &report.resolution_error {
        println!("  Resolution error: {error}");
    }
    if let Some(error) = &report.local_config_error {
        println!("  Local config error: {error}");
    }
    if let Some(batch) = &report.batch {
        println!("  Batch file: {}", yes_no(batch.batch_file));
        println!("  Batch streaming: {}", yes_no(batch.streaming));
        println!(
            "  Batch requires network: {}",
            yes_no(batch.requires_network)
        );
    }
    if let Some(live) = &report.live {
        println!("  Live mode: {}", live.mode);
        println!("  Live mic: {}", yes_no(live.mic));
        println!("  Live speaker: {}", yes_no(live.speaker));
        println!("  Live streaming audio: {}", yes_no(live.streaming_audio));
        println!("  Live partial results: {}", yes_no(live.partial_results));
        println!(
            "  Live finalized results: {}",
            yes_no(live.finalized_results)
        );
        println!("  Live translation: {}", yes_no(live.translation));
        println!("  Live voice processing: {}", yes_no(live.voice_processing));
        println!("  Live device selection: {}", yes_no(live.device_selection));
        println!("  Live requires network: {}", yes_no(live.requires_network));
        if let Some(latency) = &live.expected_latency {
            println!("  Live expected latency: {latency}");
        }
    } else {
        println!("  Live: no");
    }
    for note in &report.notes {
        println!("  Note: {note}");
    }
}

fn print_doctor_backend_details(report: &DoctorReport) {
    match report.backend.resolved.as_deref() {
        Some("openai-compatible") => {
            println!(
                "  API base configured: {}",
                yes_no(report.backend.api_base_configured)
            );
            println!(
                "  API key configured: {}",
                yes_no(report.backend.api_key_configured)
            );
            println!("  Model: {}", report.backend.model);
        }
        Some("apple") => {
            if let Some(adapter) = &report.backend.native_adapter {
                println!("  Native adapter: {adapter}");
            }
        }
        _ => {
            println!("  Model: {}", report.backend.model);
        }
    }
}

fn doctor_audio_hint(report: &DoctorReport) -> Option<&'static str> {
    if report.system.os == "macos" {
        Some(
            "start a microphone capture from the terminal app you use, for example `dicta --mic-duration 1 --asr openai-compatible`, so macOS can show the permission prompt; then allow microphone access in System Settings > Privacy & Security > Microphone, restart the terminal, and check that a default input device is selected",
        )
    } else {
        None
    }
}

fn yes_no(value: bool) -> &'static str {
    if value { "yes" } else { "no" }
}

struct ResolvedAudioSource {
    path: PathBuf,
    channel: AudioChannel,
    cleanup: bool,
}

impl ResolvedAudioSource {
    fn cleanup(&self) {
        if self.cleanup {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

fn resolve_audio_source(cli: &Cli) -> Result<ResolvedAudioSource> {
    match (&cli.input, cli.mic_duration) {
        (Some(_), Some(_)) => bail!("--input and --mic-duration are mutually exclusive"),
        (None, None) => bail!("one of --input or --mic-duration is required"),
        (Some(input), None) => {
            if !input.exists() {
                bail!("input file does not exist: {}", input.display());
            }
            if !input.is_file() {
                bail!("input path is not a file: {}", input.display());
            }
            Ok(ResolvedAudioSource {
                path: input.clone(),
                channel: AudioChannel::File,
                cleanup: false,
            })
        }
        (None, Some(seconds)) => {
            if !seconds.is_finite() || seconds <= 0.0 {
                bail!("--mic-duration must be greater than zero seconds");
            }
            let path = temp_recording_path();
            eprintln!("dicta: recording default microphone for {seconds:.1}s...");
            dicta_audio::record_default_input_to_wav(&path, Duration::from_secs_f64(seconds))
                .with_context(|| "failed to record default microphone")?;
            Ok(ResolvedAudioSource {
                path,
                channel: AudioChannel::Mic,
                cleanup: true,
            })
        }
    }
}

fn temp_recording_path() -> PathBuf {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    std::env::temp_dir().join(format!("dicta-mic-{millis}.wav"))
}

fn resolve_backend(cli: &Cli) -> Result<AsrBackend> {
    let support = apple_support();
    resolve_backend_for(cli, &support)
}

fn resolve_backend_for(cli: &Cli, apple_support: &AppleSupport) -> Result<AsrBackend> {
    if let Some(profile) = resolve_provider_profile_for(cli, apple_support)? {
        return Ok(profile.profile.kind.backend());
    }

    match cli.asr {
        AsrBackend::Auto => {
            let _ = apple_support;
            Ok(AsrBackend::OpenaiCompatible)
        }
        AsrBackend::OpenaiCompatible => Ok(cli.asr),
        AsrBackend::External => unreachable!("external backend is selected through --provider"),
        AsrBackend::Apple => {
            if apple_support.supported {
                Ok(AsrBackend::Apple)
            } else {
                bail!("apple ASR is unavailable: {}", apple_support.reason)
            }
        }
    }
}

fn build_provider(backend: AsrBackend, cli: &Cli) -> Result<Box<dyn AsrProvider>> {
    match backend {
        AsrBackend::OpenaiCompatible => {
            let profile = resolve_provider_profile(cli)?;
            let base_url = openai_compatible_api_base(cli, profile.as_ref());
            let model = openai_compatible_model(cli, profile.as_ref());
            Ok(Box::new(OpenAiCompatibleAsr::new(
                OpenAiCompatibleConfig {
                    base_url,
                    api_key: openai_compatible_api_key(cli, profile.as_ref()),
                    model,
                },
            )?))
        }
        AsrBackend::Apple => Ok(Box::new(build_native_adapter(cli)?)),
        AsrBackend::External => Ok(Box::new(build_external_provider(cli)?)),
        AsrBackend::Auto => unreachable!("backend must be resolved first"),
    }
}

fn resolve_provider_profile(cli: &Cli) -> Result<Option<ResolvedProviderProfile>> {
    let apple_support = apple_support();
    resolve_provider_profile_for(cli, &apple_support)
}

fn resolve_provider_profile_for(
    cli: &Cli,
    apple_support: &AppleSupport,
) -> Result<Option<ResolvedProviderProfile>> {
    let Some(name) = resolve_requested_provider_name(cli, apple_support)? else {
        return Ok(None);
    };

    if let Some(profile) = builtin_provider_profile(&name) {
        return Ok(Some(ResolvedProviderProfile {
            name,
            profile,
            installed: None,
        }));
    }

    if let Some(installed) = installed_provider(&name, cli)? {
        let profile = installed.profile();
        return Ok(Some(ResolvedProviderProfile {
            name,
            profile,
            installed: Some(installed),
        }));
    }

    let Some(config_path) = provider_config_path(cli) else {
        bail!(
            "provider profile '{name}' was not found in built-ins and no provider config file was found"
        );
    };
    let content = std::fs::read_to_string(&config_path).with_context(|| {
        format!(
            "failed to read provider config from {}",
            config_path.display()
        )
    })?;
    let parsed: ProviderProfilesFile = toml::from_str(&content).with_context(|| {
        format!(
            "failed to parse provider config from {}",
            config_path.display()
        )
    })?;
    let profile = parsed.providers.get(&name).cloned().with_context(|| {
        format!(
            "provider profile '{name}' was not found in {}",
            config_path.display()
        )
    })?;

    Ok(Some(ResolvedProviderProfile {
        name,
        profile,
        installed: None,
    }))
}

fn resolve_requested_provider_name(
    cli: &Cli,
    apple_support: &AppleSupport,
) -> Result<Option<String>> {
    let Some(name) = non_empty(&cli.provider) else {
        return Ok(None);
    };
    if name == "active" {
        let Some(active) = active_or_default_provider_name(cli, apple_support)? else {
            bail!(
                "no active provider is set; run `dicta provider set <name>` or pass a concrete --provider"
            );
        };
        return Ok(Some(active));
    }
    Ok(Some(name))
}

fn active_or_default_provider_name(
    cli: &Cli,
    apple_support: &AppleSupport,
) -> Result<Option<String>> {
    if let Some(active) = read_active_provider_name(cli)? {
        return Ok(Some(active));
    }
    Ok(default_live_provider_name(apple_support).map(ToOwned::to_owned))
}

fn default_live_provider_name(apple_support: &AppleSupport) -> Option<&'static str> {
    if apple_support.supported {
        Some("apple")
    } else {
        None
    }
}

fn available_provider_profiles(cli: &Cli) -> Result<BTreeMap<String, ProviderProfile>> {
    let mut profiles = BTreeMap::new();
    for name in ["apple", "openai"] {
        if let Some(profile) = builtin_provider_profile(name) {
            profiles.insert(name.to_owned(), profile);
        }
    }

    for (name, installed) in installed_providers(cli)? {
        profiles.insert(name, installed.profile());
    }

    if let Some(config_path) = provider_config_path(cli) {
        let content = std::fs::read_to_string(&config_path).with_context(|| {
            format!(
                "failed to read provider config from {}",
                config_path.display()
            )
        })?;
        let parsed: ProviderProfilesFile = toml::from_str(&content).with_context(|| {
            format!(
                "failed to parse provider config from {}",
                config_path.display()
            )
        })?;
        profiles.extend(parsed.providers);
    }

    Ok(profiles)
}

fn installed_provider(name: &str, cli: &Cli) -> Result<Option<InstalledProvider>> {
    Ok(installed_providers(cli)?.remove(name))
}

fn installed_providers(cli: &Cli) -> Result<BTreeMap<String, InstalledProvider>> {
    let Some(root) = provider_install_dir(cli) else {
        return Ok(BTreeMap::new());
    };
    if !root.exists() {
        return Ok(BTreeMap::new());
    }
    let mut providers = BTreeMap::new();
    for entry in fs::read_dir(&root).with_context(|| {
        format!(
            "failed to read provider install directory {}",
            root.display()
        )
    })? {
        let entry = entry?;
        let provider_root = entry.path();
        if !provider_root.is_dir() {
            continue;
        }
        let manifest_path = provider_root.join("provider.toml");
        if !manifest_path.is_file() {
            continue;
        }
        let manifest = read_provider_manifest(&provider_root).with_context(|| {
            format!(
                "failed to load installed provider from {}",
                provider_root.display()
            )
        })?;
        validate_provider_manifest(&provider_root, &manifest)?;
        providers.insert(
            manifest.id.clone(),
            InstalledProvider {
                install_metadata: read_provider_install_metadata(&provider_root),
                root: provider_root,
                manifest,
            },
        );
    }
    Ok(providers)
}

fn read_active_provider_name(cli: &Cli) -> Result<Option<String>> {
    let Some(path) = provider_state_path(cli) else {
        return Ok(None);
    };
    if !path.exists() {
        return Ok(None);
    }
    let content = std::fs::read_to_string(&path)
        .with_context(|| format!("failed to read provider state from {}", path.display()))?;
    let state: ActiveProviderState = serde_json::from_str(&content)
        .with_context(|| format!("failed to parse provider state from {}", path.display()))?;
    Ok(state
        .provider
        .and_then(|value| non_empty_string(Some(&value)).map(ToOwned::to_owned)))
}

fn write_active_provider_name(cli: &Cli, name: &str) -> Result<()> {
    let path = provider_state_path(cli).context(
        "could not determine provider state path; set DICTA_PROVIDER_STATE or HOME/XDG_CONFIG_HOME",
    )?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).with_context(|| {
            format!(
                "failed to create provider state directory {}",
                parent.display()
            )
        })?;
    }
    let state = ActiveProviderState {
        provider: Some(name.to_owned()),
        updated_at_ms: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_millis())
            .unwrap_or(0),
    };
    let content = serde_json::to_string_pretty(&state)?;
    std::fs::write(&path, format!("{content}\n"))
        .with_context(|| format!("failed to write provider state to {}", path.display()))
}

fn clear_active_provider_name(cli: &Cli) -> Result<()> {
    let Some(path) = provider_state_path(cli) else {
        return Ok(());
    };
    if path.exists() {
        fs::remove_file(&path)
            .with_context(|| format!("failed to remove provider state {}", path.display()))?;
    }
    Ok(())
}

fn builtin_provider_profile(name: &str) -> Option<ProviderProfile> {
    match name {
        "apple" => Some(ProviderProfile {
            kind: ProfileProviderKind::Apple,
            api_base: None,
            default_model: Some("apple".to_owned()),
            api_key: None,
            api_key_env: None,
            batch_file: None,
            streaming: None,
            requires_network: None,
            live_enabled: None,
            notes: vec!["Built-in Apple on-device profile.".to_owned()],
        }),
        "openai" => Some(ProviderProfile {
            kind: ProfileProviderKind::OpenaiCompatible,
            api_base: Some(default_openai_api_base().to_owned()),
            default_model: Some("whisper-1".to_owned()),
            api_key: None,
            api_key_env: Some("DICTA_ASR_API_KEY".to_owned()),
            batch_file: None,
            streaming: None,
            requires_network: None,
            live_enabled: Some(false),
            notes: vec!["Built-in OpenAI-compatible profile.".to_owned()],
        }),
        _ => None,
    }
}

fn provider_config_path(cli: &Cli) -> Option<PathBuf> {
    cli.provider_config
        .clone()
        .or_else(default_provider_config_path)
        .filter(|path| path.exists())
}

fn configured_provider_config_path(cli: &Cli) -> Option<PathBuf> {
    cli.provider_config
        .clone()
        .or_else(default_provider_config_path)
}

fn default_provider_config_path() -> Option<PathBuf> {
    Some(config_dir()?.join("providers.toml"))
}

fn provider_state_path(cli: &Cli) -> Option<PathBuf> {
    cli.provider_state
        .clone()
        .or_else(|| config_dir().map(|dir| dir.join("active-provider.json")))
}

fn provider_install_dir(cli: &Cli) -> Option<PathBuf> {
    cli.provider_dir
        .clone()
        .or_else(|| data_dir().map(|dir| dir.join("providers")))
}

fn config_dir() -> Option<PathBuf> {
    let base = if cfg!(windows) {
        env::var_os("APPDATA")
            .map(PathBuf::from)
            .or_else(|| env::var_os("USERPROFILE").map(|home| PathBuf::from(home).join(".config")))
    } else {
        env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
    }?;
    Some(base.join(CONFIG_DIR_NAME))
}

fn data_dir() -> Option<PathBuf> {
    if cfg!(target_os = "macos") {
        return env::var_os("HOME").map(PathBuf::from).map(|home| {
            home.join("Library")
                .join("Application Support")
                .join(APP_NAME)
        });
    }
    if cfg!(windows) {
        return env::var_os("APPDATA")
            .map(PathBuf::from)
            .or_else(|| env::var_os("USERPROFILE").map(PathBuf::from))
            .map(|base| base.join(APP_NAME));
    }
    env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/share")))
        .map(|base| base.join(APP_NAME))
}

fn temp_provider_staging_dir(providers_dir: &Path) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    providers_dir.join(format!(".install-{}-{nanos}", std::process::id()))
}

fn openai_compatible_api_base(cli: &Cli, profile: Option<&ResolvedProviderProfile>) -> String {
    non_empty(&cli.api_base)
        .or_else(|| {
            profile.and_then(|profile| {
                non_empty_string(profile.profile.api_base.as_deref()).map(ToOwned::to_owned)
            })
        })
        .unwrap_or_else(|| default_openai_api_base().to_owned())
}

fn openai_compatible_model(cli: &Cli, profile: Option<&ResolvedProviderProfile>) -> String {
    non_empty(&cli.api_model)
        .or_else(|| {
            profile.and_then(|profile| {
                non_empty_string(profile.profile.default_model.as_deref()).map(ToOwned::to_owned)
            })
        })
        .unwrap_or_else(|| default_model(AsrBackend::OpenaiCompatible).to_owned())
}

fn openai_compatible_api_key(
    cli: &Cli,
    profile: Option<&ResolvedProviderProfile>,
) -> Option<String> {
    non_empty(&cli.api_key).or_else(|| profile.and_then(profile_api_key))
}

fn configured_api_key(cli: &Cli, provider: Option<&EffectiveProvider>) -> Option<String> {
    non_empty(&cli.api_key).or_else(|| {
        provider
            .and_then(|provider| provider.profile.as_ref())
            .and_then(profile_api_key)
    })
}

fn profile_api_key(profile: &ResolvedProviderProfile) -> Option<String> {
    non_empty_string(profile.profile.api_key.as_deref())
        .map(ToOwned::to_owned)
        .or_else(|| {
            non_empty_string(profile.profile.api_key_env.as_deref())
                .and_then(|env_name| env::var(env_name).ok())
                .and_then(|value| non_empty_string(Some(&value)).map(ToOwned::to_owned))
        })
}

fn configured_api_base(cli: &Cli, provider: Option<&EffectiveProvider>) -> Option<String> {
    non_empty(&cli.api_base).or_else(|| {
        provider
            .filter(|provider| provider.backend == AsrBackend::OpenaiCompatible)
            .and_then(|provider| provider.profile.as_ref())
            .and_then(|profile| non_empty_string(profile.profile.api_base.as_deref()))
            .map(ToOwned::to_owned)
    })
}

#[derive(Debug, Clone)]
struct ExternalProvider {
    id: String,
    root: PathBuf,
    command: PathBuf,
    capabilities: ProviderCapabilities,
}

fn build_external_provider(cli: &Cli) -> Result<ExternalProvider> {
    let profile = resolve_provider_profile(cli)?
        .filter(|profile| profile.profile.kind == ProfileProviderKind::External)
        .context("external provider requires --provider <installed-provider>")?;
    let installed = profile
        .installed
        .context("external provider is not installed; run `dicta provider install <package>`")?;
    Ok(ExternalProvider {
        id: installed.id().to_owned(),
        root: installed.root.clone(),
        command: installed.command_path(),
        capabilities: installed.capabilities(),
    })
}

impl ExternalProvider {
    fn command(&self) -> tokio::process::Command {
        let mut command = tokio::process::Command::new(&self.command);
        command
            .current_dir(&self.root)
            .env("DICTA_PROVIDER_ID", &self.id)
            .env("DICTA_PROVIDER_ROOT", &self.root);
        command
    }
}

#[async_trait]
impl AsrProvider for ExternalProvider {
    async fn transcribe(
        &self,
        input: AudioInput,
        options: AsrOptions,
    ) -> dicta_asr::AsrResult<dicta_asr::Transcript> {
        let AudioInput::File(path) = input else {
            return Err(dicta_asr::AsrError::Input(
                "external providers currently require file input".to_owned(),
            ));
        };

        let mut command = self.command();
        command.arg("--input").arg(path).arg("--json");
        if let Some(language) = options.language {
            command.arg("--src").arg(language);
        }

        let output = command.output().await.map_err(|err| {
            dicta_asr::AsrError::Request(format!(
                "failed to run provider {} at {}: {err}",
                self.id,
                self.command.display()
            ))
        })?;
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);

        if !output.status.success() {
            return Err(dicta_asr::AsrError::Request(format!(
                "provider {} exited with {}: {}",
                self.id,
                output.status,
                stderr.trim()
            )));
        }

        parse_external_provider_jsonl(&stdout)
    }

    fn name(&self) -> &'static str {
        "external-provider"
    }

    fn capabilities(&self) -> AsrCapabilities {
        self.capabilities.batch.clone()
    }

    fn provider_capabilities(&self) -> ProviderCapabilities {
        self.capabilities.clone()
    }
}

#[async_trait]
impl LiveAsrProvider for ExternalProvider {
    async fn run_live(
        &self,
        options: LiveAsrOptions,
        on_event: LiveEventCallback<'_>,
    ) -> dicta_asr::AsrResult<()> {
        let mut command = self.command();
        if let Some(src) = options.src {
            command.arg("--src").arg(src);
        }
        if let Some(dst) = options.dst {
            command.arg("--dst").arg(dst);
        }
        command.arg("--json").arg("--event-json");
        if !options.mic {
            command.arg("--no-mic");
        }
        if !options.speaker {
            command.arg("--no-speaker");
        }
        if options.voice_processing {
            command.arg("--voice-processing");
        }
        if options.select_device {
            command.arg("--select-device");
        }

        let mut child = command
            .stdin(Stdio::inherit())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|err| {
                dicta_asr::AsrError::Request(format!(
                    "failed to run provider {} at {}: {err}",
                    self.id,
                    self.command.display()
                ))
            })?;
        let stdout = child.stdout.take().ok_or_else(|| {
            dicta_asr::AsrError::Request(format!("provider {} stdout was not piped", self.id))
        })?;
        let mut lines = BufReader::new(stdout).lines();
        let mut interrupted = false;
        let mut ctrl_c = Box::pin(tokio::signal::ctrl_c());
        let shutdown_timer = time::sleep(Duration::from_secs(0));
        tokio::pin!(shutdown_timer);
        let mut shutdown_requested = false;

        loop {
            tokio::select! {
                biased;
                signal = &mut ctrl_c, if !shutdown_requested => {
                    signal.map_err(|err| {
                        dicta_asr::AsrError::Request(format!("failed to listen for Ctrl-C: {err}"))
                    })?;
                    interrupted = true;
                    shutdown_requested = true;
                    shutdown_timer
                        .as_mut()
                        .reset(TokioInstant::now() + Duration::from_secs(30));
                    request_external_provider_shutdown(&mut child).await;
                }
                _ = &mut shutdown_timer, if shutdown_requested => {
                    let _ = child.start_kill();
                    on_event(LiveEvent::Eof)?;
                    break;
                }
                line = lines.next_line() => {
                    let Some(line) = line.map_err(|err| {
                        dicta_asr::AsrError::Request(format!("failed to read provider {} stdout: {err}", self.id))
                    })? else {
                        break;
                    };
                    let line = line.trim();
                    if line.is_empty() {
                        continue;
                    }
                    let event = parse_external_live_event(line)?;
                    on_event(event)?;
                }
            }
        }

        let status = child.wait().await.map_err(|err| {
            dicta_asr::AsrError::Request(format!(
                "failed to wait for provider {} at {}: {err}",
                self.id,
                self.command.display()
            ))
        })?;
        if !interrupted && !status.success() {
            return Err(dicta_asr::AsrError::Request(format!(
                "provider {} exited with {status}",
                self.id
            )));
        }
        Ok(())
    }

    fn live_name(&self) -> &'static str {
        "external-provider"
    }

    fn live_capabilities(&self) -> LiveCapabilities {
        self.capabilities
            .live
            .clone()
            .unwrap_or_else(|| live_capabilities_for_backend(AsrBackend::External))
    }
}

#[cfg(unix)]
async fn request_external_provider_shutdown(child: &mut tokio::process::Child) {
    if let Some(pid) = child.id() {
        let _ = tokio::process::Command::new("/bin/kill")
            .arg("-INT")
            .arg(pid.to_string())
            .status()
            .await;
    } else {
        let _ = child.start_kill();
    }
}

#[cfg(not(unix))]
async fn request_external_provider_shutdown(child: &mut tokio::process::Child) {
    let _ = child.start_kill();
}

fn parse_external_provider_jsonl(stdout: &str) -> dicta_asr::AsrResult<dicta_asr::Transcript> {
    let mut text = Vec::new();
    let mut language = None;

    for line in stdout.lines().filter(|line| !line.trim().is_empty()) {
        let value: serde_json::Value = serde_json::from_str(line)
            .map_err(|err| dicta_asr::AsrError::InvalidResponse(format!("{err}: {line}")))?;
        if let Some(src) = value.get("src") {
            if language.is_none() {
                language = src
                    .get("lang")
                    .and_then(serde_json::Value::as_str)
                    .map(ToOwned::to_owned);
            }
            if let Some(chunk) = src.get("text").and_then(serde_json::Value::as_str) {
                if !chunk.trim().is_empty() {
                    text.push(chunk.trim().to_owned());
                }
            }
            continue;
        }
        if let Some(chunk) = value.get("text").and_then(serde_json::Value::as_str) {
            if !chunk.trim().is_empty() {
                text.push(chunk.trim().to_owned());
            }
        }
        if language.is_none() {
            language = value
                .get("language")
                .and_then(serde_json::Value::as_str)
                .map(ToOwned::to_owned);
        }
    }

    if text.is_empty() {
        return Err(dicta_asr::AsrError::InvalidResponse(
            "external provider produced no transcript text".to_owned(),
        ));
    }

    Ok(dicta_asr::Transcript {
        text: text.join("\n"),
        language,
    })
}

fn parse_external_live_event(line: &str) -> dicta_asr::AsrResult<LiveEvent> {
    if let Ok(event) = serde_json::from_str::<LiveEvent>(line) {
        return Ok(event);
    }

    let event: TranscriptEvent = serde_json::from_str(line).map_err(|err| {
        dicta_asr::AsrError::InvalidResponse(format!("invalid provider JSONL event: {err}: {line}"))
    })?;
    Ok(LiveEvent::Finalized(event))
}

fn build_native_adapter(cli: &Cli) -> Result<NativeAdapterAsr> {
    let command = resolve_native_adapter(cli).context(
        "--asr apple requires --native-adapter, DICTA_NATIVE_ADAPTER, or a bundled native adapter binary next to dicta",
    )?;
    Ok(NativeAdapterAsr::new(NativeAdapterConfig { command })?)
}

fn resolve_native_adapter(cli: &Cli) -> Option<PathBuf> {
    if let Some(command) = resolve_configured_native_adapter(cli) {
        return Some(command.clone());
    }

    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?;
    bundled_native_adapter_names()
        .iter()
        .map(|name| dir.join(name))
        .find(|path| path.exists())
}

fn resolve_configured_native_adapter(cli: &Cli) -> Option<PathBuf> {
    cli.native_adapter.clone()
}

fn bundled_native_adapter_names() -> &'static [&'static str] {
    if cfg!(windows) {
        &[
            "dicta-adapter-apple-speech.exe",
            "dicta-adapter-windows-speech.exe",
        ]
    } else {
        &["dicta-adapter-apple-speech"]
    }
}

fn default_openai_api_base() -> &'static str {
    "https://api.openai.com"
}

fn non_empty(value: &Option<String>) -> Option<String> {
    value
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn non_empty_string(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|value| !value.is_empty())
}

fn validate_backend_config(backend: AsrBackend, cli: &Cli) -> Option<String> {
    match backend {
        AsrBackend::Apple if resolve_native_adapter(cli).is_none() => Some(
            "--asr apple requires --native-adapter, DICTA_NATIVE_ADAPTER, or a bundled native adapter binary next to dicta"
                .to_owned(),
        ),
        AsrBackend::External => resolve_provider_profile(cli)
            .ok()
            .flatten()
            .and_then(|profile| profile.installed)
            .and_then(|provider| {
                let command = provider.command_path();
                (!command.is_file()).then(|| {
                    format!(
                        "installed provider '{}' command does not exist: {}",
                        provider.id(),
                        command.display()
                    )
                })
            }),
        AsrBackend::OpenaiCompatible | AsrBackend::Apple => None,
        AsrBackend::Auto => None,
    }
}

fn validate_capability_config(
    backend: AsrBackend,
    cli: &Cli,
    apple_support: &AppleSupport,
) -> Option<String> {
    match backend {
        AsrBackend::Apple if !apple_support.supported => Some(format!(
            "apple ASR is unavailable: {}",
            apple_support.reason
        )),
        AsrBackend::Apple => validate_backend_config(backend, cli),
        AsrBackend::External => validate_backend_config(backend, cli),
        AsrBackend::OpenaiCompatible => None,
        AsrBackend::Auto => None,
    }
}

fn default_model(backend: AsrBackend) -> &'static str {
    match backend {
        AsrBackend::External => "external",
        _ => "whisper-1",
    }
}

fn default_model_for_name(backend: &str) -> &'static str {
    match backend {
        "apple" => "apple",
        "external" => "external",
        _ => "whisper-1",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn test_cli() -> Cli {
        Cli {
            asr: AsrBackend::Auto,
            api_base: None,
            api_key: None,
            api_model: None,
            provider: None,
            provider_config: None,
            src: None,
            dst: None,
            native_adapter: None,
            input: None,
            mic_duration: None,
            live: false,
            live_chunk: None,
            no_mic: false,
            no_speaker: false,
            voice_processing: false,
            select_device: false,
            json: false,
            transcript: None,
            doctor: false,
            capabilities: false,
            ui: false,
            provider_state: None,
            provider_dir: None,
            command: None,
        }
    }

    fn test_audio_report() -> AudioReport {
        AudioReport {
            default_input_available: false,
            name: None,
            sample_rate: None,
            channels: None,
            sample_format: None,
            error: Some("not probed during tests".to_owned()),
        }
    }

    fn write_temp_provider_config(contents: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "dicta-test-providers-{}-{:?}-{}.toml",
            std::process::id(),
            std::thread::current().id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::write(&path, contents).unwrap();
        path
    }

    fn temp_provider_state_path() -> PathBuf {
        std::env::temp_dir().join(format!(
            "dicta-test-active-provider-{}-{:?}-{}.json",
            std::process::id(),
            std::thread::current().id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    fn temp_test_dir(prefix: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "{prefix}-{}-{:?}-{}",
            std::process::id(),
            std::thread::current().id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    fn write_test_provider_source(root: &Path, id: &str, version: &str) {
        std::fs::create_dir_all(root.join("bin")).unwrap();
        std::fs::write(
            root.join("provider.toml"),
            format!(
                r#"
id = "{id}"
name = "Test ASR"
version = "{version}"
protocol = "dicta-provider-jsonl-v1"
command = "bin/{id}"
model = "{id}"
notes = ["Installed from a local directory."]

[batch]
file = true
streaming = false
requires_network = false

[live]
mode = "chunked"
mic = true
finalized_results = true
expected_latency_ms = 5000
"#
            ),
        )
        .unwrap();
        std::fs::write(root.join("bin").join(id), "#!/bin/sh\nexit 0\n").unwrap();
    }

    fn test_installed_provider(
        id: &str,
        install_metadata: Option<ProviderInstallMetadata>,
    ) -> InstalledProvider {
        InstalledProvider {
            root: temp_test_dir("dicta-installed-provider"),
            manifest: ProviderPackageManifest {
                id: id.to_owned(),
                name: Some("Test ASR".to_owned()),
                version: Some("0.1.0".to_owned()),
                protocol: PROVIDER_PROTOCOL.to_owned(),
                command: PathBuf::from(format!("bin/{id}")),
                model: Some(id.to_owned()),
                batch: ProviderBatchManifest::default(),
                live: None,
                notes: Vec::new(),
            },
            install_metadata,
        }
    }

    #[test]
    fn installed_binary_names_include_cli_tray_and_adapter() {
        let names = installed_binary_names();
        if cfg!(windows) {
            assert!(names.contains(&"dicta.exe"));
            assert!(names.contains(&"dicta-tray.exe"));
            assert!(names.contains(&"dicta-adapter-apple-speech.exe"));
        } else {
            assert!(names.contains(&"dicta"));
            assert!(names.contains(&"dicta-tray"));
            assert!(names.contains(&"dicta-adapter-apple-speech"));
        }
    }

    #[test]
    fn serve_command_uses_localhost_defaults() {
        let cli = Cli::try_parse_from(["dicta", "serve"]).unwrap();

        let Some(Command::Serve(command)) = cli.command else {
            panic!("expected serve command");
        };
        assert_eq!(command.host, IpAddr::V4(Ipv4Addr::LOCALHOST));
        assert_eq!(command.port, 4777);
        assert_eq!(command.max_upload_mb, 25);
        assert!(command.cors_origins.is_empty());
    }

    #[test]
    fn serve_command_accepts_cors_origins() {
        let cli = Cli::try_parse_from([
            "dicta",
            "serve",
            "--cors-origin",
            "http://localhost:3000,http://127.0.0.1:5173",
        ])
        .unwrap();

        let Some(Command::Serve(command)) = cli.command else {
            panic!("expected serve command");
        };
        assert_eq!(
            command.cors_origins,
            vec!["http://localhost:3000", "http://127.0.0.1:5173"]
        );
    }

    #[test]
    fn serve_response_format_supports_json_and_text_only() {
        assert_eq!(
            parse_serve_response_format(None).unwrap(),
            ServeResponseFormat::Json
        );
        assert_eq!(
            parse_serve_response_format(Some("json")).unwrap(),
            ServeResponseFormat::Json
        );
        assert_eq!(
            parse_serve_response_format(Some("text")).unwrap(),
            ServeResponseFormat::Text
        );

        let err = parse_serve_response_format(Some("verbose_json")).unwrap_err();
        assert_eq!(err.status, StatusCode::BAD_REQUEST);
        assert_eq!(err.param.as_deref(), Some("response_format"));
    }

    #[test]
    fn serve_model_alias_keeps_configured_provider_model() {
        assert_eq!(serve_model_override("dicta"), None);
        assert_eq!(serve_model_override("default"), None);
        assert_eq!(
            serve_model_override(" whisper-1 ").as_deref(),
            Some("whisper-1")
        );
    }

    #[test]
    fn serve_bool_parser_matches_form_values() {
        assert!(parse_form_bool("true", "stream").unwrap());
        assert!(parse_form_bool("1", "stream").unwrap());
        assert!(!parse_form_bool("false", "stream").unwrap());
        assert!(!parse_form_bool("0", "stream").unwrap());

        let err = parse_form_bool("maybe", "stream").unwrap_err();
        assert_eq!(err.param.as_deref(), Some("stream"));
    }

    #[test]
    fn serve_upload_extension_is_sanitized() {
        assert_eq!(
            safe_upload_extension("recording.wav").as_deref(),
            Some("wav")
        );
        assert_eq!(
            safe_upload_extension("recording.w@v").as_deref(),
            Some("wv")
        );
        assert!(safe_upload_extension("recording").is_none());
    }

    #[test]
    fn serve_rejects_zero_upload_limit() {
        assert!(max_upload_bytes(0).is_err());
        assert_eq!(max_upload_bytes(2).unwrap(), 2 * 1024 * 1024);
    }

    #[test]
    fn update_installer_command_has_fetch_fallbacks() {
        let command = update_installer_command();
        assert!(command.contains("curl"));
        assert!(command.contains("wget"));
        assert!(command.contains("install.sh"));
        assert!(command.contains("sh \"$tmp\""));
    }

    #[test]
    fn uninstall_confirmation_accepts_y_or_yes_only() {
        assert!(is_uninstall_confirmation("y"));
        assert!(is_uninstall_confirmation("Y"));
        assert!(is_uninstall_confirmation("yes"));
        assert!(is_uninstall_confirmation(" yes\n"));
        assert!(!is_uninstall_confirmation(""));
        assert!(!is_uninstall_confirmation("n"));
        assert!(!is_uninstall_confirmation("no"));
    }

    #[test]
    fn auto_uses_openai_compatible_when_apple_on_device_is_unavailable() {
        let cli = Cli {
            input: Some(PathBuf::from("audio.wav")),
            ..test_cli()
        };
        let support = AppleSupport {
            version: Some("15.6.1".to_owned()),
            supported: false,
            reason: "macOS 15.6.1 is below 26".to_owned(),
        };

        assert_eq!(
            resolve_backend_for(&cli, &support).unwrap(),
            AsrBackend::OpenaiCompatible
        );
    }

    #[test]
    fn auto_keeps_http_provider_until_native_adapter_is_connected() {
        let cli = Cli {
            input: Some(PathBuf::from("audio.wav")),
            ..test_cli()
        };
        let support = AppleSupport {
            version: Some("26.0.0".to_owned()),
            supported: true,
            reason: "macOS 26+ detected".to_owned(),
        };

        assert_eq!(
            resolve_backend_for(&cli, &support).unwrap(),
            AsrBackend::OpenaiCompatible
        );
    }

    #[test]
    fn auto_uses_openai_compatible_when_model_is_configured() {
        let cli = Cli {
            api_base: Some("https://example.com".to_owned()),
            api_model: Some("whisper-1".to_owned()),
            input: Some(PathBuf::from("audio.wav")),
            ..test_cli()
        };
        let support = AppleSupport {
            version: Some("15.6.1".to_owned()),
            supported: false,
            reason: "macOS 15.6.1 is below 26".to_owned(),
        };

        assert_eq!(
            resolve_backend_for(&cli, &support).unwrap(),
            AsrBackend::OpenaiCompatible
        );
    }

    #[test]
    fn apple_backend_requires_platform_support() {
        let cli = Cli {
            asr: AsrBackend::Apple,
            input: Some(PathBuf::from("audio.wav")),
            ..test_cli()
        };
        let support = AppleSupport {
            version: Some("15.6.1".to_owned()),
            supported: false,
            reason: "macOS 15.6.1 is below 26".to_owned(),
        };

        assert!(resolve_backend_for(&cli, &support).is_err());
    }

    #[test]
    fn apple_backend_resolves_when_platform_support_is_available() {
        let cli = Cli {
            asr: AsrBackend::Apple,
            native_adapter: Some(PathBuf::from("dicta-adapter-apple-speech")),
            input: Some(PathBuf::from("audio.wav")),
            ..test_cli()
        };
        let support = AppleSupport {
            version: Some("26.0.0".to_owned()),
            supported: true,
            reason: "macOS 26+ detected".to_owned(),
        };

        assert_eq!(
            resolve_backend_for(&cli, &support).unwrap(),
            AsrBackend::Apple
        );
        assert!(build_provider(AsrBackend::Apple, &cli).is_ok());
    }

    #[test]
    fn doctor_reports_missing_native_adapter() {
        let cli = Cli {
            asr: AsrBackend::Apple,
            json: true,
            doctor: true,
            ..test_cli()
        };
        let support = AppleSupport {
            version: Some("26.0.0".to_owned()),
            supported: true,
            reason: "macOS 26+ detected".to_owned(),
        };
        let backend = resolve_backend_for(&cli, &support).unwrap();

        assert_eq!(backend, AsrBackend::Apple);
        assert!(
            validate_backend_config(backend, &cli)
                .as_deref()
                .is_some_and(|error| error.contains("--native-adapter"))
        );
    }

    #[test]
    fn rejects_missing_audio_source() {
        let cli = test_cli();

        assert!(resolve_audio_source(&cli).is_err());
    }

    #[test]
    fn rejects_input_and_mic_duration_together() {
        let cli = Cli {
            input: Some(PathBuf::from("audio.wav")),
            mic_duration: Some(1.0),
            ..test_cli()
        };

        assert!(resolve_audio_source(&cli).is_err());
    }

    #[test]
    fn batch_mode_rejects_live_only_translation_target() {
        let cli = Cli {
            dst: Some("en-US".to_owned()),
            ..test_cli()
        };

        assert!(
            validate_batch_options(&cli)
                .unwrap_err()
                .to_string()
                .contains("--dst")
        );
    }

    #[test]
    fn batch_mode_rejects_live_only_capture_flags() {
        let cli = Cli {
            select_device: true,
            ..test_cli()
        };

        assert!(
            validate_batch_options(&cli)
                .unwrap_err()
                .to_string()
                .contains("--select-device")
        );
    }

    #[test]
    fn live_mode_rejects_openai_compatible_backend() {
        let cli = Cli {
            live: true,
            asr: AsrBackend::OpenaiCompatible,
            ..test_cli()
        };

        assert!(
            resolve_live_backend(&cli)
                .unwrap_err()
                .to_string()
                .contains("installed live provider")
        );
    }

    #[test]
    fn live_mode_rejects_batch_only_provider_profile() {
        let cli = Cli {
            live: true,
            provider: Some("openai".to_owned()),
            ..test_cli()
        };
        let support = AppleSupport {
            version: Some("15.6.1".to_owned()),
            supported: false,
            reason: "macOS 15.6.1 is below 26".to_owned(),
        };

        assert!(
            resolve_live_backend_for(&cli, &support)
                .unwrap_err()
                .to_string()
                .contains("does not support live mode")
        );
    }

    #[test]
    fn missing_batch_input_defaults_to_live_mode() {
        let cli = test_cli();

        assert!(should_run_live(&cli));
    }

    #[test]
    fn input_uses_batch_mode() {
        let cli = Cli {
            input: Some(PathBuf::from("audio.wav")),
            ..test_cli()
        };

        assert!(!should_run_live(&cli));
    }

    #[test]
    fn live_mode_rejects_disabling_all_capture_sources() {
        let cli = Cli {
            live: true,
            asr: AsrBackend::Apple,
            no_mic: true,
            no_speaker: true,
            ..test_cli()
        };

        assert!(
            validate_live_options(&cli, "apple", &native_adapter_live_capabilities())
                .unwrap_err()
                .to_string()
                .contains("--no-mic")
        );
    }

    #[test]
    fn streaming_live_rejects_chunk_duration() {
        let cli = Cli {
            live: true,
            asr: AsrBackend::Apple,
            live_chunk: Some(3.0),
            ..test_cli()
        };

        assert!(
            validate_live_options(&cli, "apple", &native_adapter_live_capabilities())
                .unwrap_err()
                .to_string()
                .contains("--live-chunk")
        );
    }

    #[test]
    fn doctor_reports_no_python_sidecar_requirement() {
        let cli = Cli {
            json: true,
            doctor: true,
            ..test_cli()
        };

        let report = gather_doctor_report_with_audio(&cli, test_audio_report());
        assert!(!report.runtime.python_sidecar_required);
        assert!(report.runtime.web_direct_available);
        assert!(report.live.mode.is_some() || report.live.error.is_some());
        assert_eq!(report.system.os, std::env::consts::OS);
        if std::env::consts::OS == "macos" {
            assert!(report.system.apple_on_device_reason.contains("macOS"));
        }
    }

    #[test]
    fn capabilities_report_marks_openai_compatible_as_batch_only() {
        let cli = Cli {
            asr: AsrBackend::OpenaiCompatible,
            capabilities: true,
            ..test_cli()
        };
        let support = AppleSupport {
            version: Some("15.6.1".to_owned()),
            supported: false,
            reason: "macOS 15.6.1 is below 26".to_owned(),
        };

        let report = gather_capabilities_report_with_support(&cli, &support);

        assert_eq!(report.resolved.as_deref(), Some("openai-compatible"));
        assert!(report.local_config_ok);
        assert!(report.batch.as_ref().is_some_and(|batch| {
            batch.batch_file && !batch.streaming && batch.requires_network
        }));
        assert!(report.live.is_none());
    }

    #[test]
    fn provider_profile_resolves_openai_compatible_from_config() {
        let config = write_temp_provider_config(
            r#"
[providers.siliconflow]
kind = "openai-compatible"
api_base = "https://api.siliconflow.cn"
default_model = "FunAudioLLM/SenseVoiceSmall"
api_key_env = "SILICONFLOW_API_KEY"
notes = ["SiliconFlow OpenAI-compatible profile."]
batch_file = true
streaming = false
requires_network = true
live_enabled = false
"#,
        );
        let cli = Cli {
            provider: Some("siliconflow".to_owned()),
            provider_config: Some(config.clone()),
            capabilities: true,
            ..test_cli()
        };
        let support = AppleSupport {
            version: Some("15.6.1".to_owned()),
            supported: false,
            reason: "macOS 15.6.1 is below 26".to_owned(),
        };

        let backend = resolve_backend_for(&cli, &support).unwrap();
        let report = gather_capabilities_report_with_support(&cli, &support);
        let _ = std::fs::remove_file(config);

        assert_eq!(backend, AsrBackend::OpenaiCompatible);
        assert_eq!(report.provider.as_deref(), Some("siliconflow"));
        assert_eq!(report.provider_kind.as_deref(), Some("openai-compatible"));
        assert_eq!(report.model, "FunAudioLLM/SenseVoiceSmall");
        assert!(report.local_config_ok);
        assert!(report.batch.as_ref().is_some_and(|batch| {
            batch.batch_file && !batch.streaming && batch.requires_network
        }));
        assert!(report.live.is_none());
        assert!(report.notes.iter().any(|note| note.contains("SiliconFlow")));
    }

    #[test]
    fn provider_profile_cannot_enable_live_beyond_implementation() {
        let config = write_temp_provider_config(
            r#"
[providers.bad-live]
kind = "openai-compatible"
api_base = "https://example.com"
default_model = "model"
live_enabled = true
"#,
        );
        let cli = Cli {
            provider: Some("bad-live".to_owned()),
            provider_config: Some(config.clone()),
            capabilities: true,
            ..test_cli()
        };
        let support = AppleSupport {
            version: Some("15.6.1".to_owned()),
            supported: false,
            reason: "macOS 15.6.1 is below 26".to_owned(),
        };

        let report = gather_capabilities_report_with_support(&cli, &support);
        let _ = std::fs::remove_file(config);

        assert_eq!(report.resolved.as_deref(), Some("openai-compatible"));
        assert_eq!(report.provider.as_deref(), Some("bad-live"));
        assert!(!report.local_config_ok);
        assert!(
            report
                .local_config_error
                .as_deref()
                .is_some_and(|error| error.contains("does not support live mode"))
        );
        assert!(report.live.is_none());
    }

    #[test]
    fn provider_profile_accepts_direct_api_key() {
        let config = write_temp_provider_config(
            r#"
[providers.local-key]
kind = "openai-compatible"
api_base = "https://example.com"
default_model = "model"
api_key = "profile-secret"
live_enabled = false
"#,
        );
        let cli = Cli {
            provider: Some("local-key".to_owned()),
            provider_config: Some(config.clone()),
            doctor: true,
            ..test_cli()
        };

        let report = gather_doctor_report_with_audio(&cli, test_audio_report());
        let _ = std::fs::remove_file(config);

        assert_eq!(
            report.backend.resolved.as_deref(),
            Some("openai-compatible")
        );
        assert_eq!(report.backend.provider.as_deref(), Some("local-key"));
        assert!(report.backend.api_key_configured);
        assert!(report.backend.api_base_configured);
    }

    #[test]
    fn active_provider_state_resolves_provider_profile() {
        let source_dir = temp_test_dir("dicta-provider-source");
        let provider_dir = temp_test_dir("dicta-provider-install");
        let state = temp_provider_state_path();
        write_test_provider_source(&source_dir, "local-asr", "0.1.0");
        let cli = Cli {
            provider: Some("active".to_owned()),
            provider_state: Some(state.clone()),
            provider_dir: Some(provider_dir.clone()),
            capabilities: true,
            ..test_cli()
        };
        let source = ProviderInstallSource {
            description: source_dir.display().to_string(),
            package: ProviderInstallPackage::Directory(source_dir.clone()),
            metadata: provider_install_metadata(
                ProviderInstallSourceKind::Directory,
                None,
                None,
                None,
            ),
        };
        install_provider_from_source(&cli, source, false).unwrap();
        write_active_provider_name(&cli, "local-asr").unwrap();
        let support = AppleSupport {
            version: Some("15.6.1".to_owned()),
            supported: false,
            reason: "macOS 15.6.1 is below 26".to_owned(),
        };

        let report = gather_capabilities_report_with_support(&cli, &support);
        let _ = std::fs::remove_dir_all(source_dir);
        let _ = std::fs::remove_dir_all(provider_dir);
        let _ = std::fs::remove_file(state);

        assert_eq!(report.provider.as_deref(), Some("local-asr"));
        assert_eq!(report.resolved.as_deref(), Some("external"));
        assert!(report.live.is_some());
    }

    #[test]
    fn active_provider_has_no_default_when_state_is_missing_and_apple_is_unavailable() {
        let state = temp_provider_state_path();
        let cli = Cli {
            provider: Some("active".to_owned()),
            provider_state: Some(state),
            capabilities: true,
            ..test_cli()
        };
        let support = AppleSupport {
            version: Some("15.6.1".to_owned()),
            supported: false,
            reason: "macOS 15.6.1 is below 26".to_owned(),
        };

        let report = gather_capabilities_report_with_support(&cli, &support);

        assert_eq!(report.provider, None);
        assert_eq!(report.resolved, None);
        assert!(!report.local_config_ok);
        assert!(
            report
                .resolution_error
                .as_deref()
                .is_some_and(|error| error.contains("no active provider is set"))
        );
    }

    #[test]
    fn active_provider_defaults_to_apple_when_state_is_missing_and_apple_is_available() {
        let state = temp_provider_state_path();
        let cli = Cli {
            provider: Some("active".to_owned()),
            provider_state: Some(state),
            native_adapter: Some(PathBuf::from("dicta-adapter-apple-speech")),
            capabilities: true,
            ..test_cli()
        };
        let support = AppleSupport {
            version: Some("26.0.0".to_owned()),
            supported: true,
            reason: "macOS 26+ detected".to_owned(),
        };

        let report = gather_capabilities_report_with_support(&cli, &support);

        assert_eq!(report.provider.as_deref(), Some("apple"));
        assert_eq!(report.resolved.as_deref(), Some("apple"));
        assert!(report.local_config_ok);
        assert!(report.live.is_some());
    }

    #[test]
    fn provider_list_includes_builtins_and_custom_profiles() {
        let config = write_temp_provider_config(
            r#"
[providers.siliconflow]
kind = "openai-compatible"
api_base = "https://api.siliconflow.cn"
default_model = "FunAudioLLM/SenseVoiceSmall"
api_key_env = "SILICONFLOW_API_KEY"
live_enabled = false
"#,
        );
        let state = temp_provider_state_path();
        let cli = Cli {
            provider_config: Some(config.clone()),
            provider_state: Some(state.clone()),
            ..test_cli()
        };
        write_active_provider_name(&cli, "siliconflow").unwrap();

        let report = gather_provider_list_report(&cli).unwrap();
        let _ = std::fs::remove_file(config);
        let _ = std::fs::remove_file(state);

        assert_eq!(report.current.as_deref(), Some("siliconflow"));
        assert!(
            report
                .providers
                .iter()
                .any(|provider| provider.name == "openai" && provider.built_in)
        );
        assert!(
            report
                .providers
                .iter()
                .any(|provider| provider.name == "siliconflow"
                    && provider.selected
                    && !provider.built_in)
        );
    }

    #[tokio::test]
    async fn provider_install_copies_local_provider_without_node_modules() {
        let source = temp_test_dir("dicta-provider-source");
        let provider_dir = temp_test_dir("dicta-provider-install");
        write_test_provider_source(&source, "local-asr", "0.1.0");

        let cli = Cli {
            provider_dir: Some(provider_dir.clone()),
            ..test_cli()
        };
        let command = ProviderInstallCommand {
            package: source.display().to_string(),
            version: None,
            registry: DEFAULT_NPM_REGISTRY.to_owned(),
            force: false,
        };

        run_provider_install(&cli, &command).await.unwrap();
        let installed_root = provider_dir.join("local-asr");
        assert!(installed_root.join("provider.toml").is_file());
        assert!(installed_root.join("bin/local-asr").is_file());
        assert!(
            installed_root
                .join(PROVIDER_INSTALL_METADATA_FILE)
                .is_file()
        );
        assert!(!provider_dir.join("node_modules").exists());

        let metadata = read_provider_install_metadata(&installed_root).unwrap();
        assert_eq!(metadata.source, ProviderInstallSourceKind::Directory);
        assert_eq!(metadata.package, None);
        assert_eq!(metadata.registry, None);
        assert_eq!(metadata.version, None);

        let report = gather_provider_list_report(&cli).unwrap();
        let installed = report
            .providers
            .iter()
            .find(|provider| provider.name == "local-asr")
            .expect("installed provider listed");
        assert!(installed.installed);
        assert!(!installed.built_in);
        assert_eq!(installed.kind, "external");
        assert_eq!(installed.model, "local-asr");
        assert_eq!(installed.installed_version.as_deref(), Some("0.1.0"));
        assert_eq!(installed.source_package, None);
        assert!(installed.live);

        let _ = std::fs::remove_dir_all(source);
        let _ = std::fs::remove_dir_all(provider_dir);
    }

    #[test]
    fn provider_remove_deletes_installed_provider_and_clears_active_state() {
        let source_dir = temp_test_dir("dicta-provider-source");
        let provider_dir = temp_test_dir("dicta-provider-install");
        let state = temp_provider_state_path();
        write_test_provider_source(&source_dir, "local-asr", "0.1.0");

        let cli = Cli {
            provider_dir: Some(provider_dir.clone()),
            provider_state: Some(state.clone()),
            ..test_cli()
        };
        let source = ProviderInstallSource {
            description: source_dir.display().to_string(),
            package: ProviderInstallPackage::Directory(source_dir.clone()),
            metadata: provider_install_metadata(
                ProviderInstallSourceKind::Directory,
                None,
                None,
                None,
            ),
        };
        install_provider_from_source(&cli, source, false).unwrap();
        write_active_provider_name(&cli, "local-asr").unwrap();

        run_provider_remove(
            &cli,
            &ProviderRemoveCommand {
                name: "local-asr".to_owned(),
                yes: true,
            },
        )
        .unwrap();

        assert!(!provider_dir.join("local-asr").exists());
        assert_eq!(read_active_provider_name(&cli).unwrap(), None);

        let _ = std::fs::remove_dir_all(source_dir);
        let _ = std::fs::remove_dir_all(provider_dir);
        let _ = std::fs::remove_file(state);
    }

    #[test]
    fn provider_update_uses_npm_metadata_and_skips_local_sources() {
        let npm_provider = test_installed_provider(
            "example-asr",
            Some(provider_install_metadata(
                ProviderInstallSourceKind::Npm,
                Some("@dicta-asr/example-asr".to_owned()),
                Some(DEFAULT_NPM_REGISTRY.to_owned()),
                Some("0.1.0".to_owned()),
            )),
        );
        let local_provider = test_installed_provider(
            "local-asr",
            Some(provider_install_metadata(
                ProviderInstallSourceKind::Directory,
                None,
                None,
                None,
            )),
        );
        let legacy_provider = test_installed_provider("legacy-asr", None);

        assert_eq!(
            provider_update_package(&npm_provider).as_deref(),
            Some("@dicta-asr/example-asr")
        );
        assert_eq!(provider_update_package(&local_provider), None);
        assert_eq!(
            provider_update_package(&legacy_provider).as_deref(),
            Some("@dicta-asr/legacy-asr")
        );
    }

    #[test]
    fn provider_package_names_use_logical_scope_and_npm_platform_terms() {
        assert_eq!(
            canonical_provider_package_name("example-asr"),
            "@dicta-asr/example-asr"
        );
        assert_eq!(
            canonical_provider_package_name("@dicta-asr/example-asr"),
            "@dicta-asr/example-asr"
        );
        assert_eq!(
            provider_id_from_package_name("@dicta-asr/example-asr"),
            "example-asr"
        );
        assert!(is_probable_provider_platform_package(
            "@dicta-asr/example-asr",
            "@dicta-asr/example-asr-darwin-arm64"
        ));
        assert!(!is_probable_provider_platform_package(
            "@dicta-asr/example-asr",
            "@dicta-asr/other-asr-darwin-arm64"
        ));
        assert_ne!(npm_current_os(), "macos");
        assert_ne!(npm_current_cpu(), "aarch64");
    }

    #[test]
    fn npm_platform_constraints_follow_current_platform() {
        let current_os = npm_current_os().to_owned();
        let current_cpu = npm_current_cpu().to_owned();

        assert!(npm_list_allows(&[], &current_os));
        assert!(npm_list_allows(
            std::slice::from_ref(&current_os),
            &current_os
        ));
        assert!(!npm_list_allows(&[format!("!{current_os}")], &current_os));
        assert!(!npm_list_allows(
            &["definitely-other".to_owned()],
            &current_os
        ));

        let package = NpmPackageVersion {
            os: vec![current_os],
            cpu: vec![current_cpu],
            optional_dependencies: BTreeMap::new(),
            dist: NpmPackageDist {
                tarball: "https://registry.npmjs.org/example/-/example.tgz".to_owned(),
                integrity: None,
            },
        };
        assert!(npm_package_version_matches_current_platform(&package));
    }

    #[test]
    fn current_provider_reports_none_when_state_is_missing_and_no_platform_default_exists() {
        let state = temp_provider_state_path();
        let cli = Cli {
            provider_state: Some(state),
            ..test_cli()
        };

        let report = gather_current_provider_report(&cli).unwrap();

        assert_eq!(report.provider, None);
        assert!(!report.live);
        assert!(
            report
                .local_config_error
                .as_deref()
                .is_some_and(|error| error.contains("no active provider is set"))
        );
    }

    #[test]
    fn builtin_openai_profile_uses_openai_compatible_defaults() {
        let cli = Cli {
            provider: Some("openai".to_owned()),
            capabilities: true,
            ..test_cli()
        };
        let support = AppleSupport {
            version: Some("15.6.1".to_owned()),
            supported: false,
            reason: "macOS 15.6.1 is below 26".to_owned(),
        };

        let report = gather_capabilities_report_with_support(&cli, &support);

        assert_eq!(report.resolved.as_deref(), Some("openai-compatible"));
        assert_eq!(report.provider.as_deref(), Some("openai"));
        assert_eq!(report.model, "whisper-1");
        assert!(report.local_config_ok);
    }

    #[test]
    fn capabilities_report_surfaces_native_adapter_config_error() {
        let cli = Cli {
            asr: AsrBackend::Apple,
            capabilities: true,
            ..test_cli()
        };
        let support = AppleSupport {
            version: Some("26.0.0".to_owned()),
            supported: true,
            reason: "macOS 26+ detected".to_owned(),
        };

        let report = gather_capabilities_report_with_support(&cli, &support);

        assert_eq!(report.resolved.as_deref(), Some("apple"));
        assert!(!report.local_config_ok);
        assert!(
            report
                .local_config_error
                .as_deref()
                .is_some_and(|error| error.contains("--native-adapter"))
        );
        assert!(report.live.as_ref().is_some_and(|live| live.translation));
    }

    #[test]
    fn capabilities_report_keeps_apple_capabilities_when_platform_is_unavailable() {
        let cli = Cli {
            asr: AsrBackend::Apple,
            capabilities: true,
            ..test_cli()
        };
        let support = AppleSupport {
            version: Some("15.6.1".to_owned()),
            supported: false,
            reason: "macOS 15.6.1 is below 26".to_owned(),
        };

        let report = gather_capabilities_report_with_support(&cli, &support);

        assert_eq!(report.resolved.as_deref(), Some("apple"));
        assert!(!report.local_config_ok);
        assert!(report.resolution_error.is_none());
        assert!(
            report
                .local_config_error
                .as_deref()
                .is_some_and(|error| error.contains("unavailable"))
        );
        assert!(report.batch.as_ref().is_some_and(|batch| batch.batch_file));
        assert!(report.live.as_ref().is_some_and(|live| live.translation));
    }

    #[test]
    fn empty_api_values_are_not_treated_as_configured() {
        let cli = Cli {
            asr: AsrBackend::OpenaiCompatible,
            api_base: Some("   ".to_owned()),
            api_key: Some("".to_owned()),
            api_model: Some("  ".to_owned()),
            json: true,
            doctor: true,
            ..test_cli()
        };

        let report = gather_doctor_report_with_audio(&cli, test_audio_report());

        assert!(!report.backend.api_base_configured);
        assert!(!report.backend.api_key_configured);
        assert_eq!(report.backend.model, "whisper-1");
    }

    #[test]
    fn writes_plain_transcript_file() {
        let path = std::env::temp_dir().join(format!(
            "dicta-test-transcript-{}.txt",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let payload = OutputPayload::new(
            dicta_asr::Transcript {
                text: "hello".to_owned(),
                language: Some("en-US".to_owned()),
            },
            AudioChannel::File,
            None,
        );

        write_transcript(&path, &payload, false).unwrap();
        let written = std::fs::read_to_string(&path).unwrap();
        let _ = std::fs::remove_file(&path);

        assert_eq!(written, "hello\n");
    }

    #[test]
    fn rejects_transcript_path_when_parent_is_missing() {
        let path = std::env::temp_dir()
            .join("dicta-definitely-missing-dir")
            .join("out.txt");

        assert!(validate_transcript_path(&path).is_err());
    }

    fn test_event(channel: AudioChannel, src: &str, dst: Option<&str>) -> TranscriptEvent {
        test_event_with_lang(channel, src, "en-US", dst, "ja-JP")
    }

    fn test_event_with_lang(
        channel: AudioChannel,
        src: &str,
        src_lang: &str,
        dst: Option<&str>,
        dst_lang: &str,
    ) -> TranscriptEvent {
        TranscriptEvent {
            seq: 7,
            channel,
            timestamp: EventTimestamp::from_unix_second(1_700_000_000).unwrap(),
            audio: None,
            src: TranscriptSource {
                lang: src_lang.to_owned(),
                text: src.to_owned(),
                confidence: None,
            },
            dst: dst.map(|text| TranscriptTarget {
                lang: dst_lang.to_owned(),
                text: text.to_owned(),
            }),
        }
    }

    #[test]
    fn live_renderer_formats_channel_labels() {
        let renderer = LiveRenderer::new(
            true,
            None,
            true,
            Some("en-US".to_owned()),
            Some("ja-JP".to_owned()),
        )
        .unwrap();
        let lines = renderer.tty_lines(test_event(
            AudioChannel::Speaker,
            "hello",
            Some("こんにちは"),
        ));

        assert_eq!(lines.len(), 2);
        assert!(lines[0].contains("[spk]"));
        assert!(lines[0].contains("\x1b[38;5;24m"));
        assert!(lines[0].ends_with("hello"));
        assert!(lines[1].contains("\x1b[38;5;244m"));
        assert!(lines[1].contains("こんにちは"));
    }

    #[test]
    fn live_renderer_suppresses_passthrough_translation_line() {
        let renderer =
            LiveRenderer::new(true, None, false, Some("en-US".to_owned()), None).unwrap();
        let lines = renderer.tty_lines(test_event_with_lang(
            AudioChannel::Mic,
            "same",
            "ja-JP",
            Some("same"),
            "ja",
        ));

        assert_eq!(lines.len(), 1);
    }

    #[test]
    fn live_renderer_keeps_same_text_cross_language_translation_line() {
        let renderer =
            LiveRenderer::new(true, None, false, Some("en-US".to_owned()), None).unwrap();
        let lines = renderer.tty_lines(test_event_with_lang(
            AudioChannel::Mic,
            "OK",
            "en-US",
            Some("OK"),
            "ja-JP",
        ));

        assert_eq!(lines.len(), 2);
    }

    #[test]
    fn live_renderer_appends_transcript_jsonl() {
        let path = std::env::temp_dir().join(format!(
            "dicta-live-renderer-{}.jsonl",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let mut renderer = LiveRenderer::new(
            true,
            Some(path.clone()),
            false,
            Some("en-US".to_owned()),
            None,
        )
        .unwrap();
        renderer
            .emit_event(&test_event(AudioChannel::Mic, "hello", None))
            .unwrap();
        drop(renderer);

        let written = std::fs::read_to_string(&path).unwrap();
        let _ = std::fs::remove_file(&path);
        assert!(written.contains(r#""seq":7"#));
        assert!(written.contains(r#""text":"hello""#));
    }

    #[test]
    fn live_renderer_keeps_status_out_of_transcript_log() {
        let path = std::env::temp_dir().join(format!(
            "dicta-live-renderer-status-{}.jsonl",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let mut renderer = LiveRenderer::new(
            true,
            Some(path.clone()),
            false,
            Some("en-US".to_owned()),
            None,
        )
        .unwrap();

        renderer
            .handle_live_event(LiveEvent::Status(LiveStatusEvent {
                phase: dicta_core::LiveStatusPhase::Recording,
                message: "recording 3s chunk".to_owned(),
                detail: None,
            }))
            .unwrap();
        renderer
            .emit_event(&test_event(AudioChannel::Mic, "hello", None))
            .unwrap();
        drop(renderer);

        let written = std::fs::read_to_string(&path).unwrap();
        let _ = std::fs::remove_file(&path);
        assert!(!written.contains(r#""type":"status""#));
        assert!(!written.contains("recording 3s chunk"));
        assert!(written.contains(r#""text":"hello""#));
    }

    #[test]
    fn status_text_includes_non_empty_detail() {
        let status = LiveStatusEvent {
            phase: dicta_core::LiveStatusPhase::Recovering,
            message: "chunk failed; continuing".to_owned(),
            detail: Some("network timeout".to_owned()),
        };

        assert_eq!(
            status_text(&status),
            "chunk failed; continuing: network timeout"
        );
    }

    #[test]
    fn live_renderer_finalizes_explicit_transcript() {
        let path = std::env::temp_dir().join(format!(
            "dicta-live-renderer-finalize-{}.jsonl",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let mut renderer = LiveRenderer::new(
            false,
            Some(path.clone()),
            false,
            Some("en-US".to_owned()),
            None,
        )
        .unwrap();
        renderer
            .emit_event(&test_event(AudioChannel::Mic, "hello", None))
            .unwrap();
        renderer.finalize_session_log().unwrap();

        let written = std::fs::read_to_string(&path).unwrap();
        let _ = std::fs::remove_file(&path);
        assert!(written.contains(r#""text":"hello""#));
    }

    #[test]
    fn live_renderer_commits_pending_translation_with_event_language() {
        let path = std::env::temp_dir().join(format!(
            "dicta-live-renderer-translation-{}.jsonl",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let mut renderer = LiveRenderer::new(
            true,
            Some(path.clone()),
            false,
            Some("en-US".to_owned()),
            Some("ja-JP".to_owned()),
        )
        .unwrap();

        renderer
            .handle_live_event(LiveEvent::Finalized(test_event(
                AudioChannel::Mic,
                "hello",
                None,
            )))
            .unwrap();
        let before_translation = std::fs::read_to_string(&path).unwrap();
        assert!(before_translation.is_empty());

        renderer
            .handle_live_event(LiveEvent::Translated(LiveTranslatedEvent {
                seq: 7,
                lang: Some("zh-CN".to_owned()),
                text: "你好".to_owned(),
            }))
            .unwrap();
        drop(renderer);

        let written = std::fs::read_to_string(&path).unwrap();
        let _ = std::fs::remove_file(&path);
        assert!(written.contains(r#""text":"hello""#));
        assert!(written.contains(r#""lang":"zh-CN""#));
        assert!(written.contains(r#""text":"你好""#));
    }
}
