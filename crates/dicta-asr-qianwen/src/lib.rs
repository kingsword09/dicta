use async_trait::async_trait;
use dicta_asr::{
    AsrCapabilities, AsrError, AsrOptions, AsrProvider, AsrResult, LiveAsrOptions, LiveAsrProvider,
    LiveCapabilities, LiveEventCallback, LiveModeKind, ProviderCapabilities, Transcript,
};
use dicta_core::AudioInput;
#[cfg(unix)]
use dicta_core::LiveStatusEvent;
#[cfg(unix)]
use dicta_core::{
    AudioChannel, EventTimestamp, LiveEvent, LiveMetaEvent, LiveStatusPhase, LiveVolatileEvent,
    TranscriptEvent, TranscriptSource,
};
#[cfg(unix)]
use libloading::{Library, Symbol};
#[cfg(any(unix, test))]
use serde::Deserialize;
#[cfg(any(unix, test))]
use serde_json::Value;
#[cfg(test)]
use serde_json::json;
use std::collections::HashSet;
#[cfg(unix)]
use std::ffi::CString;
#[cfg(unix)]
use std::os::raw::{c_char, c_int, c_void};
use std::path::{Path, PathBuf};
#[cfg(unix)]
use std::sync::mpsc::{self, Receiver};
#[cfg(unix)]
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
#[cfg(unix)]
use tokio::runtime::Handle;
#[cfg(unix)]
use uuid::Uuid;

pub const DEFAULT_MODEL: &str = "qianwenime-asr";
pub const DEFAULT_EMBEDDED_RUNTIME_NAME: &str = "libQianwenShellEmbedded.dylib";
pub const DEFAULT_UNET_RUNTIME_NAME: &str = "libqianwen_unet_runtime.dylib";
pub const DEFAULT_WSG_IMPL_NAME: &str = "libwsg_impl.dylib";
pub const DEFAULT_WSG_SHIM_NAME: &str = "libdicta_qianwen_wsg_shim.dylib";
pub const ENV_HOST_BUNDLE_PATH: &str = "DICTA_QIANWEN_HOST_BUNDLE_PATH";
pub const ENV_ASR_QUERY_SIGN: &str = "QWEN_SHELL_ASR_QUERY_SIGN";
pub const ENV_UTDID_SDK_DIR: &str = "QWEN_SHELL_UTDID_SDK_DIR";
pub const ENV_SETTINGS_PATH: &str = "QIANWEN_SHELL_SETTINGS_PATH";
pub const ENV_WSG_IMPL_PATH: &str = "QIANWEN_WSG_IMPL_PATH";
pub const ENV_UNET_RUNTIME_PATH: &str = "DICTA_QIANWEN_UNET_RUNTIME_PATH";
pub const ENV_UNET_PROCESS_NAME: &str = "DICTA_QIANWEN_UNET_PROCESS_NAME";

#[cfg(unix)]
const DEFAULT_MICROPHONE_SETTING_KEY: &str = "browser.quark.ai.voice_input.default_microphone";

#[cfg(all(unix, target_os = "macos"))]
const DEFAULT_WSG_SHIM_DYLIB: &[u8] = include_bytes!(env!("DICTA_QIANWEN_WSG_SHIM_DYLIB"));

#[cfg(unix)]
const POLL_INTERVAL: Duration = Duration::from_millis(100);
#[cfg(unix)]
const FINAL_DRAIN_GRACE: Duration = Duration::from_millis(500);
#[cfg(unix)]
const FINALIZE_TIMEOUT: Duration = Duration::from_secs(20);
#[cfg(unix)]
const INITIAL_BUFFER_CAPACITY: u32 = 64 * 1024;
#[cfg(unix)]
const MAX_BUFFER_CAPACITY: u32 = 4 * 1024 * 1024;

#[cfg(unix)]
// Qianwen's Swift enum is RawRepresentable<Int32> and uses 1-based raw values.
// Passing 0 is converted to the enum's invalid sentinel and is rejected.
const HOTKEY_PRESS: i32 = 1;
#[cfg(unix)]
const HOTKEY_RELEASE: i32 = 2;
#[cfg(unix)]
const HOTKEY_LONG_PRESS_TIMER: i32 = 3;
#[cfg(unix)]
const LONG_PRESS_TIMER_DELAY: Duration = Duration::from_millis(400);

#[derive(Debug, Clone, Default)]
pub struct QianwenConfig {
    pub runtime_path: Option<PathBuf>,
    pub host_bundle_path: Option<PathBuf>,
    pub wsg_impl_path: Option<PathBuf>,
    pub asr_query_sign: Option<String>,
}

#[derive(Debug, Clone)]
pub struct QianwenAsr {
    config: QianwenConfig,
}

impl QianwenAsr {
    pub fn new(config: QianwenConfig) -> AsrResult<Self> {
        Ok(Self { config })
    }

    pub fn resolved_runtime_path(&self) -> Option<PathBuf> {
        resolve_runtime_path(
            self.config.runtime_path.as_deref(),
            self.config.host_bundle_path.as_deref(),
        )
    }

    pub fn resolved_host_bundle_path(&self) -> Option<PathBuf> {
        resolve_host_bundle_path(
            self.config.host_bundle_path.as_deref(),
            self.config.runtime_path.as_deref(),
        )
    }

    pub fn resolved_wsg_impl_path(&self) -> Option<PathBuf> {
        let host_bundle = self.resolved_host_bundle_path();
        resolve_wsg_impl_path(self.config.wsg_impl_path.as_deref(), host_bundle.as_deref())
    }
}

#[async_trait]
impl AsrProvider for QianwenAsr {
    async fn transcribe(&self, _input: AudioInput, _options: AsrOptions) -> AsrResult<Transcript> {
        Err(AsrError::Input(
            "qianwen uses the local Qianwen Shell embedded runtime and only supports live microphone transcription; run with --asr qianwen --live".to_owned(),
        ))
    }

    fn name(&self) -> &'static str {
        "qianwen"
    }

    fn capabilities(&self) -> AsrCapabilities {
        qianwen_capabilities().batch
    }

    fn provider_capabilities(&self) -> ProviderCapabilities {
        qianwen_capabilities()
    }
}

#[async_trait]
impl LiveAsrProvider for QianwenAsr {
    async fn run_live(
        &self,
        options: LiveAsrOptions,
        on_event: LiveEventCallback<'_>,
    ) -> AsrResult<()> {
        validate_live_options(&options)?;
        run_qianwen_live(&self.config, self.live_name(), options, on_event)
    }

    fn live_name(&self) -> &'static str {
        "qianwen"
    }

    fn live_capabilities(&self) -> LiveCapabilities {
        qianwen_live_capabilities()
    }
}

