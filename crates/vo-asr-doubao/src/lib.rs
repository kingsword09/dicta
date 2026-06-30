use async_trait::async_trait;
use futures_util::{SinkExt, StreamExt};
use opus::{Application, Channels, Encoder};
use prost::Message;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::fs;
use tokio_tungstenite::{
    connect_async,
    tungstenite::{client::IntoClientRequest, protocol::Message as WsMessage},
};
use uuid::Uuid;
use vo_asr::{
    AsrCapabilities, AsrError, AsrOptions, AsrProvider, AsrResult, LiveAsrOptions, LiveAsrProvider,
    LiveCapabilities, LiveEventCallback, LiveModeKind, ProviderCapabilities, Transcript,
};
use vo_core::{
    AudioChannel, AudioInput, EventTimestamp, LiveEvent, LiveMetaEvent, LiveStatusEvent,
    LiveStatusPhase, TranscriptEvent, TranscriptSource,
};

pub const DEFAULT_MODEL: &str = "doubaoime-asr";

const REGISTER_URL: &str = "https://log.snssdk.com/service/2/device_register/";
const SETTINGS_URL: &str = "https://is.snssdk.com/service/settings/v3/";
const WEBSOCKET_URL: &str = "wss://frontier-audio-ime-ws.doubao.com/ocean/api/v1/ws";
const AID: u32 = 401734;
const APP_NAME: &str = "oime";
const VERSION_CODE: u32 = 100102018;
const VERSION_NAME: &str = "1.1.2";
const CHANNEL: &str = "official";
const PACKAGE_NAME: &str = "com.bytedance.android.doubaoime";
const USER_AGENT: &str = "com.bytedance.android.doubaoime/100102018 (Linux; U; Android 16; en_US; Pixel 7 Pro; Build/BP2A.250605.031.A2; Cronet/TTNetVersion:94cf429a 2025-11-17 QuicVersion:1f89f732 2025-05-08)";
const SAMPLE_RATE: u32 = 16_000;
const CHANNELS: u16 = 1;
const FRAME_DURATION_MS: u32 = 20;

#[derive(Debug, Clone)]
pub struct DoubaoConfig {
    pub credential_path: Option<PathBuf>,
    pub device_id: Option<String>,
    pub token: Option<String>,
}

