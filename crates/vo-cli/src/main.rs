use anyhow::{bail, Context, Result};
use chrono::Local;
use clap::{Parser, ValueEnum};
use serde::Serialize;
use std::fs::OpenOptions;
use std::io::{self, IsTerminal, Write};
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use std::{cmp, env};
use vo_asr::{AsrOptions, AsrProvider};
use vo_asr_doubao::{DoubaoAsr, DoubaoConfig};
use vo_asr_native_adapter::{NativeAdapterAsr, NativeAdapterConfig, NativeLiveOptions};
use vo_asr_openai_compatible::{OpenAiCompatibleAsr, OpenAiCompatibleConfig};
use vo_core::{
    AudioChannel, AudioInput, LiveEvent, LiveMetaEvent, LiveTranslatedEvent, LiveVolatileEvent,
    TranscriptEvent, TranscriptSource, TranscriptTarget,
};

#[derive(Debug, Parser)]
#[command(name = "vo")]
#[command(version)]
#[command(about = "Cross-platform transcription CLI with pluggable ASR providers")]
struct Cli {
    #[arg(long, value_enum, default_value_t = AsrBackend::Auto, env = "VO_ASR_BACKEND", help = "ASR backend to use")]
    asr: AsrBackend,

    #[arg(
        long = "api-base",
        env = "VO_ASR_API_BASE",
        help = "Provider API base URL"
    )]
    api_base: Option<String>,

    #[arg(long = "api-key", env = "VO_ASR_API_KEY", help = "Provider API key")]
    api_key: Option<String>,

    #[arg(
        long = "api-model",
        env = "VO_ASR_API_MODEL",
        help = "Provider model id"
    )]
    api_model: Option<String>,

    #[arg(long, env = "VO_SRC", help = "Source language/locale hint")]
    src: Option<String>,

    #[arg(
        long = "doubao-credential-path",
        env = "VO_DOUBAO_CREDENTIAL_PATH",
        help = "Path for cached Doubao IME device credentials"
    )]
    doubao_credential_path: Option<PathBuf>,

    #[arg(
        long = "doubao-device-id",
        env = "VO_DOUBAO_DEVICE_ID",
        help = "Override Doubao IME device id"
    )]
    doubao_device_id: Option<String>,

    #[arg(
        long = "doubao-token",
        env = "VO_DOUBAO_TOKEN",
        help = "Override Doubao IME ASR token"
    )]
    doubao_token: Option<String>,

    #[arg(
        long,
        env = "VO_DST",
        help = "Target language/locale for Apple on-device live translation"
    )]
    dst: Option<String>,

    #[arg(
        long = "native-adapter",
        env = "VO_NATIVE_ADAPTER",
        help = "Path to the native adapter binary used for platform on-device ASR"
    )]
    native_adapter: Option<PathBuf>,

    #[arg(long = "apple-adapter", env = "VO_APPLE_ADAPTER", hide = true)]
    apple_adapter: Option<PathBuf>,

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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum AsrBackend {
    Auto,
    OpenaiCompatible,
    Doubao,
    Apple,
}

impl AsrBackend {
    fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::OpenaiCompatible => "openai-compatible",
            Self::Doubao => "doubao",
            Self::Apple => "apple",
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
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
    validate_live_options(cli, backend)?;

    match backend {
        AsrBackend::Apple => run_apple_live(cli).await,
        AsrBackend::Doubao => run_doubao_live(cli).await,
        AsrBackend::OpenaiCompatible | AsrBackend::Auto => {
            bail!("interactive live mode currently supports apple and doubao")
        }
    }
}

fn validate_live_options(cli: &Cli, backend: AsrBackend) -> Result<()> {
    if cli.input.is_some() || cli.mic_duration.is_some() {
        bail!("--live cannot be combined with --input or --mic-duration");
    }
    if cli.no_mic && cli.no_speaker {
        bail!("--live cannot disable both --no-mic and --no-speaker");
    }
    if let Some(path) = &cli.transcript {
        validate_transcript_path(path)?;
    }
    if backend == AsrBackend::Doubao {
        if cli.dst.is_some() {
            bail!("--dst is only supported with Apple live mode");
        }
        if cli.no_mic {
            bail!("doubao live mode requires microphone input");
        }
        if cli.voice_processing || cli.select_device {
            bail!("--voice-processing and --select-device are only supported with Apple live mode");
        }
    }
    Ok(())
}

fn resolve_live_backend(cli: &Cli) -> Result<AsrBackend> {
    let support = apple_support();
    resolve_live_backend_for(cli, &support)
}