#[cfg(unix)]
fn run_qianwen_live(
    config: &QianwenConfig,
    backend: &str,
    options: LiveAsrOptions,
    on_event: LiveEventCallback<'_>,
) -> AsrResult<()> {
    let src = options.src.clone().unwrap_or_else(|| "zh-CN".to_owned());
    on_event(LiveEvent::Meta(LiveMetaEvent {
        backend: backend.to_owned(),
        src: src.clone(),
        dst: None,
        mic: true,
        speaker: false,
        devices: Vec::new(),
    }))?;

    on_event(LiveEvent::Status(LiveStatusEvent {
        phase: LiveStatusPhase::Recovering,
        message: "starting Qianwen Shell runtime".to_owned(),
        detail: None,
    }))?;

    let mut session = QianwenEmbeddedSession::start(config)?;
    on_event(LiveEvent::Status(LiveStatusEvent {
        phase: LiveStatusPhase::Recording,
        message: "Qianwen Shell runtime started; press Ctrl-C to stop".to_owned(),
        detail: session.status_detail(),
    }))?;

    session.set_stream_text_in_editor(false)?;
    session.set_insert_available(true)?;
    session.hotkey(HOTKEY_PRESS)?;
    let long_press_timer_deadline = Instant::now() + LONG_PRESS_TIMER_DELAY;

    let ctrl_c = ctrl_c_channel();
    let mut seq = 0_u64;
    let mut final_text = String::new();
    let mut emitted_finals = HashSet::new();
    let mut long_press_timer_sent = false;
    let mut release_sent = false;
    let mut final_deadline: Option<Instant> = None;
    let mut done_after: Option<Instant> = None;

    loop {
        if !release_sent && !long_press_timer_sent && Instant::now() >= long_press_timer_deadline {
            session.hotkey(HOTKEY_LONG_PRESS_TIMER)?;
            long_press_timer_sent = true;
        }

        if !release_sent && ctrl_c.try_recv().is_ok() {
            session.hotkey(HOTKEY_RELEASE).ok();
            release_sent = true;
            final_deadline = Some(Instant::now() + FINALIZE_TIMEOUT);
            on_event(LiveEvent::Status(LiveStatusEvent {
                phase: LiveStatusPhase::Transcribing,
                message: "Qianwen Shell finalizing voice input".to_owned(),
                detail: session.status_detail(),
            }))?;
        }

        session.request_pump().ok();
        for event in session.next_events()? {
            match event {
                QianwenEvent::Volatile(text) => {
                    on_event(LiveEvent::Volatile(LiveVolatileEvent {
                        channel: AudioChannel::Mic,
                        text,
                    }))?;
                }
                QianwenEvent::Final(text) => {
                    let text = text.trim();
                    if text.is_empty() || !emitted_finals.insert(text.to_owned()) {
                        continue;
                    }
                    final_text.push_str(text);
                    on_event(LiveEvent::Finalized(TranscriptEvent {
                        seq,
                        channel: AudioChannel::Mic,
                        timestamp: EventTimestamp::now(),
                        audio: None,
                        src: TranscriptSource {
                            lang: src.clone(),
                            text: text.to_owned(),
                            confidence: None,
                        },
                        dst: None,
                    }))?;
                    seq += 1;
                    if release_sent {
                        done_after.get_or_insert_with(|| Instant::now() + FINAL_DRAIN_GRACE);
                    }
                }
                QianwenEvent::Done => {
                    done_after.get_or_insert_with(Instant::now);
                }
            }
        }

        if done_after.is_some_and(|deadline| Instant::now() >= deadline) {
            break;
        }
        if release_sent && final_deadline.is_some_and(|deadline| Instant::now() >= deadline) {
            break;
        }

        std::thread::sleep(POLL_INTERVAL);
    }

    session.set_insert_available(false).ok();
    if final_text.trim().is_empty() {
        on_event(LiveEvent::Status(LiveStatusEvent {
            phase: LiveStatusPhase::Recovering,
            message: "Qianwen live session ended without finalized text".to_owned(),
            detail: session.status_detail(),
        }))?;
    }
    on_event(LiveEvent::Eof)?;
    Ok(())
}

#[cfg(not(unix))]
fn run_qianwen_live(
    _config: &QianwenConfig,
    _backend: &str,
    _options: LiveAsrOptions,
    _on_event: LiveEventCallback<'_>,
) -> AsrResult<()> {
    Err(AsrError::Config(
        "qianwen live mode requires the local Qianwen Shell embedded runtime".to_owned(),
    ))
}

pub fn qianwen_capabilities() -> ProviderCapabilities {
    ProviderCapabilities {
        batch: AsrCapabilities {
            batch_file: false,
            streaming: false,
            requires_network: true,
        },
        live: Some(qianwen_live_capabilities()),
        notes: vec![
            "Qianwen uses the local Qianwen Shell embedded runtime from an installed or supplied qw bundle.".to_owned(),
            "Batch file transcription is not exposed by the Qianwen IME voice input runtime.".to_owned(),
        ],
    }
}

pub fn qianwen_live_capabilities() -> LiveCapabilities {
    LiveCapabilities {
        mode: LiveModeKind::Streaming,
        mic: true,
        speaker: false,
        streaming_audio: true,
        partial_results: true,
        finalized_results: true,
        translation: false,
        voice_processing: false,
        device_selection: false,
        requires_network: true,
        expected_latency: None,
    }
}

fn validate_live_options(options: &LiveAsrOptions) -> AsrResult<()> {
    if !options.mic {
        return Err(AsrError::Config(
            "qianwen live mode requires microphone input".to_owned(),
        ));
    }
    if options.speaker {
        return Err(AsrError::Config(
            "qianwen live mode does not support speaker capture".to_owned(),
        ));
    }
    if options.dst.is_some() {
        return Err(AsrError::Config(
            "qianwen live mode does not support translation".to_owned(),
        ));
    }
    if options.voice_processing || options.select_device {
        return Err(AsrError::Config(
            "qianwen live mode does not support Apple-only capture controls".to_owned(),
        ));
    }
    Ok(())
}

#[cfg(unix)]
struct QianwenEmbeddedSession {
    _library: Library,
    api: QianwenEmbeddedApi,
    runtime_path: PathBuf,
    history_path: PathBuf,
    debug_log_path: PathBuf,
    observability_dir: PathBuf,
    settings_path: Option<PathBuf>,
    microphone_device: Option<AudioDeviceSetting>,
    wsg_impl_path: Option<PathBuf>,
    seen_history_ids: HashSet<String>,
    started: bool,
}