impl Default for DoubaoConfig {
    fn default() -> Self {
        Self {
            credential_path: default_credential_path(),
            device_id: None,
            token: None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct DoubaoAsr {
    client: Client,
    config: DoubaoConfig,
}

impl DoubaoAsr {
    pub fn new(mut config: DoubaoConfig) -> AsrResult<Self> {
        if config.credential_path.is_none() {
            config.credential_path = default_credential_path();
        }
        Ok(Self {
            client: Client::new(),
            config,
        })
    }

    async fn credentials(&self) -> AsrResult<DeviceCredentials> {
        let mut credentials = self.load_credentials().await.unwrap_or_default();

        if let Some(device_id) = &self.config.device_id {
            credentials.device_id = Some(device_id.clone());
        }
        if let Some(token) = &self.config.token {
            credentials.token = Some(token.clone());
        }

        let mut changed = false;
        if credentials
            .device_id
            .as_deref()
            .filter(|s| !s.is_empty())
            .is_none()
        {
            credentials = self.register_device().await?;
            changed = true;
        }

        if credentials
            .token
            .as_deref()
            .filter(|s| !s.is_empty())
            .is_none()
        {
            let device_id = credentials
                .device_id
                .clone()
                .ok_or_else(|| AsrError::Config("doubao device_id is missing".to_owned()))?;
            let token = self
                .get_asr_token(&device_id, credentials.cdid.as_deref())
                .await?;
            credentials.token = Some(token);
            changed = true;
        }

        if changed {
            self.save_credentials(&credentials).await?;
        }

        Ok(credentials)
    }

    async fn load_credentials(&self) -> Option<DeviceCredentials> {
        let path = self.config.credential_path.as_ref()?;
        let bytes = fs::read(path).await.ok()?;
        serde_json::from_slice(&bytes).ok()
    }

    async fn save_credentials(&self, credentials: &DeviceCredentials) -> AsrResult<()> {
        let Some(path) = &self.config.credential_path else {
            return Ok(());
        };
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).await.map_err(|err| {
                AsrError::Config(format!(
                    "failed to create doubao credential directory {}: {err}",
                    parent.display()
                ))
            })?;
        }
        let json = serde_json::to_vec_pretty(credentials).map_err(|err| {
            AsrError::InvalidResponse(format!("failed to serialize doubao credentials: {err}"))
        })?;
        fs::write(path, json).await.map_err(|err| {
            AsrError::Config(format!(
                "failed to write doubao credentials {}: {err}",
                path.display()
            ))
        })
    }

    async fn register_device(&self) -> AsrResult<DeviceCredentials> {
        let cdid = Uuid::new_v4().to_string();
        let openudid = random_hex_8();
        let clientudid = Uuid::new_v4().to_string();

        let params = device_register_params(&cdid);
        let body = json!({
            "magic_tag": "ss_app_log",
            "header": device_register_header(&cdid, &openudid, &clientudid),
            "_gen_time": now_millis(),
        });

        let response = self
            .client
            .post(REGISTER_URL)
            .query(&params)
            .header("User-Agent", USER_AGENT)
            .json(&body)
            .send()
            .await
            .map_err(|err| {
                AsrError::Request(format!("doubao device registration failed: {err}"))
            })?;
        let status = response.status();
        let body = response.text().await.map_err(|err| {
            AsrError::Request(format!(
                "failed to read doubao registration response: {err}"
            ))
        })?;
        if !status.is_success() {
            return Err(AsrError::Request(format!(
                "doubao device registration HTTP {status}: {body}"
            )));
        }

        let parsed: RegisterResponse = serde_json::from_str(&body).map_err(|err| {
            AsrError::InvalidResponse(format!(
                "invalid doubao registration response: {err}: {body}"
            ))
        })?;
        if parsed.device_id == 0 {
            return Err(AsrError::InvalidResponse(
                "doubao registration returned empty device_id".to_owned(),
            ));
        }

        Ok(DeviceCredentials {
            device_id: Some(parsed.device_id.to_string()),
            install_id: Some(parsed.install_id.to_string()),
            cdid: Some(cdid),
            openudid: Some(openudid),
            clientudid: Some(clientudid),
            token: None,
        })
    }

    async fn get_asr_token(&self, device_id: &str, cdid: Option<&str>) -> AsrResult<String> {
        let owned_cdid;
        let cdid = match cdid {
            Some(value) if !value.is_empty() => value,
            _ => {
                owned_cdid = Uuid::new_v4().to_string();
                &owned_cdid
            }
        };
        let params = settings_params(device_id, cdid);
        let body = "body=null";
        let x_ss_stub = format!("{:x}", md5::compute(body.as_bytes())).to_uppercase();

        let response = self
            .client
            .post(SETTINGS_URL)
            .query(&params)
            .header("User-Agent", USER_AGENT)
            .header("x-ss-stub", x_ss_stub)
            .body(body.to_owned())
            .send()
            .await
            .map_err(|err| AsrError::Request(format!("doubao token request failed: {err}")))?;
        let status = response.status();
        let body = response.text().await.map_err(|err| {
            AsrError::Request(format!("failed to read doubao token response: {err}"))
        })?;
        if !status.is_success() {
            return Err(AsrError::Request(format!(
                "doubao token HTTP {status}: {body}"
            )));
        }

        let parsed: SettingsResponse = serde_json::from_str(&body).map_err(|err| {
            AsrError::InvalidResponse(format!("invalid doubao token response: {err}: {body}"))
        })?;
        let token = parsed.data.settings.asr_config.app_key.trim().to_owned();
        if token.is_empty() {
            return Err(AsrError::InvalidResponse(
                "doubao token response did not contain app_key".to_owned(),
            ));
        }
        Ok(token)
    }

    async fn transcribe_pcm(&self, pcm: &[i16], options: AsrOptions) -> AsrResult<Transcript> {
        if pcm.is_empty() {
            return Err(AsrError::Input("audio input is empty".to_owned()));
        }

        let credentials = self.credentials().await?;
        let device_id = credentials
            .device_id
            .as_deref()
            .ok_or_else(|| AsrError::Config("doubao device_id is missing".to_owned()))?;
        let token = credentials
            .token
            .as_deref()
            .ok_or_else(|| AsrError::Config("doubao token is missing".to_owned()))?;
        let request_id = Uuid::new_v4().to_string();
        let url = format!("{WEBSOCKET_URL}?aid={AID}&device_id={device_id}");
        let mut request = url
            .into_client_request()
            .map_err(|err| AsrError::Config(format!("invalid doubao websocket URL: {err}")))?;
        {
            let headers = request.headers_mut();
            headers.insert(
                "User-Agent",
                USER_AGENT.parse().map_err(|err| {
                    AsrError::Config(format!("invalid doubao user agent header: {err}"))
                })?,
            );
            headers.insert(
                "proto-version",
                "v2".parse().map_err(|err| {
                    AsrError::Config(format!("invalid doubao proto-version header: {err}"))
                })?,
            );
            headers.insert(
                "x-custom-keepalive",
                "true".parse().map_err(|err| {
                    AsrError::Config(format!("invalid doubao keepalive header: {err}"))
                })?,
            );
        }

        let (mut ws, _) = connect_async(request)
            .await
            .map_err(|err| AsrError::Request(format!("doubao websocket connect failed: {err}")))?;

        send_pb(&mut ws, start_task(&request_id, token)).await?;
        let started = receive_response(&mut ws).await?;
        if matches!(started.kind, ResponseKind::Error) {
            return Err(AsrError::Request(format!(
                "doubao StartTask failed: {}",
                started.error_msg
            )));
        }

        send_pb(&mut ws, start_session(&request_id, token, device_id)?).await?;
        let session_started = receive_response(&mut ws).await?;
        if matches!(session_started.kind, ResponseKind::Error) {
            return Err(AsrError::Request(format!(
                "doubao StartSession failed: {}",
                session_started.error_msg
            )));
        }

        let frames = encode_opus_frames(pcm)?;
        let last_index = frames.len().saturating_sub(1);
        for (index, frame) in frames.into_iter().enumerate() {
            let frame_state = if index == 0 {
                FrameState::First
            } else if index == last_index {
                FrameState::Last
            } else {
                FrameState::Middle
            };
            send_pb(
                &mut ws,
                task_request(&request_id, frame, frame_state, now_millis()),
            )
            .await?;
        }
        send_pb(&mut ws, finish_session(&request_id, token)).await?;

        let mut final_text = None;
        while let Some(message) = ws.next().await {
            let message = message.map_err(|err| {
                AsrError::Request(format!("doubao websocket receive failed: {err}"))
            })?;
            let parsed = parse_ws_message(message)?;
            match parsed.kind {
                ResponseKind::Final => final_text = Some(parsed.text),
                ResponseKind::Error => {
                    return Err(AsrError::Request(format!(
                        "doubao ASR failed: {}",
                        parsed.error_msg
                    )))
                }
                ResponseKind::SessionFinished => break,
                ResponseKind::TaskStarted
                | ResponseKind::SessionStarted
                | ResponseKind::Interim
                | ResponseKind::Heartbeat
                | ResponseKind::Unknown => {}
            }
        }

        let text = final_text
            .map(|text| text.trim().to_owned())
            .filter(|text| !text.is_empty())
            .unwrap_or_default();

        Ok(Transcript {
            text,
            language: options.language,
        })
    }
}

#[async_trait]
impl AsrProvider for DoubaoAsr {
    async fn transcribe(&self, input: AudioInput, options: AsrOptions) -> AsrResult<Transcript> {
        let pcm = match input {
            AudioInput::File(path) => read_wav_as_16k_mono(&path)?,
            AudioInput::Bytes { data, filename, .. } => {
                read_wav_bytes_as_16k_mono(&data, &filename)?
            }
        };
        self.transcribe_pcm(&pcm, options).await
    }