fn resolve_live_backend_for(cli: &Cli, apple_support: &AppleSupport) -> Result<AsrBackend> {
    match cli.asr {
        AsrBackend::Auto | AsrBackend::Apple => {
            if apple_support.supported {
                Ok(AsrBackend::Apple)
            } else if cli.asr == AsrBackend::Auto {
                Ok(AsrBackend::Doubao)
            } else {
                bail!(
                    "interactive live mode requires Apple on-device ASR: {}. Use --input or --mic-duration with --asr doubao on this system",
                    apple_support.reason
                )
            }
        }
        AsrBackend::Doubao => Ok(AsrBackend::Doubao),
        AsrBackend::OpenaiCompatible => {
            bail!("interactive live mode currently supports --asr apple or --asr doubao")
        }
    }
}

async fn run_apple_live(cli: &Cli) -> Result<()> {
    let adapter = build_native_adapter(cli)?;
    let mut renderer = LiveRenderer::new(
        cli.json,
        cli.transcript.clone(),
        !cli.no_mic && !cli.no_speaker,
        cli.src.clone(),
        cli.dst.clone(),
    )?;

    adapter
        .run_live_events(
            NativeLiveOptions {
                src: cli.src.clone(),
                dst: cli.dst.clone(),
                json: true,
                transcript: None,
                mic: !cli.no_mic,
                speaker: !cli.no_speaker,
                voice_processing: cli.voice_processing,
                select_device: cli.select_device,
            },
            |event| {
                renderer.handle_live_event(event).map_err(|err| {
                    vo_asr::AsrError::Request(format!("failed to render apple live event: {err}"))
                })
            },
        )
        .await
        .context("apple live transcription failed")?;

    renderer.finalize_session_log()?;
    renderer.print_summary();
    Ok(())
}

struct OutputPayload {
    text: String,
    event: TranscriptEvent,
}

impl OutputPayload {
    fn new(
        transcript: vo_asr::Transcript,
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
            timestamp: Local::now(),
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

    fn new_live(seq: u64, transcript: vo_asr::Transcript, src_hint: Option<String>) -> Self {
        let lang = transcript
            .language
            .clone()
            .or(src_hint)
            .unwrap_or_else(|| "und".to_owned());
        let text = transcript.text;
        let event = TranscriptEvent {
            seq,
            channel: AudioChannel::Mic,
            timestamp: Local::now(),
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
            volatile: Vec::new(),
            live_region_lines: 0,
        })
    }

