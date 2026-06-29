use chrono::{DateTime, Local};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AudioChannel {
    Mic,
    Speaker,
    File,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AudioRange {
    pub start: f64,
    pub end: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TranscriptSource {
    pub lang: String,
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub confidence: Option<TranscriptConfidence>,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct TranscriptConfidence {
    pub mean: f64,
    pub min: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TranscriptTarget {
    pub lang: String,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TranscriptEvent {
    pub seq: u64,
    pub channel: AudioChannel,
    pub timestamp: DateTime<Local>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub audio: Option<AudioRange>,
    pub src: TranscriptSource,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dst: Option<TranscriptTarget>,
}

impl TranscriptEvent {
    pub fn jsonl(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum LiveEvent {
    Meta(LiveMetaEvent),
    Volatile(LiveVolatileEvent),
    Finalized(TranscriptEvent),
    Translated(LiveTranslatedEvent),
    Eof,
}

impl LiveEvent {
    pub fn jsonl(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LiveMetaEvent {
    pub backend: String,
    pub src: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dst: Option<String>,
    pub mic: bool,
    pub speaker: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub devices: Vec<LiveDeviceEvent>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LiveDeviceEvent {
    pub channel: AudioChannel,
    pub name: String,
    pub pinned: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LiveVolatileEvent {
    pub channel: AudioChannel,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LiveTranslatedEvent {
    pub seq: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lang: Option<String>,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AudioInput {
    File(PathBuf),
    Bytes {
        data: Vec<u8>,
        filename: String,
        mime_type: Option<String>,
    },
}

impl AudioInput {
    pub fn filename(&self) -> String {
        match self {
            Self::File(path) => path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("audio")
                .to_owned(),
            Self::Bytes { filename, .. } => filename.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn transcript_event_json_omits_optional_fields_when_absent() {
        let event = TranscriptEvent {
            seq: 0,
            channel: AudioChannel::File,
            timestamp: Local.timestamp_opt(0, 0).single().unwrap(),
            audio: None,
            src: TranscriptSource {
                lang: "en-US".to_owned(),
                text: "hello".to_owned(),
                confidence: None,
            },
            dst: None,
        };

        let json = event.jsonl().unwrap();
        assert!(json.contains(r#""channel":"file""#));
        assert!(json.contains(r#""text":"hello""#));
        assert!(!json.contains(r#""audio""#));
        assert!(!json.contains(r#""dst""#));
    }

    #[test]
    fn live_event_json_uses_tagged_shape() {
        let event = LiveEvent::Volatile(LiveVolatileEvent {
            channel: AudioChannel::Mic,
            text: "partial".to_owned(),
        });

        let json = event.jsonl().unwrap();
        assert!(json.contains(r#""type":"volatile""#));
        assert!(json.contains(r#""channel":"mic""#));
    }
}