    fn name(&self) -> &'static str {
        "doubao"
    }

    fn capabilities(&self) -> AsrCapabilities {
        doubao_capabilities().batch
    }

    fn provider_capabilities(&self) -> ProviderCapabilities {
        doubao_capabilities()
    }
}

#[async_trait]
impl LiveAsrProvider for DoubaoAsr {
    async fn run_live(
        &self,
        options: LiveAsrOptions,
        on_event: LiveEventCallback<'_>,
    ) -> AsrResult<()> {
        validate_live_options(&options)?;
        let src = options.src.clone().unwrap_or_else(|| "und".to_owned());
        on_event(LiveEvent::Meta(LiveMetaEvent {
            backend: self.live_name().to_owned(),
            src: src.clone(),
            dst: None,
            mic: true,
            speaker: false,
            devices: Vec::new(),
        }))?;

        let mut seq = 0_u64;
        loop {
            let chunk_path = temp_recording_path();
            on_event(LiveEvent::Status(LiveStatusEvent {
                phase: LiveStatusPhase::Recording,
                message: format!(
                    "recording {} chunk",
                    format_status_duration(options.chunk_duration)
                ),
                detail: None,
            }))?;
            let record_task = tokio::task::spawn_blocking({
                let chunk_path = chunk_path.clone();
                let chunk_duration = options.chunk_duration;
                move || vo_audio::record_default_input_to_wav(&chunk_path, chunk_duration)
            });
            let record_result = tokio::select! {
                result = record_task => result.map_err(|err| {
                    AsrError::Request(format!("doubao live recorder task failed: {err}"))
                })?,
                result = tokio::signal::ctrl_c() => {
                    result.map_err(|err| {
                        AsrError::Request(format!("failed to listen for Ctrl-C: {err}"))
                    })?;
                    let _ = std::fs::remove_file(&chunk_path);
                    break;
                }
            };

            if let Err(err) = record_result {
                let _ = std::fs::remove_file(&chunk_path);
                return Err(AsrError::Input(format!(
                    "failed to record default microphone: {err}"
                )));
            }

            on_event(LiveEvent::Status(LiveStatusEvent {
                phase: LiveStatusPhase::Transcribing,
                message: "transcribing chunk".to_owned(),
                detail: None,
            }))?;
            let transcribe_result = tokio::select! {
                result = self.transcribe(
                    AudioInput::File(chunk_path.clone()),
                    AsrOptions {
                        language: options.src.clone(),
                        ..AsrOptions::default()
                    },
                ) => result,
                result = tokio::signal::ctrl_c() => {
                    result.map_err(|err| {
                        AsrError::Request(format!("failed to listen for Ctrl-C: {err}"))
                    })?;
                    let _ = std::fs::remove_file(&chunk_path);
                    break;
                }
            };

            match transcribe_result {
                Ok(transcript) => {
                    let text = transcript.text.trim();
                    if !text.is_empty() {
                        on_event(LiveEvent::Finalized(TranscriptEvent {
                            seq,
                            channel: AudioChannel::Mic,
                            timestamp: EventTimestamp::now(),
                            audio: None,
                            src: TranscriptSource {
                                lang: transcript.language.unwrap_or_else(|| src.clone()),
                                text: text.to_owned(),
                                confidence: None,
                            },
                            dst: None,
                        }))?;
                        seq += 1;
                    }
                }
                Err(err) => {
                    on_event(LiveEvent::Status(LiveStatusEvent {
                        phase: LiveStatusPhase::Recovering,
                        message: "chunk failed; continuing".to_owned(),
                        detail: Some(err.to_string()),
                    }))?;
                }
            }

            let _ = std::fs::remove_file(&chunk_path);
        }

        on_event(LiveEvent::Eof)?;
        Ok(())
    }