#[cfg(unix)]
impl QianwenEmbeddedSession {
    fn start(config: &QianwenConfig) -> AsrResult<Self> {
        let runtime_path = resolve_runtime_path(
            config.runtime_path.as_deref(),
            config.host_bundle_path.as_deref(),
        )
        .ok_or_else(|| {
            AsrError::Config(
                "qianwen embedded runtime not found; set --qianwen-host-bundle-path or --qianwen-runtime-path".to_owned(),
            )
        })?;
        let host_bundle_path =
            resolve_host_bundle_path(config.host_bundle_path.as_deref(), Some(&runtime_path));

        let session_id = format!("dicta-{}", Uuid::new_v4());
        let history_path = temp_path(&format!("{session_id}-history.json"));
        let debug_log_path = temp_path(&format!("{session_id}-shell_voice_input.log"));
        let observability_dir = temp_path(&format!("{session_id}-observability"));
        let (settings_path, microphone_device) = prepare_settings_path(&session_id)?;
        let _ = std::fs::remove_file(&history_path);
        let _ = std::fs::remove_file(&debug_log_path);
        let _ = std::fs::remove_dir_all(&observability_dir);
        let _ = std::fs::create_dir_all(&observability_dir);
        let wsg_impl_path = prepare_wsg_impl_path(
            config.wsg_impl_path.as_deref(),
            host_bundle_path.as_deref(),
            &session_id,
        )?;

        set_process_env("QIANWEN_SHELL_VOICE_HISTORY_PATH", &history_path);
        set_process_env("QWEN_SHELL_VOICE_DEBUG_LOG", &debug_log_path);
        set_process_env("QWEN_SHELL_OBSERVABILITY_DIR", &observability_dir);
        set_process_env("QWEN_SHELL_RUN_ID", &session_id);
        if let Some(path) = settings_path.as_deref() {
            set_process_env(ENV_SETTINGS_PATH, path);
        }
        if let Some(sign) = non_empty(config.asr_query_sign.as_deref()) {
            set_process_env(ENV_ASR_QUERY_SIGN, sign);
        }
        if let Some(host) = host_bundle_path.as_deref() {
            set_process_env_if_absent(ENV_UTDID_SDK_DIR, host.join("Frameworks"));
            configure_unet_runtime_env(host);
        }
        if let Some(path) = wsg_impl_path.as_deref() {
            set_process_env(ENV_WSG_IMPL_PATH, path);
        }

        let library = unsafe { Library::new(&runtime_path) }.map_err(|err| {
            AsrError::Config(format!(
                "failed to load qianwen embedded runtime {}: {err}",
                runtime_path.display()
            ))
        })?;
        let api = unsafe { QianwenEmbeddedApi::load(&library) }?;

        let process_args = CString::new(qianwen_process_args_json())
            .map_err(|err| AsrError::Config(format!("invalid qianwen process args json: {err}")))?;
        let mode = CString::new("standalone").expect("static string has no nul");
        let args = ShellEmbeddedStartArgsFfi {
            size: std::mem::size_of::<ShellEmbeddedStartArgsFfi>() as u32,
            mode: mode.as_ptr(),
            process_args_json: process_args.as_ptr(),
            host_api: std::ptr::null(),
        };
        call_status("shell_embedded_start", unsafe { (api.start)(&args) })?;

        Ok(Self {
            _library: library,
            api,
            runtime_path,
            history_path,
            debug_log_path,
            observability_dir,
            settings_path,
            microphone_device,
            wsg_impl_path,
            seen_history_ids: HashSet::new(),
            started: true,
        })
    }

    fn status_detail(&self) -> Option<String> {
        let mut detail = format!(
            "runtime={}, log={}, observability={}",
            self.runtime_path.display(),
            self.debug_log_path.display(),
            self.observability_dir.display()
        );
        if let Some(path) = &self.wsg_impl_path {
            detail.push_str(&format!(", wsg={}", path.display()));
        }
        if let Some(path) = &self.settings_path {
            detail.push_str(&format!(", settings={}", path.display()));
        }
        if let Some(device) = &self.microphone_device {
            detail.push_str(&format!(", mic={} ({})", device.name, device.id));
        }
        Some(detail)
    }

    fn set_stream_text_in_editor(&self, enabled: bool) -> AsrResult<()> {
        call_status(
            "shell_embedded_voice_input_set_stream_text_in_editor_enabled",
            unsafe { (self.api.set_stream_text_in_editor_enabled)(enabled as c_int) },
        )
    }

    fn set_insert_available(&self, available: bool) -> AsrResult<()> {
        call_status("shell_embedded_voice_input_set_insert_available", unsafe {
            (self.api.set_insert_available)(available as c_int)
        })
    }

    fn hotkey(&self, kind: i32) -> AsrResult<()> {
        let mut esc_guard_required = 0_i32;
        let event = ShellVoiceInputHotkeyEventFfi {
            size: std::mem::size_of::<ShellVoiceInputHotkeyEventFfi>() as u32,
            kind,
            out_esc_guard_required: &mut esc_guard_required,
        };
        call_status("shell_embedded_voice_input_on_hotkey_event", unsafe {
            (self.api.on_hotkey_event)(&event)
        })
    }

    fn request_pump(&self) -> AsrResult<()> {
        call_status("shell_embedded_voice_input_request_pump", unsafe {
            (self.api.request_pump)()
        })
    }

    fn next_events(&mut self) -> AsrResult<Vec<QianwenEvent>> {
        let mut events = Vec::new();
        events.extend(parse_voice_results_json(
            &self.read_string_buffer("voice results", self.api.drain_results_json)?,
        ));
        events.extend(parse_voice_text_events_json(&self.read_string_buffer(
            "voice text events",
            self.api.drain_text_events_json,
        )?));
        if let Some(event) = self.next_history_event()? {
            events.push(event);
        }
        Ok(events)
    }

    fn read_string_buffer(&self, name: &str, call: ShellEmbeddedDrainJson) -> AsrResult<String> {
        let mut capacity = INITIAL_BUFFER_CAPACITY;
        loop {
            let mut buffer = vec![0_u8; capacity as usize];
            let mut ffi = ShellStringBufferFfi {
                size: std::mem::size_of::<ShellStringBufferFfi>() as u32,
                data: buffer.as_mut_ptr().cast::<c_char>(),
                capacity,
                len: 0,
                required_len: 0,
            };
            let status = unsafe { call(&mut ffi) };
            if ffi.required_len > capacity {
                capacity = grow_buffer_capacity(ffi.required_len)?;
                continue;
            }
            if status != 0 {
                return Err(AsrError::Request(format!(
                    "qianwen {name} drain failed with status {status}"
                )));
            }
            let len = ffi.len.min(capacity) as usize;
            let bytes = &buffer[..len];
            let nul = bytes
                .iter()
                .position(|byte| *byte == 0)
                .unwrap_or(bytes.len());
            return String::from_utf8(bytes[..nul].to_vec()).map_err(|err| {
                AsrError::InvalidResponse(format!("qianwen {name} json is not utf-8: {err}"))
            });
        }
    }

    fn next_history_event(&mut self) -> AsrResult<Option<QianwenEvent>> {
        let Ok(content) = std::fs::read_to_string(&self.history_path) else {
            return Ok(None);
        };
        let Ok(mut entries) = serde_json::from_str::<Vec<VoiceHistoryEntry>>(&content) else {
            return Ok(None);
        };
        entries.sort_by_key(|entry| entry.created_at_ms.unwrap_or_default());

        for entry in entries {
            let Some(id) = non_empty(entry.id.as_deref()).map(ToOwned::to_owned) else {
                continue;
            };
            if !self.seen_history_ids.insert(id) {
                continue;
            }
            if let Some(text) = entry.final_text() {
                return Ok(Some(QianwenEvent::Final(text)));
            }
        }
        Ok(None)
    }
}

