# AGENTS.md

This file guides agents working in this repository.

## What This Is

`vo` is a Rust-first, provider-based transcription project. The Rust workspace
is the main path for new work. The old Swift macOS 26 implementation is isolated
under `legacy/apple-swift-adapter` as the optional Apple on-device adapter.

## Primary Rust Workflow

```bash
cargo build -p vo-cli
cargo test --workspace
./scripts/build.sh
./scripts/test.sh
target/debug/vo --help
target/debug/vo --doctor
```

The Rust code is organized as:

| Path | Role |
|---|---|
| `crates/vo-core` | Shared transcript schema and audio input types. Keep this portable and low-dependency so future Web/WASM or server code can reuse it. |
| `crates/vo-asr` | Provider trait, ASR options, capabilities, and provider-level errors. |
| `crates/vo-asr-openai-compatible` | Direct multipart client for OpenAI-compatible `/v1/audio/transcriptions` APIs. |
| `crates/vo-asr-apple-legacy` | Adapter that invokes the isolated Apple Swift macOS 26 `vo` binary and parses JSONL output. |
| `crates/vo-asr-doubao` | Rust-native Doubao IME ASR provider. It owns device registration, credential caching, Opus frame encoding, and protobuf WebSocket requests. |
| `crates/vo-audio` | Cross-platform default microphone capture. It currently records fixed-duration WAV files through CPAL/Hound. |
| `crates/vo-cli` | CLI argument parsing, provider orchestration, live rendering, transcript logging, and exit prompts. |
| `web/direct` | Static browser tool for no-backend direct provider calls. |

## Current Rust CLI Surface

```bash
vo --input PATH
   [--mic-duration SECONDS]
   [--asr auto|openai-compatible|doubao|apple]
   [--api-base URL]
   [--api-key KEY]
   [--api-model MODEL]
   [--doubao-credential-path PATH]
   [--doubao-device-id ID]
   [--doubao-token TOKEN]
   [--src LOCALE]
   [--dst LOCALE]
   [--live]
   [--no-mic]
   [--no-speaker]
   [--voice-processing]
   [--select-device]
   [--apple-adapter PATH]
   [--json]
   [--transcript PATH]
   [--doctor]
```

`--input` and `--mic-duration` are mutually exclusive. Microphone mode records
the default input device to a temporary WAV and then submits that file to the
selected provider.

Without `--input` or `--mic-duration`, `vo` enters live mode. Rust owns live TTY
rendering, finalized JSONL output, transcript logging, and exit prompts for both
Apple and Doubao.

`--doctor` bypasses audio-source validation and does not call providers. It
prints system, backend-resolution, API-config, default-input, Apple on-device
support, and runtime diagnostics. With `--json`, it emits a single
pretty-printed JSON object.

`--transcript PATH` writes the single finalized result after the provider
returns. With `--json`, it writes one JSONL event plus a trailing newline.
Without `--json`, it writes plain transcript text plus a trailing newline. In
live mode, `vo-cli` owns the incremental renderer and session log for both
Apple and Doubao; Apple adapter volatile/meta events are internal and only drive
the TTY view.

`auto` is platform-aware. In live mode it selects Apple live on macOS 26+ and
Doubao live where Apple on-device ASR is unavailable. In batch mode it resolves
to `doubao` on systems where Apple on-device ASR is not available; on macOS 26+
it keeps the generic `openai-compatible` HTTP path unless
`--asr apple --apple-adapter ...` is selected explicitly.
`--api-model doubaoime-asr` and legacy alias `--api-model doubao-asr` also
resolve to `doubao`; any other explicit model resolves to `openai-compatible`.
`doubao` is handled by `vo-asr-doubao` with default model `doubaoime-asr`. It
does not require `--api-base` or `--api-key`; first use auto-registers
credentials and caches them at `~/.config/vo/doubao-credentials.json` unless
overridden. `apple` is available only when the current OS supports Apple
on-device ASR and `--apple-adapter` / `VO_APPLE_ADAPTER` points to a macOS 26
Apple Swift adapter binary. On systems below macOS 26, `apple` reports that
Apple on-device mode is unavailable.

## Apple Adapter Workflow

The original Swift implementation is isolated in `legacy/apple-swift-adapter`.
It requires macOS 26, Apple Silicon, and Xcode 26 SDK for the Apple
`SpeechTranscriber`, `SpeechAnalyzer`, and `TranslationSession` APIs.

```bash
./scripts/build-apple-swift-adapter.sh
./scripts/test-apple-swift-adapter.sh
```

Use `--asr apple --apple-adapter <path>` from the Rust CLI for batch file
transcription. Add `--live` when the Rust CLI should launch the adapter's live
mic/speaker capture. The Swift adapter is headless in that path and emits typed
events; Rust owns TTY rendering, transcript logging, and exit prompts.

## Python Sidecar Status

`doubao_asr_api.py` is not part of the new runtime architecture. New work should
not add dependencies on Python, FastAPI, or the Python `doubaoime_asr` package.
Doubao support lives in `vo-asr-doubao` as a Rust-native protocol implementation.
The protocol is unofficial; keep the implementation isolated behind the provider
crate so OpenAI-compatible and Apple providers do not inherit that risk.

## Web Direct Mode

`web/direct/index.html` is intentionally static. It exists to support local
browser use without a backend or WASM. It sends `FormData` directly to an
OpenAI-compatible transcription endpoint. It supports selected audio files and
short browser microphone recordings through `MediaRecorder`. Keep it
dependency-free unless there is a concrete product requirement for a built
frontend.

Do not use this mode for public hosted deployments that need secret protection.
Provider keys are visible inside the browser process.

## Design Principles

- KISS: prefer direct provider calls over local sidecars or extra services.
- YAGNI: do not add web server, WASM, plugin loading, or live audio abstractions
  before the current CLI path needs them.
- DRY: shared schemas and provider contracts belong in `vo-core` and `vo-asr`.
- SOLID: provider protocol code, CLI orchestration, rendering, audio capture, and
  platform-specific adapters should stay separated. Keep Apple-specific behavior
  inside `legacy/apple-swift-adapter` or `vo-asr-apple-legacy`; do not spread
  macOS 26 API checks through provider-neutral crates.

## Testing Expectations

For Rust changes, run:

```bash
./scripts/test.sh
```

For Apple adapter changes, run:

```bash
./scripts/test-apple-swift-adapter.sh
```

Network ASR calls are not unit-tested directly. Keep provider URL building,
configuration validation, schema serialization, and backend resolution covered by
unit tests.