    fn live_name(&self) -> &'static str {
        "doubao"
    }

    fn live_capabilities(&self) -> LiveCapabilities {
        doubao_live_capabilities()
    }
}

pub fn doubao_capabilities() -> ProviderCapabilities {
    ProviderCapabilities {
        batch: AsrCapabilities {
            batch_file: true,
            streaming: false,
            requires_network: true,
        },
        live: Some(doubao_live_capabilities()),
        notes: vec![
            "Doubao credentials can be auto-registered and cached locally.".to_owned(),
            "Doubao live mode is chunked and emits finalized text only.".to_owned(),
        ],
    }
}

pub fn doubao_live_capabilities() -> LiveCapabilities {
    LiveCapabilities {
        mode: LiveModeKind::Chunked,
        mic: true,
        speaker: false,
        streaming_audio: false,
        partial_results: false,
        finalized_results: true,
        translation: false,
        voice_processing: false,
        device_selection: false,
        requires_network: true,
        expected_latency: Some(Duration::from_secs(5)),
    }
}

fn validate_live_options(options: &LiveAsrOptions) -> AsrResult<()> {
    if !options.mic {
        return Err(AsrError::Config(
            "doubao live mode requires microphone input".to_owned(),
        ));
    }
    if options.speaker {
        return Err(AsrError::Config(
            "doubao live mode does not support speaker capture".to_owned(),
        ));
    }
    if options.dst.is_some() {
        return Err(AsrError::Config(
            "doubao live mode does not support translation".to_owned(),
        ));
    }
    if options.voice_processing || options.select_device {
        return Err(AsrError::Config(
            "doubao live mode does not support Apple-only capture controls".to_owned(),
        ));
    }
    Ok(())
}

fn temp_recording_path() -> PathBuf {
    let millis = now_millis();
    std::env::temp_dir().join(format!("vo-doubao-live-{millis}.wav"))
}