#[cfg(unix)]
impl Drop for QianwenEmbeddedSession {
    fn drop(&mut self) {
        if self.started {
            let _ = unsafe { (self.api.shutdown)() };
            self.started = false;
        }
    }
}

#[cfg(unix)]
#[derive(Clone, Copy)]
struct QianwenEmbeddedApi {
    start: ShellEmbeddedStart,
    shutdown: ShellEmbeddedNoArg,
    request_pump: ShellEmbeddedNoArg,
    on_hotkey_event: ShellEmbeddedHotkey,
    drain_results_json: ShellEmbeddedDrainJson,
    drain_text_events_json: ShellEmbeddedDrainJson,
    set_insert_available: ShellEmbeddedBool,
    set_stream_text_in_editor_enabled: ShellEmbeddedBool,
}

#[cfg(unix)]
impl QianwenEmbeddedApi {
    unsafe fn load(library: &Library) -> AsrResult<Self> {
        Ok(Self {
            start: unsafe { load_symbol(library, b"shell_embedded_start\0")? },
            shutdown: unsafe { load_symbol(library, b"shell_embedded_shutdown\0")? },
            request_pump: unsafe {
                load_symbol(library, b"shell_embedded_voice_input_request_pump\0")?
            },
            on_hotkey_event: unsafe {
                load_symbol(library, b"shell_embedded_voice_input_on_hotkey_event\0")?
            },
            drain_results_json: unsafe {
                load_symbol(library, b"shell_embedded_voice_input_drain_results_json\0")?
            },
            drain_text_events_json: unsafe {
                load_symbol(
                    library,
                    b"shell_embedded_voice_input_drain_text_events_json\0",
                )?
            },
            set_insert_available: unsafe {
                load_symbol(
                    library,
                    b"shell_embedded_voice_input_set_insert_available\0",
                )?
            },
            set_stream_text_in_editor_enabled: unsafe {
                load_symbol(
                    library,
                    b"shell_embedded_voice_input_set_stream_text_in_editor_enabled\0",
                )?
            },
        })
    }
}

#[cfg(unix)]
unsafe fn load_symbol<T: Copy>(library: &Library, symbol: &'static [u8]) -> AsrResult<T> {
    let loaded: Symbol<'_, T> = unsafe { library.get(symbol) }.map_err(|err| {
        AsrError::Config(format!(
            "qianwen embedded runtime missing symbol {}: {err}",
            String::from_utf8_lossy(symbol).trim_end_matches('\0')
        ))
    })?;
    Ok(*loaded)
}

#[cfg(unix)]
type ShellEmbeddedStart = unsafe extern "C" fn(*const ShellEmbeddedStartArgsFfi) -> c_int;
#[cfg(unix)]
type ShellEmbeddedNoArg = unsafe extern "C" fn() -> c_int;
#[cfg(unix)]
type ShellEmbeddedHotkey = unsafe extern "C" fn(*const ShellVoiceInputHotkeyEventFfi) -> c_int;
#[cfg(unix)]
type ShellEmbeddedDrainJson = unsafe extern "C" fn(*mut ShellStringBufferFfi) -> c_int;
#[cfg(unix)]
type ShellEmbeddedBool = unsafe extern "C" fn(c_int) -> c_int;

#[cfg(unix)]
#[repr(C)]
struct ShellEmbeddedStartArgsFfi {
    size: u32,
    mode: *const c_char,
    process_args_json: *const c_char,
    host_api: *const c_void,
}

#[cfg(unix)]
#[repr(C)]
struct ShellVoiceInputHotkeyEventFfi {
    size: u32,
    kind: i32,
    out_esc_guard_required: *mut i32,
}

#[cfg(unix)]
#[repr(C)]
struct ShellStringBufferFfi {
    size: u32,
    data: *mut c_char,
    capacity: u32,
    len: u32,
    required_len: u32,
}

#[cfg(unix)]
fn call_status(name: &str, status: c_int) -> AsrResult<()> {
    if status == 0 {
        Ok(())
    } else {
        Err(AsrError::Request(format!(
            "qianwen embedded call {name} failed with status {status}"
        )))
    }
}

#[cfg(unix)]
fn grow_buffer_capacity(required_len: u32) -> AsrResult<u32> {
    let capacity = required_len.saturating_add(1).next_power_of_two();
    if capacity > MAX_BUFFER_CAPACITY {
        return Err(AsrError::InvalidResponse(format!(
            "qianwen JSON payload too large: required {required_len} bytes"
        )));
    }
    Ok(capacity)
}

#[cfg(unix)]
fn qianwen_process_args_json() -> &'static str {
    r#"{"source":"qianwen_desktop_shell_standalone_ime_service","runtimeMode":"standalone","headless":true,"argv":[],"switches":{"qianwen-ime":"1","qianwen-ime-shell-process":"1","qianwen-desktop-shell-standalone":"1"}}"#
}

#[cfg(unix)]
fn set_process_env(key: &str, value: impl AsRef<std::ffi::OsStr>) {
    // The Qianwen runtime reads these integration knobs from process
    // environment variables during initialization.
    unsafe {
        std::env::set_var(key, value);
    }
}

#[cfg(unix)]
fn set_process_env_if_absent(key: &str, value: impl AsRef<std::ffi::OsStr>) {
    if std::env::var_os(key).is_none() {
        set_process_env(key, value);
    }
}

#[cfg(unix)]
fn configure_unet_runtime_env(host: &Path) {
    let runtime_path = qianwen_unet_runtime_path_from_host(host);
    if runtime_path.exists() {
        set_process_env(ENV_UNET_RUNTIME_PATH, absolute_path(&runtime_path));
        set_process_env_if_absent(ENV_UNET_PROCESS_NAME, "qianwen-ime");
    }
}

#[cfg(unix)]
#[derive(Debug, Clone, PartialEq, Eq)]
struct AudioDeviceSetting {
    id: String,
    name: String,
}

#[cfg(unix)]
fn prepare_settings_path(
    session_id: &str,
) -> AsrResult<(Option<PathBuf>, Option<AudioDeviceSetting>)> {
    if std::env::var_os(ENV_SETTINGS_PATH).is_some() {
        return Ok((None, None));
    }

    let Some(device) = default_input_audio_device() else {
        return Ok((None, None));
    };

    let settings = qianwen_settings_with_default_microphone(load_user_qianwen_settings(), &device);
    let path = temp_path(&format!("{session_id}-settings.json"));
    let bytes = serde_json::to_vec_pretty(&settings).map_err(|err| {
        AsrError::Config(format!("failed to serialize qianwen settings json: {err}"))
    })?;
    std::fs::write(&path, bytes).map_err(|err| {
        AsrError::Config(format!(
            "failed to write qianwen settings {}: {err}",
            path.display()
        ))
    })?;

    Ok((Some(path), Some(device)))
}