    fn print_banner(&mut self, backend: &str, mic: bool, speaker: bool, translating: bool) {
        if self.json_mode || self.banner_printed {
            return;
        }
        self.print_banner_header(backend, mic, speaker, translating);
        println!();
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
            "vo {} - listening on {channels}{provider}{suffix} ({langs})",
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

    fn emit_event(&mut self, event: &TranscriptEvent) -> Result<()> {
        self.handle_live_event(LiveEvent::Finalized(event.clone()))
    }

    fn handle_live_event(&mut self, event: LiveEvent) -> Result<()> {
        match event {
            LiveEvent::Meta(meta) => {
                self.apply_meta(meta);
                Ok(())
            }
            LiveEvent::Volatile(volatile) => {
                self.set_volatile(volatile);
                self.redraw_live_region();
                Ok(())
            }
            LiveEvent::Finalized(event) => {
                self.clear_volatile(event.channel);
                if event.dst.is_some() || self.dst_lang.is_none() {
                    self.commit_event(event)
                } else {
                    self.pending.push(event);
                    self.redraw_live_region();
                    Ok(())
                }
            }
            LiveEvent::Translated(translated) => self.apply_translation(translated),
            LiveEvent::Eof => {
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
        let timestamp = event.timestamp.format("%H:%M:%S");
        let color = channel_color(event.channel);
        let timestamp = ansi256(color, &timestamp.to_string());
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
        if self.show_channel_label {
            16
        } else {
            11
        }
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
            "vo-{}-{}.jsonl",
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
            suggested_path: PathBuf::from(format!("./vo-{}.jsonl", transcript_stamp())),
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
    Local::now().format("%Y-%m-%d-%H%M%S").to_string()
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

async fn run_doubao_live(cli: &Cli) -> Result<()> {
    let provider = DoubaoAsr::new(DoubaoConfig {
        credential_path: cli.doubao_credential_path.clone(),
        device_id: non_empty(&cli.doubao_device_id),
        token: non_empty(&cli.doubao_token),
    })?;
    let mut renderer = LiveRenderer::new(
        cli.json,
        cli.transcript.clone(),
        false,
        cli.src.clone(),
        None,
    )?;
    renderer.print_banner("doubao", true, false, false);

    let mut seq = 0_u64;
    loop {
        let chunk_path = temp_recording_path();
        let record_result = tokio::task::spawn_blocking({
            let chunk_path = chunk_path.clone();
            move || vo_audio::record_default_input_to_wav(&chunk_path, Duration::from_secs(5))
        })
        .await
        .context("doubao live recorder task failed")?;

        if let Err(err) = record_result {
            let _ = std::fs::remove_file(&chunk_path);
            return Err(err).context("failed to record default microphone");
        }

        match provider
            .transcribe(
                AudioInput::File(chunk_path.clone()),
                AsrOptions {
                    language: cli.src.clone(),
                    ..AsrOptions::default()
                },
            )
            .await
        {
            Ok(transcript) => {
                let text = transcript.text.trim();
                if !text.is_empty() {
                    let payload = OutputPayload::new_live(seq, transcript, cli.src.clone());
                    renderer.emit_event(&payload.event)?;
                    seq += 1;
                }
            }
            Err(err) => {
                eprintln!("vo: doubao live chunk failed: {err}");
            }
        }

        let _ = std::fs::remove_file(&chunk_path);

        if has_pending_ctrl_c().await? {
            break;
        }
    }

    renderer.finalize_session_log()?;
    renderer.print_summary();
    Ok(())
}

async fn has_pending_ctrl_c() -> Result<bool> {
    tokio::select! {
        result = tokio::signal::ctrl_c() => {
            result.context("failed to listen for Ctrl-C")?;
            Ok(true)
        }
        _ = tokio::time::sleep(Duration::from_millis(1)) => Ok(false),
    }
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
    api_base_configured: bool,
    api_key_configured: bool,
    model: String,
    doubao_credential_path: Option<String>,
    doubao_device_id_configured: bool,
    doubao_token_configured: bool,
    native_adapter: Option<String>,
    error: Option<String>,
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

fn gather_doctor_report(cli: &Cli) -> DoctorReport {
    gather_doctor_report_with_audio(cli, default_audio_report())
}

fn default_audio_report() -> AudioReport {
    match vo_audio::default_input_device_info() {
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
    let backend_result = resolve_backend_for(cli, &apple_support);
    let resolved = backend_result
        .as_ref()
        .ok()
        .map(|backend| backend.as_str().to_owned());
    let error = match &backend_result {
        Ok(backend) => validate_backend_config(*backend, cli),
        Err(err) => Some(err.to_string()),
    };
    let model = non_empty(&cli.api_model).unwrap_or_else(|| {
        resolved
            .as_deref()
            .map(default_model_for_name)
            .unwrap_or("whisper-1")
            .to_owned()
    });

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
            api_base_configured: non_empty(&cli.api_base).is_some(),
            api_key_configured: non_empty(&cli.api_key).is_some(),
            model,
            doubao_credential_path: cli
                .doubao_credential_path
                .clone()
                .or_else(vo_asr_doubao::default_credential_path)
                .map(|path| path.display().to_string()),
            doubao_device_id_configured: non_empty(&cli.doubao_device_id).is_some(),
            doubao_token_configured: non_empty(&cli.doubao_token).is_some(),
            native_adapter: resolve_configured_native_adapter(cli)
                .as_ref()
                .map(|path| path.display().to_string()),
            error,
        },
        audio,
        runtime: RuntimeReport {
            python_sidecar_required: false,
            web_direct_available: true,
            native_adapter_supported_os: true,
        },
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
            reason: format!("macOS {version} is below 26; use doubao or another HTTP provider"),
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
    if let Some(error) = &report.backend.error {
        println!("  Error: {error}");
    }
    println!(
        "  API base configured: {}",
        yes_no(report.backend.api_base_configured)
    );
    println!(
        "  API key configured: {}",
        yes_no(report.backend.api_key_configured)
    );
    println!("  Model: {}", report.backend.model);
    if let Some(path) = &report.backend.doubao_credential_path {
        println!("  Doubao credential path: {path}");
    }
    println!(
        "  Doubao device id configured: {}",
        yes_no(report.backend.doubao_device_id_configured)
    );
    println!(
        "  Doubao token configured: {}",
        yes_no(report.backend.doubao_token_configured)
    );
    if let Some(adapter) = &report.backend.native_adapter {
        println!("  Native adapter: {adapter}");
    }

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
    }

    println!("\nRuntime");
    println!(
        "  Python sidecar required: {}",
        yes_no(report.runtime.python_sidecar_required)
    );
    println!(
        "  Web direct available: {}",
        yes_no(report.runtime.web_direct_available)
    );
    println!(
        "  Native adapter supported OS family: {}",
        yes_no(report.runtime.native_adapter_supported_os)
    );
}

fn yes_no(value: bool) -> &'static str {
    if value {
        "yes"
    } else {
        "no"
    }
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
            eprintln!("vo: recording default microphone for {seconds:.1}s...");
            vo_audio::record_default_input_to_wav(&path, Duration::from_secs_f64(seconds))
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
    std::env::temp_dir().join(format!("vo-mic-{millis}.wav"))
}

fn resolve_backend(cli: &Cli) -> Result<AsrBackend> {
    let support = apple_support();
    resolve_backend_for(cli, &support)
}

fn resolve_backend_for(cli: &Cli, apple_support: &AppleSupport) -> Result<AsrBackend> {
    match cli.asr {
        AsrBackend::Auto => {
            if let Some(model) = non_empty(&cli.api_model) {
                return if is_doubao_model_alias(&model) {
                    Ok(AsrBackend::Doubao)
                } else {
                    Ok(AsrBackend::OpenaiCompatible)
                };
            }
            if apple_support.supported {
                Ok(AsrBackend::OpenaiCompatible)
            } else {
                Ok(AsrBackend::Doubao)
            }
        }
        AsrBackend::OpenaiCompatible | AsrBackend::Doubao => Ok(cli.asr),
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
            let base_url =
                non_empty(&cli.api_base).unwrap_or_else(|| default_openai_api_base().to_owned());
            let model =
                non_empty(&cli.api_model).unwrap_or_else(|| default_model(backend).to_owned());
            Ok(Box::new(OpenAiCompatibleAsr::new(
                OpenAiCompatibleConfig {
                    base_url,
                    api_key: non_empty(&cli.api_key),
                    model,
                },
            )?))
        }
        AsrBackend::Doubao => Ok(Box::new(DoubaoAsr::new(DoubaoConfig {
            credential_path: cli.doubao_credential_path.clone(),
            device_id: non_empty(&cli.doubao_device_id),
            token: non_empty(&cli.doubao_token),
        })?)),
        AsrBackend::Apple => Ok(Box::new(build_native_adapter(cli)?)),
        AsrBackend::Auto => unreachable!("backend must be resolved first"),
    }
}

fn build_native_adapter(cli: &Cli) -> Result<NativeAdapterAsr> {
    let command = resolve_native_adapter(cli).context(
        "--asr apple requires --native-adapter, VO_NATIVE_ADAPTER, or a bundled native adapter binary next to vo",
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
    cli.native_adapter
        .clone()
        .or_else(|| cli.apple_adapter.clone())
}

fn bundled_native_adapter_names() -> &'static [&'static str] {
    if cfg!(windows) {
        &[
            "vo-adapter-apple-speech.exe",
            "vo-adapter-windows-speech.exe",
            "vo-apple-adapter.exe",
        ]
    } else {
        &["vo-adapter-apple-speech", "vo-apple-adapter"]
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

fn validate_backend_config(backend: AsrBackend, cli: &Cli) -> Option<String> {
    match backend {
        AsrBackend::Apple if resolve_native_adapter(cli).is_none() => Some(
            "--asr apple requires --native-adapter, VO_NATIVE_ADAPTER, or a bundled native adapter binary next to vo"
                .to_owned(),
        ),
        AsrBackend::OpenaiCompatible | AsrBackend::Doubao | AsrBackend::Apple => None,
        AsrBackend::Auto => None,
    }
}

fn default_model(backend: AsrBackend) -> &'static str {
    match backend {
        AsrBackend::Doubao => vo_asr_doubao::DEFAULT_MODEL,
        _ => "whisper-1",
    }
}

fn default_model_for_name(backend: &str) -> &'static str {
    match backend {
        "doubao" => vo_asr_doubao::DEFAULT_MODEL,
        _ => "whisper-1",
    }
}

fn is_doubao_model_alias(model: &str) -> bool {
    matches!(model, vo_asr_doubao::DEFAULT_MODEL | "doubao-asr")
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    fn test_cli() -> Cli {
        Cli {
            asr: AsrBackend::Auto,
            api_base: None,
            api_key: None,
            api_model: None,
            src: None,
            doubao_credential_path: None,
            doubao_device_id: None,
            doubao_token: None,
            dst: None,
            native_adapter: None,
            apple_adapter: None,
            input: None,
            mic_duration: None,
            live: false,
            no_mic: false,
            no_speaker: false,
            voice_processing: false,
            select_device: false,
            json: false,
            transcript: None,
            doctor: false,
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

    #[test]
    fn auto_uses_doubao_when_apple_on_device_is_unavailable() {
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
            AsrBackend::Doubao
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
    fn auto_uses_doubao_when_model_is_doubao_default() {
        let cli = Cli {
            api_model: Some(vo_asr_doubao::DEFAULT_MODEL.to_owned()),
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
            AsrBackend::Doubao
        );
    }

    #[test]
    fn auto_uses_openai_compatible_when_another_model_is_configured() {
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
    fn auto_uses_doubao_when_model_is_compatibility_doubao_alias() {
        let cli = Cli {
            api_model: Some("doubao-asr".to_owned()),
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
            AsrBackend::Doubao
        );
    }

    #[test]
    fn doubao_builds_without_api_base_or_key() {
        let cli = Cli {
            asr: AsrBackend::Doubao,
            input: Some(PathBuf::from("audio.wav")),
            ..test_cli()
        };

        assert!(build_provider(AsrBackend::Doubao, &cli).is_ok());
    }

    #[test]
    fn doubao_uses_native_ime_model_id() {
        assert_eq!(vo_asr_doubao::DEFAULT_MODEL, "doubaoime-asr");
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
            native_adapter: Some(PathBuf::from("vo-adapter-apple-speech")),
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
        assert!(validate_backend_config(backend, &cli)
            .as_deref()
            .is_some_and(|error| error.contains("--native-adapter")));
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

        assert!(validate_batch_options(&cli)
            .unwrap_err()
            .to_string()
            .contains("--dst"));
    }

    #[test]
    fn batch_mode_rejects_live_only_capture_flags() {
        let cli = Cli {
            select_device: true,
            ..test_cli()
        };

        assert!(validate_batch_options(&cli)
            .unwrap_err()
            .to_string()
            .contains("--select-device"));
    }

    #[test]
    fn live_mode_requires_apple_backend() {
        let cli = Cli {
            live: true,
            asr: AsrBackend::OpenaiCompatible,
            ..test_cli()
        };

        assert!(resolve_live_backend(&cli)
            .unwrap_err()
            .to_string()
            .contains("--asr apple"));
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

        assert!(validate_live_options(&cli, AsrBackend::Apple)
            .unwrap_err()
            .to_string()
            .contains("--no-mic"));
    }

    #[test]
    fn doubao_live_rejects_apple_only_flags() {
        let cli = Cli {
            live: true,
            asr: AsrBackend::Doubao,
            select_device: true,
            ..test_cli()
        };

        assert!(validate_live_options(&cli, AsrBackend::Doubao)
            .unwrap_err()
            .to_string()
            .contains("--select-device"));
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
        assert_eq!(report.system.os, std::env::consts::OS);
        if std::env::consts::OS == "macos" {
            assert!(report.system.apple_on_device_reason.contains("macOS"));
        }
    }

    #[test]
    fn doctor_reports_native_doubao_without_api_requirement() {
        let cli = Cli {
            asr: AsrBackend::Doubao,
            json: true,
            doctor: true,
            ..test_cli()
        };

        let report = gather_doctor_report_with_audio(&cli, test_audio_report());

        assert_eq!(report.backend.resolved.as_deref(), Some("doubao"));
        assert_eq!(report.backend.model, vo_asr_doubao::DEFAULT_MODEL);
        assert!(!report.backend.api_base_configured);
        assert!(!report.backend.api_key_configured);
        assert!(report.backend.doubao_credential_path.is_some());
        assert!(report.backend.error.is_none());
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
            "vo-test-transcript-{}.txt",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let payload = OutputPayload::new(
            vo_asr::Transcript {
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
            .join("vo-definitely-missing-dir")
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
            timestamp: Local.timestamp_opt(1_700_000_000, 0).single().unwrap(),
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
            "vo-live-renderer-{}.jsonl",
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
    fn live_renderer_finalizes_explicit_transcript() {
        let path = std::env::temp_dir().join(format!(
            "vo-live-renderer-finalize-{}.jsonl",
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
            "vo-live-renderer-translation-{}.jsonl",
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
