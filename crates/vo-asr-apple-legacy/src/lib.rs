use async_trait::async_trait;
use serde_json::Value;
use std::path::PathBuf;
use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use vo_asr::{AsrCapabilities, AsrError, AsrOptions, AsrProvider, AsrResult, Transcript};
use vo_core::{AudioInput, LiveEvent, TranscriptEvent};

#[derive(Debug, Clone)]
pub struct AppleLegacyConfig {
    pub command: PathBuf,
}

#[derive(Debug, Clone)]
pub struct AppleLegacyAsr {
    config: AppleLegacyConfig,
}

impl AppleLegacyAsr {
    pub fn new(config: AppleLegacyConfig) -> AsrResult<Self> {
        if config.command.as_os_str().is_empty() {
            return Err(AsrError::Config(
                "apple legacy adapter command is required".to_owned(),
            ));
        }
        Ok(Self { config })
    }

    pub async fn run_live(&self, options: AppleLiveOptions) -> AsrResult<()> {
        let mut command = self.live_command(options, false, false, true);

        let status = command
            .stdin(Stdio::inherit())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .status()
            .await
            .map_err(|err| {
                AsrError::Request(format!(
                    "failed to run apple legacy adapter {}: {err}",
                    self.config.command.display()
                ))
            })?;

        if !status.success() {
            return Err(AsrError::Request(format!(
                "apple legacy adapter exited with {status}"
            )));
        }

        Ok(())
    }

    pub async fn run_live_events<F>(
        &self,
        options: AppleLiveOptions,
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
                    "failed to run apple legacy adapter {}: {err}",
                    self.config.command.display()
                ))
            })?;

        let stdout = child.stdout.take().ok_or_else(|| {
            AsrError::Request("apple legacy adapter stdout was not piped".to_owned())
        })?;
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
                        AsrError::Request(format!("failed to read apple legacy adapter stdout: {err}"))
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
                "failed to wait for apple legacy adapter {}: {err}",
                self.config.command.display()
            ))
        })?;
        if !interrupted && !status.success() {
            return Err(AsrError::Request(format!(
                "apple legacy adapter exited with {status}"
            )));
        }

        Ok(())
    }

    fn live_command(
        &self,
        options: AppleLiveOptions,
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
pub struct AppleLiveOptions {
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
impl AsrProvider for AppleLegacyAsr {
    async fn transcribe(&self, input: AudioInput, options: AsrOptions) -> AsrResult<Transcript> {
        let AudioInput::File(path) = input else {
            return Err(AsrError::Input(
                "apple legacy adapter only accepts file input".to_owned(),
            ));
        };

        let mut command = Command::new(&self.config.command);
        command.arg("--input").arg(&path).arg("--json");
        if let Some(language) = options.language {
            command.arg("--src").arg(language);
        }

        let output = command.output().await.map_err(|err| {
            AsrError::Request(format!(
                "failed to run apple legacy adapter {}: {err}",
                self.config.command.display()
            ))
        })?;
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);

        if !output.status.success() {
            return Err(AsrError::Request(format!(
                "apple legacy adapter exited with {}: {}",
                output.status,
                stderr.trim()
            )));
        }

        parse_legacy_jsonl(&stdout)
    }

    fn name(&self) -> &'static str {
        "apple-legacy"
    }

    fn capabilities(&self) -> AsrCapabilities {
        AsrCapabilities {
            batch_file: true,
            streaming: false,
            requires_network: false,
        }
    }
}

fn parse_legacy_jsonl(stdout: &str) -> AsrResult<Transcript> {
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
            "apple legacy adapter produced no transcript text".to_owned(),
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
    fn parses_legacy_jsonl_source_text() {
        let transcript = parse_legacy_jsonl(
            r#"{"seq":0,"channel":"file","timestamp":"2026-01-01T00:00:00+00:00","src":{"lang":"en-US","text":"hello"}}
{"seq":1,"channel":"file","timestamp":"2026-01-01T00:00:01+00:00","src":{"lang":"en-US","text":"world"}}"#,
        )
        .unwrap();

        assert_eq!(transcript.text, "hello\nworld");
        assert_eq!(transcript.language.as_deref(), Some("en-US"));
    }

    #[test]
    fn rejects_empty_legacy_output() {
        let err = parse_legacy_jsonl("").unwrap_err();

        assert!(matches!(err, AsrError::InvalidResponse(_)));
    }

    #[test]
    fn parses_typed_live_event() {
        let event =
            parse_live_event(r#"{"type":"volatile","channel":"mic","text":"hel"}"#).unwrap();

        assert!(matches!(event, LiveEvent::Volatile(_)));
    }

    #[test]
    fn wraps_legacy_live_transcript_event() {
        let event = parse_live_event(
            r#"{"seq":0,"channel":"mic","timestamp":"2026-01-01T00:00:00+00:00","src":{"lang":"en-US","text":"hello"}}"#,
        )
        .unwrap();

        assert!(matches!(event, LiveEvent::Finalized(_)));
    }
}