#[cfg(unix)]
fn load_user_qianwen_settings() -> serde_json::Map<String, Value> {
    default_user_qianwen_settings_path()
        .and_then(|path| std::fs::read_to_string(path).ok())
        .and_then(|content| serde_json::from_str::<Value>(&content).ok())
        .and_then(|value| match value {
            Value::Object(map) => Some(map),
            _ => None,
        })
        .unwrap_or_default()
}

#[cfg(unix)]
fn qianwen_settings_with_default_microphone(
    mut settings: serde_json::Map<String, Value>,
    device: &AudioDeviceSetting,
) -> Value {
    settings.insert(
        DEFAULT_MICROPHONE_SETTING_KEY.to_owned(),
        serde_json::json!({
            "device_id": device.id,
            "device_name": device.name,
            "is_default": false,
            "deviceId": device.id,
            "deviceName": device.name,
            "isDefault": false,
        }),
    );
    Value::Object(settings)
}

#[cfg(unix)]
fn default_user_qianwen_settings_path() -> Option<PathBuf> {
    std::env::var_os("HOME").map(|home| {
        PathBuf::from(home)
            .join("Library")
            .join("Application Support")
            .join("QianwenShell")
            .join("settings.json")
    })
}

#[cfg(all(unix, target_os = "macos"))]
fn default_input_audio_device() -> Option<AudioDeviceSetting> {
    macos_audio::default_input_device()
}

#[cfg(all(unix, not(target_os = "macos")))]
fn default_input_audio_device() -> Option<AudioDeviceSetting> {
    None
}

#[cfg(all(unix, target_os = "macos"))]
mod macos_audio {
    use super::AudioDeviceSetting;
    use std::ffi::c_void;
    use std::os::raw::{c_char, c_int, c_uchar};

    type AudioObjectId = u32;
    type AudioObjectPropertySelector = u32;
    type AudioObjectPropertyScope = u32;
    type AudioObjectPropertyElement = u32;

    #[repr(C)]
    struct AudioObjectPropertyAddress {
        selector: AudioObjectPropertySelector,
        scope: AudioObjectPropertyScope,
        element: AudioObjectPropertyElement,
    }

    type CfStringRef = *const c_void;

    #[link(name = "CoreAudio", kind = "framework")]
    unsafe extern "C" {
        fn AudioObjectGetPropertyDataSize(
            object_id: AudioObjectId,
            address: *const AudioObjectPropertyAddress,
            qualifier_data_size: u32,
            qualifier_data: *const c_void,
            data_size: *mut u32,
        ) -> c_int;
        fn AudioObjectGetPropertyData(
            object_id: AudioObjectId,
            address: *const AudioObjectPropertyAddress,
            qualifier_data_size: u32,
            qualifier_data: *const c_void,
            data_size: *mut u32,
            data: *mut c_void,
        ) -> c_int;
    }

    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        fn CFStringGetCString(
            string: CfStringRef,
            buffer: *mut c_char,
            buffer_size: isize,
            encoding: u32,
        ) -> c_uchar;
        fn CFRelease(object: *const c_void);
    }

    const SYSTEM_OBJECT: AudioObjectId = 1;
    const MAIN_ELEMENT: AudioObjectPropertyElement = 0;
    const GLOBAL_SCOPE: AudioObjectPropertyScope = fourcc(*b"glob");
    const INPUT_SCOPE: AudioObjectPropertyScope = fourcc(*b"inpt");
    const DEVICES: AudioObjectPropertySelector = fourcc(*b"dev#");
    const DEFAULT_INPUT_DEVICE: AudioObjectPropertySelector = fourcc(*b"dIn ");
    const DEVICE_STREAMS: AudioObjectPropertySelector = fourcc(*b"stm#");
    const DEVICE_UID: AudioObjectPropertySelector = fourcc(*b"uid ");
    const OBJECT_NAME: AudioObjectPropertySelector = fourcc(*b"lnam");
    const CF_STRING_ENCODING_UTF8: u32 = 0x0800_0100;

    const fn fourcc(bytes: [u8; 4]) -> u32 {
        ((bytes[0] as u32) << 24)
            | ((bytes[1] as u32) << 16)
            | ((bytes[2] as u32) << 8)
            | bytes[3] as u32
    }

    pub fn default_input_device() -> Option<AudioDeviceSetting> {
        let default_device_id = read_u32_property(
            SYSTEM_OBJECT,
            DEFAULT_INPUT_DEVICE,
            GLOBAL_SCOPE,
            MAIN_ELEMENT,
        )
        .unwrap_or_default();

        device_setting(default_device_id).or_else(|| {
            list_audio_devices()
                .into_iter()
                .find(|device_id| has_input_streams(*device_id))
                .and_then(device_setting)
        })
    }

    fn device_setting(device_id: AudioObjectId) -> Option<AudioDeviceSetting> {
        if device_id == 0 {
            return None;
        }
        let id = read_string_property(device_id, DEVICE_UID, GLOBAL_SCOPE, MAIN_ELEMENT)
            .or_else(|| read_string_property(device_id, DEVICE_UID, INPUT_SCOPE, MAIN_ELEMENT))?;
        let name = read_string_property(device_id, OBJECT_NAME, GLOBAL_SCOPE, MAIN_ELEMENT)
            .filter(|name| !name.trim().is_empty())
            .unwrap_or_else(|| id.clone());

        Some(AudioDeviceSetting { id, name })
    }

    fn list_audio_devices() -> Vec<AudioObjectId> {
        let Some(mut data_size) =
            property_data_size(SYSTEM_OBJECT, DEVICES, GLOBAL_SCOPE, MAIN_ELEMENT)
        else {
            return Vec::new();
        };
        if data_size == 0 {
            return Vec::new();
        }

        let count = data_size as usize / std::mem::size_of::<AudioObjectId>();
        let mut devices = vec![0_u32; count];
        let address = AudioObjectPropertyAddress {
            selector: DEVICES,
            scope: GLOBAL_SCOPE,
            element: MAIN_ELEMENT,
        };
        let status = unsafe {
            AudioObjectGetPropertyData(
                SYSTEM_OBJECT,
                &address,
                0,
                std::ptr::null(),
                &mut data_size,
                devices.as_mut_ptr().cast::<c_void>(),
            )
        };
        if status == 0 { devices } else { Vec::new() }
    }

    fn has_input_streams(device_id: AudioObjectId) -> bool {
        property_data_size(device_id, DEVICE_STREAMS, INPUT_SCOPE, MAIN_ELEMENT)
            .is_some_and(|size| size > 0)
    }

    fn property_data_size(
        object_id: AudioObjectId,
        selector: AudioObjectPropertySelector,
        scope: AudioObjectPropertyScope,
        element: AudioObjectPropertyElement,
    ) -> Option<u32> {
        let address = AudioObjectPropertyAddress {
            selector,
            scope,
            element,
        };
        let mut data_size = 0_u32;
        let status = unsafe {
            AudioObjectGetPropertyDataSize(object_id, &address, 0, std::ptr::null(), &mut data_size)
        };
        (status == 0).then_some(data_size)
    }

    fn read_u32_property(
        object_id: AudioObjectId,
        selector: AudioObjectPropertySelector,
        scope: AudioObjectPropertyScope,
        element: AudioObjectPropertyElement,
    ) -> Option<u32> {
        let address = AudioObjectPropertyAddress {
            selector,
            scope,
            element,
        };
        let mut value = 0_u32;
        let mut data_size = std::mem::size_of::<u32>() as u32;
        let status = unsafe {
            AudioObjectGetPropertyData(
                object_id,
                &address,
                0,
                std::ptr::null(),
                &mut data_size,
                (&mut value as *mut u32).cast::<c_void>(),
            )
        };
        (status == 0).then_some(value)
    }

    fn read_string_property(
        object_id: AudioObjectId,
        selector: AudioObjectPropertySelector,
        scope: AudioObjectPropertyScope,
        element: AudioObjectPropertyElement,
    ) -> Option<String> {
        let address = AudioObjectPropertyAddress {
            selector,
            scope,
            element,
        };
        let mut value: CfStringRef = std::ptr::null();
        let mut data_size = std::mem::size_of::<CfStringRef>() as u32;
        let status = unsafe {
            AudioObjectGetPropertyData(
                object_id,
                &address,
                0,
                std::ptr::null(),
                &mut data_size,
                (&mut value as *mut CfStringRef).cast::<c_void>(),
            )
        };
        if status != 0 || value.is_null() {
            return None;
        }

        let result = cf_string_to_string(value);
        unsafe {
            CFRelease(value);
        }
        result
    }

    fn cf_string_to_string(value: CfStringRef) -> Option<String> {
        let mut buffer = vec![0_u8; 1024];
        let ok = unsafe {
            CFStringGetCString(
                value,
                buffer.as_mut_ptr().cast::<c_char>(),
                buffer.len() as isize,
                CF_STRING_ENCODING_UTF8,
            )
        };
        if ok == 0 {
            return None;
        }
        let nul = buffer.iter().position(|byte| *byte == 0)?;
        String::from_utf8(buffer[..nul].to_vec()).ok()
    }
}