fn format_status_duration(duration: Duration) -> String {
    if duration.subsec_millis() == 0 {
        format!("{}s", duration.as_secs())
    } else {
        format!("{:.1}s", duration.as_secs_f64())
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct DeviceCredentials {
    device_id: Option<String>,
    install_id: Option<String>,
    cdid: Option<String>,
    openudid: Option<String>,
    clientudid: Option<String>,
    token: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RegisterResponse {
    device_id: u64,
    install_id: u64,
}

#[derive(Debug, Deserialize)]
struct SettingsResponse {
    data: SettingsData,
}

#[derive(Debug, Deserialize)]
struct SettingsData {
    settings: Settings,
}

#[derive(Debug, Deserialize)]
struct Settings {
    asr_config: AsrConfigResponse,
}

#[derive(Debug, Deserialize)]
struct AsrConfigResponse {
    app_key: String,
}

#[derive(Clone, PartialEq, Message)]
struct AsrRequestPb {
    #[prost(string, tag = "2")]
    token: String,
    #[prost(string, tag = "3")]
    service_name: String,
    #[prost(string, tag = "5")]
    method_name: String,
    #[prost(string, tag = "6")]
    payload: String,
    #[prost(bytes, tag = "7")]
    audio_data: Vec<u8>,
    #[prost(string, tag = "8")]
    request_id: String,
    #[prost(enumeration = "FrameState", tag = "9")]
    frame_state: i32,
}

#[derive(Clone, PartialEq, Message)]
struct AsrResponsePb {
    #[prost(string, tag = "1")]
    request_id: String,
    #[prost(string, tag = "2")]
    task_id: String,
    #[prost(string, tag = "3")]
    service_name: String,
    #[prost(string, tag = "4")]
    message_type: String,
    #[prost(int32, tag = "5")]
    status_code: i32,
    #[prost(string, tag = "6")]
    status_message: String,
    #[prost(string, tag = "7")]
    result_json: String,
    #[prost(int32, tag = "9")]
    unknown_field_9: i32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, prost::Enumeration)]
#[repr(i32)]
enum FrameState {
    Unspecified = 0,
    First = 1,
    Middle = 3,
    Last = 9,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ResponseKind {
    TaskStarted,
    SessionStarted,
    SessionFinished,
    Interim,
    Final,
    Heartbeat,
    Error,
    Unknown,
}

#[derive(Debug, Clone)]
struct ParsedResponse {
    kind: ResponseKind,
    text: String,
    error_msg: String,
}

fn start_task(request_id: &str, token: &str) -> AsrRequestPb {
    AsrRequestPb {
        token: token.to_owned(),
        service_name: "ASR".to_owned(),
        method_name: "StartTask".to_owned(),
        request_id: request_id.to_owned(),
        ..Default::default()
    }
}

fn start_session(request_id: &str, token: &str, device_id: &str) -> AsrResult<AsrRequestPb> {
    let extra = json!({
        "app_name": "com.android.chrome",
        "cell_compress_rate": 8,
        "did": device_id,
        "enable_asr_threepass": true,
        "enable_asr_twopass": true,
        "input_mode": "tool",
    });

    let payload = json!({
        "audio_info": {
            "channel": CHANNELS,
            "format": "speech_opus",
            "sample_rate": SAMPLE_RATE,
        },
        "enable_punctuation": true,
        "enable_speech_rejection": false,
        "extra": extra,
    });
    Ok(AsrRequestPb {
        token: token.to_owned(),
        service_name: "ASR".to_owned(),
        method_name: "StartSession".to_owned(),
        payload: serde_json::to_string(&payload).map_err(|err| {
            AsrError::InvalidResponse(format!("failed to serialize doubao session config: {err}"))
        })?,
        request_id: request_id.to_owned(),
        ..Default::default()
    })
}

fn task_request(
    request_id: &str,
    audio_data: Vec<u8>,
    frame_state: FrameState,
    timestamp_ms: u128,
) -> AsrRequestPb {
    AsrRequestPb {
        service_name: "ASR".to_owned(),
        method_name: "TaskRequest".to_owned(),
        payload: json!({ "extra": {}, "timestamp_ms": timestamp_ms }).to_string(),
        audio_data,
        request_id: request_id.to_owned(),
        frame_state: frame_state as i32,
        ..Default::default()
    }
}

fn finish_session(request_id: &str, token: &str) -> AsrRequestPb {
    AsrRequestPb {
        token: token.to_owned(),
        service_name: "ASR".to_owned(),
        method_name: "FinishSession".to_owned(),
        request_id: request_id.to_owned(),
        ..Default::default()
    }
}

async fn send_pb<S>(ws: &mut S, request: AsrRequestPb) -> AsrResult<()>
where
    S: SinkExt<WsMessage> + Unpin,
    <S as futures_util::Sink<WsMessage>>::Error: std::fmt::Display,
{
    let mut bytes = Vec::new();
    request.encode(&mut bytes).map_err(|err| {
        AsrError::InvalidResponse(format!("failed to encode doubao protobuf request: {err}"))
    })?;
    ws.send(WsMessage::Binary(bytes.into()))
        .await
        .map_err(|err| AsrError::Request(format!("doubao websocket send failed: {err}")))
}

async fn receive_response<S>(ws: &mut S) -> AsrResult<ParsedResponse>
where
    S: StreamExt<Item = Result<WsMessage, tokio_tungstenite::tungstenite::Error>> + Unpin,
{
    let message = ws
        .next()
        .await
        .ok_or_else(|| AsrError::Request("doubao websocket closed".to_owned()))?
        .map_err(|err| AsrError::Request(format!("doubao websocket receive failed: {err}")))?;
    parse_ws_message(message)
}

fn parse_ws_message(message: WsMessage) -> AsrResult<ParsedResponse> {
    match message {
        WsMessage::Binary(bytes) => parse_response_pb(&bytes),
        WsMessage::Text(text) => Err(AsrError::InvalidResponse(format!(
            "doubao returned text websocket message: {text}"
        ))),
        WsMessage::Close(_) => Err(AsrError::Request("doubao websocket closed".to_owned())),
        WsMessage::Ping(_) | WsMessage::Pong(_) | WsMessage::Frame(_) => Ok(ParsedResponse {
            kind: ResponseKind::Heartbeat,
            text: String::new(),
            error_msg: String::new(),
        }),
    }
}

fn parse_response_pb(bytes: &[u8]) -> AsrResult<ParsedResponse> {
    let pb = AsrResponsePb::decode(bytes)
        .map_err(|err| AsrError::InvalidResponse(format!("invalid doubao protobuf: {err}")))?;
    match pb.message_type.as_str() {
        "TaskStarted" => Ok(kind(ResponseKind::TaskStarted)),
        "SessionStarted" => Ok(kind(ResponseKind::SessionStarted)),
        "SessionFinished" => Ok(kind(ResponseKind::SessionFinished)),
        "TaskFailed" | "SessionFailed" => Ok(ParsedResponse {
            kind: ResponseKind::Error,
            text: String::new(),
            error_msg: pb.status_message,
        }),
        _ if pb.result_json.trim().is_empty() => Ok(kind(ResponseKind::Unknown)),
        _ => parse_result_json(&pb.result_json),
    }
}

fn parse_result_json(result_json: &str) -> AsrResult<ParsedResponse> {
    let value: Value = serde_json::from_str(result_json).map_err(|err| {
        AsrError::InvalidResponse(format!("invalid doubao result JSON: {err}: {result_json}"))
    })?;
    let Some(results) = value.get("results").and_then(Value::as_array) else {
        return Ok(kind(ResponseKind::Heartbeat));
    };
    if value
        .get("extra")
        .and_then(|extra| extra.get("vad_start"))
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        return Ok(kind(ResponseKind::Interim));
    }

    let mut text = String::new();
    let mut is_interim = true;
    let mut vad_finished = false;
    let mut nonstream_result = false;

    for result in results {
        if let Some(value) = result.get("text").and_then(Value::as_str) {
            if !value.is_empty() {
                text = value.to_owned();
            }
        }
        if result.get("is_interim").and_then(Value::as_bool) == Some(false) {
            is_interim = false;
        }
        if result
            .get("is_vad_finished")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            vad_finished = true;
        }
        if result
            .get("extra")
            .and_then(|extra| extra.get("nonstream_result"))
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            nonstream_result = true;
        }
    }

    let response_kind = if nonstream_result || (!is_interim && vad_finished) {
        ResponseKind::Final
    } else {
        ResponseKind::Interim
    };
    Ok(ParsedResponse {
        kind: response_kind,
        text,
        error_msg: String::new(),
    })
}

