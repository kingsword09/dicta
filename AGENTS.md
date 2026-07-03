# AGENTS.md

This file guides agents working in this repository.

## What This Is

`dicta` is a Rust-first, provider-based transcription project. The Rust workspace
is the main path for new work. Platform-native on-device implementations live
under `adapters/` and are launched through the native adapter protocol.

## Primary Rust Workflow

```bash
cargo build -p dicta-cli
cargo test --workspace
./scripts/build.sh
./scripts/test.sh
target/debug/dicta --help
target/debug/dicta --doctor
```

`dicta serve` exposes the selected batch ASR provider as a local
OpenAI-compatible HTTP API:

```bash
target/debug/dicta --provider active serve --host 127.0.0.1 --port 4777
```

It serves `GET /health`, `GET /v1/models`, and
`POST /v1/audio/transcriptions` with multipart `file`, `model`, `language`,
`prompt`, and `response_format=json|text`. Keep this a thin adapter over the
existing provider orchestration; do not move provider protocol logic into the
HTTP layer.

The Rust code is organized as:

| Path | Role |
|---|---|
| `crates/dicta-core` | Shared transcript schema and audio input types. Keep this portable and low-dependency so future Web/WASM or server code can reuse it. |
| `crates/dicta-asr` | Provider trait, ASR options, capabilities, and provider-level errors. |
| `crates/dicta-asr-openai-compatible` | Direct multipart client for OpenAI-compatible `/v1/audio/transcriptions` APIs. |
| `crates/dicta-asr-native-adapter` | Provider-neutral bridge that invokes platform-native adapter binaries and parses JSONL output/events. |
| `crates/dicta-audio` | Cross-platform default microphone capture. It currently records fixed-duration WAV files through CPAL/Hound. |
| `crates/dicta-cli` | CLI argument parsing, provider orchestration, live rendering, transcript logging, and exit prompts. |
| `web/direct` | Dependency-free browser transcription module and integration demo for no-backend direct provider calls. |

## Current Rust CLI Surface

```bash
dicta --input PATH
   [--mic-duration SECONDS]
   [--asr auto|openai-compatible|apple]
   [--api-base URL]
   [--api-key KEY]
   [--api-model MODEL]
   [--src LOCALE]
   [--dst LOCALE]
   [--live]
   [--no-mic]
   [--no-speaker]
   [--voice-processing]
   [--select-device]
   [--native-adapter PATH]
   [--json]
   [--transcript PATH]
   [--doctor]
dicta serve
   [--host HOST]
   [--port PORT]
   [--cors-origin ORIGIN]
   [--max-upload-mb MIB]
```

`--input` and `--mic-duration` are mutually exclusive. Microphone mode records
the default input device to a temporary WAV and then submits that file to the
selected provider.

Without `--input` or `--mic-duration`, `dicta` enters live mode. Rust owns live TTY
rendering, finalized JSONL output, transcript logging, and exit prompts for Apple
and installed live provider packages.

`--doctor` bypasses audio-source validation and does not call providers. It
prints system, backend-resolution, API-config, default-input, Apple on-device
support, and runtime diagnostics. With `--json`, it emits a single
pretty-printed JSON object.

`serve` bypasses CLI audio-source validation and does not enter live mode. It
accepts OpenAI-compatible multipart batch transcription requests and returns
OpenAI-style JSON errors. It is intentionally batch-only: streaming,
timestamps, verbose JSON, SRT, and VTT should remain unsupported unless a
provider capability can supply those results honestly.

`--transcript PATH` writes the single finalized result after the provider
returns. With `--json`, it writes one JSONL event plus a trailing newline.
Without `--json`, it writes plain transcript text plus a trailing newline. In
live mode, `dicta-cli` owns the incremental renderer and session log; Apple
adapter volatile/meta events are internal and only drive the TTY view.

`auto` resolves batch transcription to the generic `openai-compatible` HTTP path
unless `--asr apple --native-adapter ...` is selected explicitly. In live mode it
selects Apple live only on supported macOS systems; other live providers must be
installed and selected through `dicta provider install` and `dicta provider set`.
`apple` is available only when the current OS supports Apple on-device ASR and
`--native-adapter` / `DICTA_NATIVE_ADAPTER` points to a native Apple Speech
adapter binary. On systems below macOS 26, `apple` reports that Apple on-device
mode is unavailable.

## Native Adapter Workflow

The current Apple Speech adapter lives in `adapters/apple-speech`. It requires
macOS 26, Apple Silicon, and Xcode 26 SDK for the Apple
`SpeechTranscriber`, `SpeechAnalyzer`, and `TranslationSession` APIs.

```bash
./scripts/build-apple-speech-adapter.sh
./scripts/test-apple-speech-adapter.sh
```

Use `--asr apple --native-adapter <path>` from the Rust CLI for batch file
transcription. Add `--live` when the Rust CLI should launch the adapter's live
mic/speaker capture. The Swift adapter is headless in that path and emits typed
events; Rust owns TTY rendering, transcript logging, and exit prompts.

## Python Sidecar Status

Python sidecars are not part of the runtime architecture. New work should not
add dependencies on Python, FastAPI, or provider-specific Python packages.
Private or unofficial ASR protocols belong in installable provider packages
outside this main repository so OpenAI-compatible and Apple providers do not
inherit that risk.

## Web Direct Mode

`web/direct` is intentionally static and dependency-free. Its primary API is
`dicta-transcriber.js`, which exposes Blob/File-to-transcript helpers for UI
components such as Web Components, plain JavaScript controls, or React voice
inputs. `index.html` is only an integration demo. The module sends `FormData`
directly to an OpenAI-compatible transcription endpoint and can also provide a
small `<dicta-speech-recorder>` custom element for simple browser use.

Do not use this mode for public hosted deployments that need secret protection.
Provider keys are visible inside the browser process. Keep provider calling
logic separate from UI widgets so framework integrations can reuse the same
transcription adapter.

## Design Principles

- KISS: prefer direct provider calls over local sidecars or extra services.
- YAGNI: do not add broader web server features, plugin loading, or live audio
  abstractions before the current CLI/server path needs them.
- DRY: shared schemas and provider contracts belong in `dicta-core` and `dicta-asr`.
- SOLID: provider protocol code, CLI orchestration, rendering, audio capture, and
  platform-specific adapters should stay separated. Keep Apple-specific behavior
  inside `adapters/apple-speech`; keep Rust-side native adapter process/protocol
  logic in `dicta-asr-native-adapter`.

## Testing Expectations

For Rust changes, run:

```bash
./scripts/test.sh
```

For Apple adapter changes, run:

```bash
./scripts/test-apple-speech-adapter.sh
```

Network ASR calls are not unit-tested directly. Keep provider URL building,
configuration validation, schema serialization, and backend resolution covered by
unit tests.