#[cfg(unix)]
fn prepare_wsg_impl_path(
    explicit: Option<&Path>,
    host_bundle: Option<&Path>,
    session_id: &str,
) -> AsrResult<Option<PathBuf>> {
    if let Some(path) = resolve_wsg_impl_path(explicit, host_bundle) {
        return Ok(Some(absolute_path(&path)));
    }

    materialize_default_wsg_shim(host_bundle, session_id)
}

#[cfg(all(unix, target_os = "macos"))]
fn materialize_default_wsg_shim(
    host_bundle: Option<&Path>,
    session_id: &str,
) -> AsrResult<Option<PathBuf>> {
    let Some(host) = host_bundle else {
        return Ok(None);
    };
    if !qianwen_unet_runtime_path_from_host(host).exists() {
        return Ok(None);
    }

    let path = temp_path(&format!("{session_id}-{DEFAULT_WSG_SHIM_NAME}"));
    std::fs::write(&path, DEFAULT_WSG_SHIM_DYLIB).map_err(|err| {
        AsrError::Config(format!(
            "failed to write qianwen WSG shim {}: {err}",
            path.display()
        ))
    })?;
    Ok(Some(path))
}

#[cfg(all(unix, not(target_os = "macos")))]
fn materialize_default_wsg_shim(
    _host_bundle: Option<&Path>,
    _session_id: &str,
) -> AsrResult<Option<PathBuf>> {
    Ok(None)
}

#[cfg(unix)]
fn ctrl_c_channel() -> Receiver<()> {
    let (tx, rx) = mpsc::channel();
    if let Ok(handle) = Handle::try_current() {
        std::thread::spawn(move || {
            let _ = handle.block_on(tokio::signal::ctrl_c());
            let _ = tx.send(());
        });
    }
    rx
}

#[cfg(any(unix, test))]
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct VoiceHistoryEntry {
    id: Option<String>,
    created_at_ms: Option<u64>,
    input_text: Option<String>,
    text: Option<String>,
    action_kind: Option<String>,
}

#[cfg(any(unix, test))]
impl VoiceHistoryEntry {
    fn final_text(&self) -> Option<String> {
        if self
            .action_kind
            .as_deref()
            .is_some_and(|kind| kind != "InsertText")
        {
            return None;
        }
        non_empty(self.text.as_deref())
            .or_else(|| non_empty(self.input_text.as_deref()))
            .map(ToOwned::to_owned)
    }
}

#[cfg(any(unix, test))]
enum QianwenEvent {
    Volatile(String),
    Final(String),
    Done,
}

#[cfg(any(unix, test))]
fn parse_voice_results_json(raw: &str) -> Vec<QianwenEvent> {
    let Some(value) = parse_json(raw) else {
        return Vec::new();
    };
    let mut events = Vec::new();
    collect_voice_result_events(&value, &mut events);
    events
}

#[cfg(any(unix, test))]
fn parse_voice_text_events_json(raw: &str) -> Vec<QianwenEvent> {
    let Some(value) = parse_json(raw) else {
        return Vec::new();
    };
    let mut events = Vec::new();
    collect_voice_text_events(&value, &mut events);
    events
}

#[cfg(any(unix, test))]
fn parse_json(raw: &str) -> Option<Value> {
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    serde_json::from_str(raw).ok()
}

#[cfg(any(unix, test))]
fn collect_voice_result_events(value: &Value, events: &mut Vec<QianwenEvent>) {
    if let Some(items) = value.get("results").and_then(Value::as_array) {
        for item in items {
            collect_voice_result_events(item, events);
        }
        return;
    }
    if let Some(items) = value.get("items").and_then(Value::as_array) {
        for item in items {
            collect_voice_result_events(item, events);
        }
        return;
    }
    if let Some(items) = value.as_array() {
        for item in items {
            collect_voice_result_events(item, events);
        }
        return;
    }
    if let Some(event) = parse_voice_result(value) {
        events.push(event);
    }
}

#[cfg(any(unix, test))]
fn collect_voice_text_events(value: &Value, events: &mut Vec<QianwenEvent>) {
    if let Some(items) = value.get("events").and_then(Value::as_array) {
        for item in items {
            collect_voice_text_events(item, events);
        }
        return;
    }
    if let Some(items) = value.get("items").and_then(Value::as_array) {
        for item in items {
            collect_voice_text_events(item, events);
        }
        return;
    }
    if let Some(items) = value.as_array() {
        for item in items {
            collect_voice_text_events(item, events);
        }
        return;
    }
    if let Some(event) = parse_voice_text_event(value) {
        events.push(event);
    }
}

