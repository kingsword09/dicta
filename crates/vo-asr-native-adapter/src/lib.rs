use async_trait::async_trait;
use serde_json::Value;
use std::path::PathBuf;
use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use vo_asr::{
    AsrCapabilities, AsrError, AsrOptions, AsrProvider, AsrResult, LiveAsrOptions, LiveAsrProvider,
    LiveCapabilities, LiveEventCallback, LiveModeKind, Transcript,
};
use vo_core::{AudioInput, LiveEvent, TranscriptEvent};

#[derive(Debug, Clone)]
pub struct NativeAdapterConfig {
    pub command: PathBuf,
}

#[derive(Debug, Clone)]
pub struct NativeAdapterAsr {
    config: NativeAdapterConfig,
}

impl NativeAdapterAsr {
    pub fn new(config: NativeAdapterConfig) -> AsrResult<Self> {
        if config.command.as_os_str().is_empty() {
            return Err(AsrError::Config(
                "native adapter command is required".to_owned(),
            ));
        }
        Ok(Self { config })
    }

    pub async fn run_live(&self, options: NativeLiveOptions) -> AsrResult<()> {
        let mut command = self.live_command(options, false, false, true);

        let status = command
            .stdin(Stdio::inherit())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .status()
            .await
            .map_err(|err| {
                AsrError::Request(format!(
                    "failed to run native adapter {}: {err}",
                    self.config.command.display()
                ))
            })?;

        if !status.success() {
            return Err(AsrError::Request(format!(
                "native adapter exited with {status}"
            )));
        }

        Ok(())
    }

    pub async fn run_live_events<F>(
        &self,
        options: NativeLiveOptions,
        mut on_event: F,
    ) -> AsrResult<()>
    where
        F: FnMut(LiveEvent) -> AsrResult<()>,
    {
        let mut command = self.live_command(options, true, true, false);
        let mut child = command
            .stdin(Stdio::inherit())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|err| {
                AsrError::Request(format!(
                    "failed to run native adapter {}: {err}",
                    self.config.command.display()
                ))
            })?;

        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| AsrError::Request("native adapter stdout was not piped".to_owned()))?;
        let mut lines = BufReader::new(stdout).lines();
        let mut interrupted = false;
        let mut ctrl_c = Box::pin(tokio::signal::ctrl_c());
        loop {
            tokio::select! {
                biased;
                signal = &mut ctrl_c => {
                    signal.map_err(|err| {
                        AsrError::Request(format!("failed to listen for Ctrl-C: {err}"))
                    })?;
                    interrupted = true;
                    let _ = child.start_kill();
                    on_event(LiveEvent::Eof)?;
                    break;
                }
                line = lines.next_line() => {
                    let Some(line) = line.map_err(|err| {
                        AsrError::Request(format!("failed to read native adapter stdout: {err}"))
                    })? else {
                        break;
                    };
                    let line = line.trim();
                    if line.is_empty() {
                        continue;
                    }
                    let event = parse_live_event(line)?;
                    on_event(event)?;
                }
            }
        }

        let status = child.wait().await.map_err(|err| {
            AsrError::Request(format!(
                "failed to wait for native adapter {}: {err}",
                self.config.command.display()
            ))
        })?;
        if !interrupted && !status.success() {
            return Err(AsrError::Request(format!(
                "native adapter exited with {status}"
            )));
        }

        Ok(())
    }

    fn live_command(
        &self,
        options: NativeLiveOptions,
        force_json: bool,
        event_json: bool,
        pass_transcript: bool,
    ) -> Command {
        let mut command = Command::new(&self.config.command);

        if let Some(src) = options.src {
            command.arg("--src").arg(src);
        }
        if let Some(dst) = options.dst {
            command.arg("--dst").arg(dst);
        }
        if options.json || force_json {
            command.arg("--json");
        }
        if event_json {
            command.arg("--event-json");
        }
        if pass_transcript {
            if let Some(transcript) = options.transcript {
                command.arg("--transcript").arg(transcript);
            }
        }
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

        command
    }
}

fn parse_live_event(line: &str) -> AsrResult<LiveEvent> {
    if let Ok(event) = serde_json::from_str::<LiveEvent>(line) {
        return Ok(event);
    }

    let event: TranscriptEvent = serde_json::from_str(line).map_err(|err| {
        AsrError::InvalidResponse(format!("invalid apple live JSONL event: {err}: {line}"))
    })?;
    Ok(LiveEvent::Finalized(event))
}

#[derive(Debug, Clone)]
pub struct NativeLiveOptions {
    pub src: Option<String>,
    pub dst: Option<String>,
    pub json: bool,
    pub transcript: Option<PathBuf>,
    pub mic: bool,
    pub speaker: bool,
    pub voice_processing: bool,
    pub select_device: bool,
}

#[async_trait]
impl AsrProvider for NativeAdapterAsr {
    async fn transcribe(&self, input: AudioInput, options: AsrOptions) -> AsrResult<Transcript> {
        let AudioInput::File(path) = input else {
            return Err(AsrError::Input(
                "native adapter only accepts file input".to_owned(),
            ));
        };

        let mut command = Command::new(&self.config.command);
        command.arg("--input").arg(&path).arg("--json");
        if let Some(language) = options.language {
            command.arg("--src").arg(language);
        }

        let output = command.output().await.map_err(|err| {
            AsrError::Request(format!(
                "failed to run native adapter {}: {err}",
                self.config.command.display()
            ))
        })?;
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);