fn kind(kind: ResponseKind) -> ParsedResponse {
    ParsedResponse {
        kind,
        text: String::new(),
        error_msg: String::new(),
    }
}

fn read_wav_as_16k_mono(path: &Path) -> AsrResult<Vec<i16>> {
    let reader = hound::WavReader::open(path).map_err(|err| {
        AsrError::Input(format!(
            "doubao currently accepts WAV input; failed to open {}: {err}",
            path.display()
        ))
    })?;
    read_wav_reader(reader, &path.display().to_string())
}

fn read_wav_bytes_as_16k_mono(bytes: &[u8], name: &str) -> AsrResult<Vec<i16>> {
    let cursor = std::io::Cursor::new(bytes);
    let reader = hound::WavReader::new(cursor).map_err(|err| {
        AsrError::Input(format!("doubao currently accepts WAV bytes: {name}: {err}"))
    })?;
    read_wav_reader(reader, name)
}

fn read_wav_reader<R>(mut reader: hound::WavReader<R>, name: &str) -> AsrResult<Vec<i16>>
where
    R: std::io::Read,
{
    let spec = reader.spec();
    if spec.sample_format != hound::SampleFormat::Int || spec.bits_per_sample != 16 {
        return Err(AsrError::Input(format!(
            "doubao currently accepts 16-bit PCM WAV input, got {:?} {}-bit in {name}",
            spec.sample_format, spec.bits_per_sample
        )));
    }
    let samples = reader
        .samples::<i16>()
        .collect::<Result<Vec<_>, _>>()
        .map_err(|err| AsrError::Input(format!("failed to read WAV samples from {name}: {err}")))?;
    Ok(resample_to_16k_mono(
        &samples,
        spec.sample_rate,
        spec.channels.max(1),
    ))
}