#[cfg(any(unix, test))]
fn parse_voice_result(payload: &Value) -> Option<QianwenEvent> {
    let text = first_text(
        payload,
        &["text", "finalText", "displayText", "partialText"],
    )?;
    let state = payload
        .get("state")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let completed = payload
        .get("completed")
        .and_then(Value::as_bool)
        .or_else(|| payload.get("isFinal").and_then(Value::as_bool))
        .or_else(|| payload.get("final").and_then(Value::as_bool))
        .unwrap_or_else(|| {
            matches!(
                state,
                "completed" | "complete" | "final" | "done" | "committed"
            ) || payload.get("actionKind").and_then(Value::as_str) == Some("InsertText")
        });

    Some(if completed {
        QianwenEvent::Final(text)
    } else {
        QianwenEvent::Volatile(text)
    })
}

#[cfg(any(unix, test))]
fn parse_voice_text_event(payload: &Value) -> Option<QianwenEvent> {
    let kind = payload
        .get("kind")
        .or_else(|| payload.get("type"))
        .or_else(|| payload.get("event"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    let text = first_text(
        payload,
        &[
            "text",
            "commitText",
            "finalText",
            "partialText",
            "displayText",
        ],
    );

    match kind {
        "done" | "finished" | "eof" => Some(QianwenEvent::Done),
        "clear_stream_text" | "clearStreamText" => Some(QianwenEvent::Volatile(String::new())),
        "commit_text" | "commitText" | "completed" | "final" | "finalText" => {
            text.map(QianwenEvent::Final)
        }
        "insert_text" | "insertText" | "partial" | "partialText" | "stream_text" | "streamText" => {
            text.map(QianwenEvent::Volatile)
        }
        _ if kind.contains("commit") || kind.contains("final") => text.map(QianwenEvent::Final),
        _ => text.map(QianwenEvent::Volatile),
    }
}

#[cfg(any(unix, test))]
fn first_text(payload: &Value, keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|key| payload.get(*key).and_then(Value::as_str))
        .and_then(|value| non_empty(Some(value)))
        .map(ToOwned::to_owned)
}

fn resolve_runtime_path(
    explicit_runtime: Option<&Path>,
    host_bundle: Option<&Path>,
) -> Option<PathBuf> {
    if let Some(path) = explicit_runtime
        .map(Path::to_path_buf)
        .filter(|path| path.exists())
    {
        return Some(path);
    }

    if let Some(path) = host_bundle
        .map(qianwen_embedded_runtime_path_from_host)
        .filter(|path| path.exists())
    {
        return Some(path);
    }

    default_qianwen_host_bundle_path()
        .map(|host| qianwen_embedded_runtime_path_from_host(&host))
        .filter(|path| path.exists())
}

fn resolve_host_bundle_path(
    explicit: Option<&Path>,
    runtime_path: Option<&Path>,
) -> Option<PathBuf> {
    explicit
        .map(Path::to_path_buf)
        .filter(|path| path.exists())
        .or_else(|| runtime_path.and_then(host_bundle_from_runtime_path))
        .or_else(default_qianwen_host_bundle_path)
}

fn resolve_wsg_impl_path(explicit: Option<&Path>, host_bundle: Option<&Path>) -> Option<PathBuf> {
    if let Some(path) = explicit.map(Path::to_path_buf).filter(|path| path.exists()) {
        return Some(path);
    }

    if let Some(path) = std::env::var_os(ENV_WSG_IMPL_PATH)
        .map(PathBuf::from)
        .filter(|path| path.exists())
    {
        return Some(path);
    }

    host_bundle
        .into_iter()
        .flat_map(qianwen_wsg_impl_path_candidates_from_host)
        .find(|path| path.exists())
}

fn qianwen_embedded_runtime_path_from_host(host: &Path) -> PathBuf {
    host.join("Frameworks")
        .join("qianwen_shell")
        .join(DEFAULT_EMBEDDED_RUNTIME_NAME)
}

#[cfg(any(unix, test))]
fn qianwen_unet_runtime_path_from_host(host: &Path) -> PathBuf {
    host.join("Frameworks").join(DEFAULT_UNET_RUNTIME_NAME)
}

fn qianwen_wsg_impl_path_candidates_from_host(host: &Path) -> Vec<PathBuf> {
    let frameworks = host.join("Frameworks");
    let mut candidates = vec![
        frameworks.join(DEFAULT_WSG_IMPL_NAME),
        frameworks.join("qianwen_shell").join(DEFAULT_WSG_IMPL_NAME),
        frameworks
            .join("wireless_security_guard_prebuilt")
            .join("lib")
            .join(DEFAULT_WSG_IMPL_NAME),
        frameworks
            .join("third_party")
            .join("wireless_security_guard_prebuilt")
            .join("lib")
            .join(DEFAULT_WSG_IMPL_NAME),
    ];
    for arch_dir in ["macarm64", "macx64"] {
        candidates.push(frameworks.join(arch_dir).join(DEFAULT_WSG_IMPL_NAME));
        candidates.push(
            frameworks
                .join("qianwen_shell")
                .join(arch_dir)
                .join(DEFAULT_WSG_IMPL_NAME),
        );
        candidates.push(
            frameworks
                .join("wireless_security_guard_prebuilt")
                .join("lib")
                .join(arch_dir)
                .join(DEFAULT_WSG_IMPL_NAME),
        );
        candidates.push(
            frameworks
                .join("third_party")
                .join("wireless_security_guard_prebuilt")
                .join("lib")
                .join(arch_dir)
                .join(DEFAULT_WSG_IMPL_NAME),
        );
    }
    candidates
}

fn host_bundle_from_runtime_path(runtime_path: &Path) -> Option<PathBuf> {
    let mut path = runtime_path;
    for _ in 0..3 {
        path = path.parent()?;
    }
    Some(path.to_path_buf())
}

pub fn default_qianwen_host_bundle_path() -> Option<PathBuf> {
    qianwen_host_bundle_candidates()
        .into_iter()
        .find(|path| qianwen_embedded_runtime_path_from_host(path).exists())
}

fn qianwen_host_bundle_candidates() -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(path) = std::env::var_os(ENV_HOST_BUNDLE_PATH) {
        candidates.push(PathBuf::from(path));
    }
    if let Ok(cwd) = std::env::current_dir() {
        push_relative_qianwen_candidates(&mut candidates, &cwd);
    }
    if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent()
    {
        push_relative_qianwen_candidates(&mut candidates, dir);
    }
    if let Some(home) = std::env::var_os("HOME") {
        let applications = PathBuf::from(home).join("Applications");
        push_app_bundle_candidates(&mut candidates, &applications);
    }
    push_app_bundle_candidates(&mut candidates, Path::new("/Applications"));

    let mut seen = HashSet::new();
    candidates
        .into_iter()
        .filter_map(|path| {
            let path = absolute_path(&path);
            seen.insert(path.clone()).then_some(path)
        })
        .collect()
}

fn push_relative_qianwen_candidates(candidates: &mut Vec<PathBuf>, base: &Path) {
    for ancestor in base.ancestors() {
        candidates.push(ancestor.join("qw"));
        push_app_bundle_candidates(candidates, ancestor);
    }
}

