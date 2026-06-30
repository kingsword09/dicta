use jiff::{
    fmt::temporal::{DateTimeParser, DateTimePrinter},
    tz::TimeZone,
    Timestamp,
};
use serde::de;
use serde::{Deserialize, Serialize};
use std::fmt;
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
    pub timestamp: EventTimestamp,
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventTimestamp {
    value: Timestamp,
    formatted: String,
}

impl EventTimestamp {
    pub fn now() -> Self {
        Self::from_timestamp_with_system_offset(Timestamp::now())
    }

    pub fn from_unix_second(seconds: i64) -> Result<Self, jiff::Error> {
        Timestamp::from_second(seconds).map(Self::from_timestamp)
    }

    pub fn from_timestamp(value: Timestamp) -> Self {
        let formatted = value.to_string();
        Self { value, formatted }
    }

    pub fn from_timestamp_with_system_offset(value: Timestamp) -> Self {
        let offset = value.to_zoned(TimeZone::system()).offset();
        let formatted = DateTimePrinter::new().timestamp_with_offset_to_string(&value, offset);
        Self { value, formatted }
    }

    pub fn format_local(&self, format: &str) -> String {
        self.value
            .to_zoned(TimeZone::system())
            .strftime(format)
            .to_string()
    }

    pub fn local_stamp_now() -> String {
        Timestamp::now()
            .to_zoned(TimeZone::system())
            .strftime("%Y-%m-%d-%H%M%S")
            .to_string()
    }

    pub fn as_timestamp(&self) -> Timestamp {
        self.value
    }
}

impl fmt::Display for EventTimestamp {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.formatted)
    }
}

impl Serialize for EventTimestamp {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.formatted)
    }
}

impl<'de> Deserialize<'de> for EventTimestamp {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct EventTimestampVisitor;

        impl de::Visitor<'_> for EventTimestampVisitor {
            type Value = EventTimestamp;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("an RFC 3339 timestamp string with an offset")
            }

            fn visit_str<E: de::Error>(self, value: &str) -> Result<Self::Value, E> {
                let timestamp = DateTimeParser::new()
                    .parse_timestamp(value)
                    .map_err(E::custom)?;
                Ok(EventTimestamp {
                    value: timestamp,
                    formatted: value.to_owned(),
                })
            }
        }

        deserializer.deserialize_str(EventTimestampVisitor)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum LiveEvent {
    Meta(LiveMetaEvent),
    Status(LiveStatusEvent),
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum LiveStatusPhase {
    Recording,
    Transcribing,
    Recovering,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LiveStatusEvent {
    pub phase: LiveStatusPhase,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
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

    #[test]
    fn transcript_event_json_omits_optional_fields_when_absent() {
        let event = TranscriptEvent {
            seq: 0,
            channel: AudioChannel::File,
            timestamp: EventTimestamp::from_unix_second(0).unwrap(),
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
        assert!(json.contains(r#""timestamp":"1970-01-01T00:00:00Z""#));
        assert!(json.contains(r#""text":"hello""#));
        assert!(!json.contains(r#""audio""#));
        assert!(!json.contains(r#""dst""#));
    }

    #[test]
    fn transcript_event_json_preserves_input_timestamp_string() {
        let json = r#"{"seq":0,"channel":"file","timestamp":"2026-01-01T00:00:00+00:00","src":{"lang":"en-US","text":"hello"}}"#;

        let event: TranscriptEvent = serde_json::from_str(json).unwrap();
        assert_eq!(serde_json::to_string(&event).unwrap(), json);
        assert_eq!(
            event.timestamp.as_timestamp().to_string(),
            "2026-01-01T00:00:00Z"
        );
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

    #[test]
    fn live_status_event_json_uses_tagged_shape() {
        let event = LiveEvent::Status(LiveStatusEvent {
            phase: LiveStatusPhase::Recording,
            message: "recording 3s chunk".to_owned(),
            detail: None,
        });

        let json = event.jsonl().unwrap();
        assert!(json.contains(r#""type":"status""#));
        assert!(json.contains(r#""phase":"recording""#));
        assert!(json.contains(r#""message":"recording 3s chunk""#));
        assert!(!json.contains(r#""detail""#));
    }
}