        if !output.status.success() {
            return Err(AsrError::Request(format!(
                "native adapter exited with {}: {}",
                output.status,
                stderr.trim()
            )));
        }

        parse_adapter_jsonl(&stdout)
    }

    fn name(&self) -> &'static str {
        "native-adapter"
    }

    fn capabilities(&self) -> AsrCapabilities {
        AsrCapabilities {
            batch_file: true,
            streaming: false,
            requires_network: false,
        }
    }
}

#[async_trait]
impl LiveAsrProvider for NativeAdapterAsr {
    async fn run_live(
        &self,
        options: LiveAsrOptions,
        on_event: LiveEventCallback<'_>,
    ) -> AsrResult<()> {
        self.run_live_events(NativeLiveOptions::from(options), |event| on_event(event))
            .await
    }

    fn live_name(&self) -> &'static str {
        "apple"
    }

    fn live_capabilities(&self) -> LiveCapabilities {
        native_adapter_live_capabilities()
    }
}

pub fn native_adapter_live_capabilities() -> LiveCapabilities {
    LiveCapabilities {
        mode: LiveModeKind::Streaming,
        mic: true,
        speaker: true,
        streaming_audio: true,
        partial_results: true,
        finalized_results: true,
        translation: true,
        voice_processing: true,
        device_selection: true,
        requires_network: false,
        expected_latency: None,
    }
}

impl From<LiveAsrOptions> for NativeLiveOptions {
    fn from(options: LiveAsrOptions) -> Self {
        Self {
            src: options.src,
            dst: options.dst,
            json: true,
            transcript: None,
            mic: options.mic,
            speaker: options.speaker,
            voice_processing: options.voice_processing,
            select_device: options.select_device,
        }
    }
}

fn parse_adapter_jsonl(stdout: &str) -> AsrResult<Transcript> {
    let mut text = Vec::new();
    let mut language = None;

    for line in stdout.lines().filter(|line| !line.trim().is_empty()) {
        let value: Value = serde_json::from_str(line)
            .map_err(|err| AsrError::InvalidResponse(format!("{err}: {line}")))?;
        let Some(src) = value.get("src") else {
            continue;
        };
        if language.is_none() {
            language = src
                .get("lang")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned);
        }
        if let Some(chunk) = src.get("text").and_then(Value::as_str) {
            if !chunk.trim().is_empty() {
                text.push(chunk.trim().to_owned());
            }
        }
    }

    if text.is_empty() {
        return Err(AsrError::InvalidResponse(
            "native adapter produced no transcript text".to_owned(),
        ));
    }

    Ok(Transcript {
        text: text.join("\n"),
        language,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_adapter_live_capabilities_are_streaming() {
        let capabilities = native_adapter_live_capabilities();

        assert_eq!(capabilities.mode, LiveModeKind::Streaming);
        assert!(capabilities.mic);
        assert!(capabilities.speaker);
        assert!(capabilities.streaming_audio);
        assert!(capabilities.partial_results);
        assert!(capabilities.finalized_results);
        assert!(capabilities.translation);
        assert!(capabilities.voice_processing);
        assert!(capabilities.device_selection);
        assert!(!capabilities.requires_network);
    }

    #[test]
    fn parses_adapter_jsonl_source_text() {
        let transcript = parse_adapter_jsonl(
            r#"{"seq":0,"channel":"file","timestamp":"2026-01-01T00:00:00+00:00","src":{"lang":"en-US","text":"hello"}}
{"seq":1,"channel":"file","timestamp":"2026-01-01T00:00:01+00:00","src":{"lang":"en-US","text":"world"}}"#,
        )
        .unwrap();

        assert_eq!(transcript.text, "hello\nworld");
        assert_eq!(transcript.language.as_deref(), Some("en-US"));
    }

    #[test]
    fn rejects_empty_adapter_output() {
        let err = parse_adapter_jsonl("").unwrap_err();

        assert!(matches!(err, AsrError::InvalidResponse(_)));
    }

    #[test]
    fn parses_typed_live_event() {
        let event =
            parse_live_event(r#"{"type":"volatile","channel":"mic","text":"hel"}"#).unwrap();

        assert!(matches!(event, LiveEvent::Volatile(_)));
    }

    #[test]
    fn parses_typed_status_live_event() {
        let event = parse_live_event(
            r#"{"type":"status","phase":"recording","message":"recording 3s chunk"}"#,
        )
        .unwrap();

        assert!(matches!(event, LiveEvent::Status(_)));
    }

    #[test]
    fn wraps_adapter_live_transcript_event() {
        let event = parse_live_event(
            r#"{"seq":0,"channel":"mic","timestamp":"2026-01-01T00:00:00+00:00","src":{"lang":"en-US","text":"hello"}}"#,
        )
        .unwrap();

        assert!(matches!(event, LiveEvent::Finalized(_)));
    }
}