fn push_app_bundle_candidates(candidates: &mut Vec<PathBuf>, base: &Path) {
    for name in ["QianwenIME.app", "千问输入法.app"] {
        candidates.push(base.join(name).join("Contents"));
    }
}

fn absolute_path(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

#[cfg(unix)]
fn temp_path(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("dicta-qianwen-{}-{name}", now_millis()))
}

#[cfg(unix)]
fn now_millis() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or(0)
}

#[cfg(any(unix, test))]
fn non_empty(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|value| !value.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn qianwen_is_live_only() {
        let capabilities = qianwen_capabilities();
        assert!(!capabilities.batch.batch_file);
        assert!(capabilities.live.is_some());
        assert!(capabilities.live.unwrap().mic);
    }

    #[test]
    fn host_bundle_resolves_from_runtime_path() {
        let runtime =
            PathBuf::from("/tmp/qw/Frameworks/qianwen_shell/libQianwenShellEmbedded.dylib");
        assert_eq!(
            host_bundle_from_runtime_path(&runtime).unwrap(),
            PathBuf::from("/tmp/qw")
        );
    }

    #[test]
    fn host_bundle_candidates_include_executable_ancestor_qw() {
        let mut candidates = Vec::new();
        push_relative_qianwen_candidates(&mut candidates, Path::new("/tmp/dicta/target/debug"));

        assert!(candidates.contains(&PathBuf::from("/tmp/dicta/target/debug/qw")));
        assert!(candidates.contains(&PathBuf::from("/tmp/dicta/target/qw")));
        assert!(candidates.contains(&PathBuf::from("/tmp/dicta/qw")));
        assert!(candidates.contains(&PathBuf::from("/tmp/dicta/QianwenIME.app/Contents")));
    }

    #[test]
    fn runtime_path_resolves_from_explicit_host_bundle() {
        let root = std::env::temp_dir().join(format!(
            "dicta-qianwen-test-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let runtime = qianwen_embedded_runtime_path_from_host(&root);
        std::fs::create_dir_all(runtime.parent().unwrap()).unwrap();
        std::fs::write(&runtime, "").unwrap();

        assert_eq!(resolve_runtime_path(None, Some(&root)).unwrap(), runtime);

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn wsg_impl_resolves_from_explicit_path() {
        let root = std::env::temp_dir().join(format!(
            "dicta-qianwen-wsg-test-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let wsg_impl = root.join(DEFAULT_WSG_IMPL_NAME);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(&wsg_impl, "").unwrap();

        assert_eq!(
            resolve_wsg_impl_path(Some(&wsg_impl), None).unwrap(),
            wsg_impl
        );

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn wsg_impl_candidates_include_frameworks_path() {
        let root = PathBuf::from("/tmp/qw");
        assert!(
            qianwen_wsg_impl_path_candidates_from_host(&root)
                .contains(&PathBuf::from("/tmp/qw/Frameworks/libwsg_impl.dylib"))
        );
    }

    #[test]
    fn unet_runtime_resolves_from_host_bundle() {
        let root = PathBuf::from("/tmp/qw");
        assert_eq!(
            qianwen_unet_runtime_path_from_host(&root),
            PathBuf::from("/tmp/qw/Frameworks/libqianwen_unet_runtime.dylib")
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn default_wsg_shim_materializes_when_unet_runtime_exists() {
        let root = std::env::temp_dir().join(format!(
            "dicta-qianwen-wsg-shim-test-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let unet_runtime = qianwen_unet_runtime_path_from_host(&root);
        std::fs::create_dir_all(unet_runtime.parent().unwrap()).unwrap();
        std::fs::write(&unet_runtime, "").unwrap();

        let shim = prepare_wsg_impl_path(None, Some(&root), "test-session")
            .unwrap()
            .expect("default shim path");
        assert!(shim.exists());
        assert!(
            shim.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.ends_with("test-session-libdicta_qianwen_wsg_shim.dylib"))
        );

        let _ = std::fs::remove_file(shim);
        let _ = std::fs::remove_dir_all(root);
    }

    #[cfg(unix)]
    #[test]
    fn qianwen_settings_injects_default_microphone() {
        let mut settings = serde_json::Map::new();
        settings.insert(
            "browser.quark.ai.voiceinput.command.migration_done".to_owned(),
            Value::Bool(true),
        );

        let value = qianwen_settings_with_default_microphone(
            settings,
            &AudioDeviceSetting {
                id: "ByteviewAudioDevice_UID".to_owned(),
                name: "LarkAudioDevice".to_owned(),
            },
        );

        assert_eq!(
            value
                .get("browser.quark.ai.voiceinput.command.migration_done")
                .and_then(Value::as_bool),
            Some(true)
        );
        let mic = value
            .get(DEFAULT_MICROPHONE_SETTING_KEY)
            .and_then(Value::as_object)
            .expect("default microphone setting");
        assert_eq!(
            mic.get("device_id").and_then(Value::as_str),
            Some("ByteviewAudioDevice_UID")
        );
        assert_eq!(
            mic.get("device_name").and_then(Value::as_str),
            Some("LarkAudioDevice")
        );
        assert_eq!(mic.get("is_default").and_then(Value::as_bool), Some(false));
    }

    #[test]
    fn parses_commit_text_event() {
        let event = parse_voice_text_event(&json!({
            "kind": "commit_text",
            "text": "你好"
        }));
        assert!(matches!(event, Some(QianwenEvent::Final(text)) if text == "你好"));
    }

    #[test]
    fn parses_direct_results_json() {
        let events = parse_voice_results_json(
            r#"{"results":[{"state":"completed","text":"今天星期几？"}]}"#,
        );
        assert!(matches!(
            events.as_slice(),
            [QianwenEvent::Final(text)] if text == "今天星期几？"
        ));
    }

    #[test]
    fn parses_direct_text_events_json() {
        let events = parse_voice_text_events_json(
            r#"{"events":[{"kind":"partialText","partialText":"今天"},{"kind":"commitText","commitText":"今天星期几？"}]}"#,
        );
        assert!(matches!(
            events.as_slice(),
            [QianwenEvent::Volatile(partial), QianwenEvent::Final(final_text)]
                if partial == "今天" && final_text == "今天星期几？"
        ));
    }

    #[test]
    fn parses_voice_history_entry() {
        let entry: VoiceHistoryEntry = serde_json::from_value(json!({
            "actionKind": "InsertText",
            "createdAtMs": 1782981137844_u64,
            "id": "0c820e1b-4b65-4457-8d93-237de9cb5c16",
            "inputText": "今天星期几？",
            "text": "今天星期几？"
        }))
        .unwrap();

        assert_eq!(
            entry.id.as_deref(),
            Some("0c820e1b-4b65-4457-8d93-237de9cb5c16")
        );
        assert_eq!(entry.created_at_ms, Some(1782981137844));
        assert_eq!(entry.final_text().as_deref(), Some("今天星期几？"));
    }
}