fn resample_to_16k_mono(samples: &[i16], input_rate: u32, input_channels: u16) -> Vec<i16> {
    let mono = to_mono(samples, input_channels);
    if input_rate == SAMPLE_RATE {
        return mono;
    }
    if mono.is_empty() {
        return mono;
    }

    let output_len =
        (mono.len() as u128 * SAMPLE_RATE as u128 / input_rate.max(1) as u128) as usize;
    let output_len = output_len.max(1);
    let ratio = input_rate as f64 / SAMPLE_RATE as f64;
    let mut output = Vec::with_capacity(output_len);

    for index in 0..output_len {
        let source = index as f64 * ratio;
        let lower = source.floor() as usize;
        let upper = (lower + 1).min(mono.len() - 1);
        let frac = source - lower as f64;
        let sample = mono[lower] as f64 * (1.0 - frac) + mono[upper] as f64 * frac;
        output.push(sample.round().clamp(i16::MIN as f64, i16::MAX as f64) as i16);
    }
    output
}

fn to_mono(samples: &[i16], channels: u16) -> Vec<i16> {
    if channels <= 1 {
        return samples.to_vec();
    }
    let channels = channels as usize;
    samples
        .chunks(channels)
        .map(|frame| {
            let sum: i32 = frame.iter().map(|sample| *sample as i32).sum();
            (sum / frame.len() as i32).clamp(i16::MIN as i32, i16::MAX as i32) as i16
        })
        .collect()
}

fn encode_opus_frames(pcm: &[i16]) -> AsrResult<Vec<Vec<u8>>> {
    let samples_per_frame = (SAMPLE_RATE * FRAME_DURATION_MS / 1000) as usize;
    let mut encoder = Encoder::new(SAMPLE_RATE, Channels::Mono, Application::Audio)
        .map_err(|err| AsrError::Config(format!("failed to create Opus encoder: {err}")))?;
    let mut frames = Vec::new();

    for chunk in pcm.chunks(samples_per_frame) {
        let mut frame = vec![0_i16; samples_per_frame];
        frame[..chunk.len()].copy_from_slice(chunk);
        let mut output = vec![0_u8; 4096];
        let len = encoder
            .encode(&frame, &mut output)
            .map_err(|err| AsrError::Request(format!("failed to encode Opus frame: {err}")))?;
        output.truncate(len);
        frames.push(output);
    }

    Ok(frames)
}

fn device_register_header(cdid: &str, openudid: &str, clientudid: &str) -> Value {
    json!({
        "device_id": 0,
        "install_id": 0,
        "aid": AID,
        "app_name": APP_NAME,
        "version_code": VERSION_CODE,
        "version_name": VERSION_NAME,
        "manifest_version_code": VERSION_CODE,
        "update_version_code": VERSION_CODE,
        "channel": CHANNEL,
        "package": PACKAGE_NAME,
        "device_platform": "android",
        "os": "android",
        "os_api": "34",
        "os_version": "16",
        "device_type": "Pixel 7 Pro",
        "device_brand": "google",
        "device_model": "Pixel 7 Pro",
        "resolution": "1080*2400",
        "dpi": "420",
        "language": "zh",
        "timezone": 8,
        "access": "wifi",
        "rom": "UP1A.231005.007",
        "rom_version": "UP1A.231005.007",
        "openudid": openudid,
        "clientudid": clientudid,
        "cdid": cdid,
        "region": "CN",
        "tz_name": "Asia/Shanghai",
        "tz_offset": 28800,
        "sim_region": "cn",
        "carrier_region": "cn",
        "cpu_abi": "arm64-v8a",
        "build_serial": "unknown",
        "not_request_sender": 0,
        "sig_hash": "",
        "google_aid": "",
        "mc": "",
        "serial_number": "",
    })
}

fn device_register_params(cdid: &str) -> Vec<(&'static str, String)> {
    vec![
        ("device_platform", "android".to_owned()),
        ("os", "android".to_owned()),
        ("ssmix", "a".to_owned()),
        ("_rticket", now_millis().to_string()),
        ("cdid", cdid.to_owned()),
        ("channel", CHANNEL.to_owned()),
        ("aid", AID.to_string()),
        ("app_name", APP_NAME.to_owned()),
        ("version_code", VERSION_CODE.to_string()),
        ("version_name", VERSION_NAME.to_owned()),
        ("manifest_version_code", VERSION_CODE.to_string()),
        ("update_version_code", VERSION_CODE.to_string()),
        ("resolution", "1080*2400".to_owned()),
        ("dpi", "420".to_owned()),
        ("device_type", "Pixel 7 Pro".to_owned()),
        ("device_brand", "google".to_owned()),
        ("language", "zh".to_owned()),
        ("os_api", "34".to_owned()),
        ("os_version", "16".to_owned()),
        ("ac", "wifi".to_owned()),
    ]
}

fn settings_params(device_id: &str, cdid: &str) -> Vec<(&'static str, String)> {
    vec![
        ("device_platform", "android".to_owned()),
        ("os", "android".to_owned()),
        ("ssmix", "a".to_owned()),
        ("_rticket", now_millis().to_string()),
        ("cdid", cdid.to_owned()),
        ("channel", CHANNEL.to_owned()),
        ("aid", AID.to_string()),
        ("app_name", APP_NAME.to_owned()),
        ("version_code", VERSION_CODE.to_string()),
        ("version_name", VERSION_NAME.to_owned()),
        ("device_id", device_id.to_owned()),
    ]
}

fn now_millis() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::from_secs(0))
        .as_millis()
}

fn random_hex_8() -> String {
    let id = Uuid::new_v4();
    let bytes = id.as_bytes();
    bytes[..8]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

pub fn default_credential_path() -> Option<PathBuf> {
    default_config_dir_from_values(
        cfg!(windows),
        env_path("HOME"),
        env_path("APPDATA"),
        env_path("USERPROFILE"),
    )
    .map(doubao_credential_path)
}

fn default_config_dir_from_values(
    is_windows: bool,
    home: Option<PathBuf>,
    appdata: Option<PathBuf>,
    userprofile: Option<PathBuf>,
) -> Option<PathBuf> {
    if is_windows {
        appdata
            .or_else(|| userprofile.map(|path| path.join("AppData").join("Roaming")))
            .or_else(|| home.map(|path| path.join(".config")))
    } else {
        home.map(|path| path.join(".config"))
    }
}

fn env_path(name: &str) -> Option<PathBuf> {
    let value = std::env::var_os(name)?;
    if value.as_os_str().is_empty() {
        None
    } else {
        Some(PathBuf::from(value))
    }
}

fn doubao_credential_path(config_dir: PathBuf) -> PathBuf {
    config_dir.join("vo").join("doubao-credentials.json")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn doubao_live_capabilities_are_chunked_final_only() {
        let capabilities = doubao_live_capabilities();

        assert_eq!(capabilities.mode, LiveModeKind::Chunked);
        assert!(capabilities.mic);
        assert!(!capabilities.speaker);
        assert!(!capabilities.streaming_audio);
        assert!(!capabilities.partial_results);
        assert!(capabilities.finalized_results);
        assert!(!capabilities.translation);
        assert!(capabilities.requires_network);
    }

    #[test]
    fn formats_live_status_duration_for_chunk_prompt() {
        assert_eq!(format_status_duration(Duration::from_secs(3)), "3s");
        assert_eq!(format_status_duration(Duration::from_millis(1500)), "1.5s");
    }

    #[test]
    fn defaults_to_cached_credentials_without_api_key() {
        let config = DoubaoConfig::default();

        assert_eq!(config.credential_path, default_credential_path());
        assert!(config.device_id.is_none());
        assert!(config.token.is_none());
    }

    #[test]
    fn uses_windows_appdata_for_default_config_dir() {
        let config_dir = default_config_dir_from_values(
            true,
            None,
            Some(PathBuf::from(r"C:\Users\runneradmin\AppData\Roaming")),
            Some(PathBuf::from(r"C:\Users\runneradmin")),
        );

        assert_eq!(
            config_dir,
            Some(PathBuf::from(r"C:\Users\runneradmin\AppData\Roaming"))
        );
    }

    #[test]
    fn falls_back_to_windows_userprofile_for_default_config_dir() {
        let config_dir = default_config_dir_from_values(
            true,
            None,
            None,
            Some(PathBuf::from(r"C:\Users\runneradmin")),
        );

        assert_eq!(
            config_dir,
            Some(
                PathBuf::from(r"C:\Users\runneradmin")
                    .join("AppData")
                    .join("Roaming")
            )
        );
    }

    #[test]
    fn downmixes_stereo_to_mono() {
        let mono = to_mono(&[100, 300, -100, -300], 2);

        assert_eq!(mono, vec![200, -200]);
    }

    #[test]
    fn keeps_16k_rate_when_resampling_not_needed() {
        let samples = vec![1, 2, 3];

        assert_eq!(resample_to_16k_mono(&samples, SAMPLE_RATE, 1), samples);
    }

    #[test]
    fn parses_final_result_json() {
        let parsed = parse_result_json(
            r#"{"results":[{"text":"hello","is_interim":false,"is_vad_finished":true}],"extra":{}}"#,
        )
        .unwrap();

        assert_eq!(parsed.kind, ResponseKind::Final);
        assert_eq!(parsed.text, "hello");
    }

    #[test]
    fn builds_start_task_protobuf() {
        let request = start_task("request-id", "token");
        let mut bytes = Vec::new();
        request.encode(&mut bytes).unwrap();
        let decoded = AsrRequestPb::decode(bytes.as_slice()).unwrap();

        assert_eq!(decoded.token, "token");
        assert_eq!(decoded.method_name, "StartTask");
        assert_eq!(decoded.request_id, "request-id");
    }
}
