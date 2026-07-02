# dicta

`dicta` is a Rust-first transcription toolkit for live and file-based speech
transcription. It supports Doubao, OpenAI-compatible providers, Apple on-device
speech on supported macOS systems, and browser integration paths.

## Install

```console
$ curl -fsSL https://raw.githubusercontent.com/kingsword09/dicta/main/install.sh | sh
$ curl -fsSL https://raw.githubusercontent.com/kingsword09/dicta/main/install.sh \
    | DICTA_VERSION=0.10.0 DICTA_INSTALL_DIR="$HOME/bin" sh
```

The installer downloads the matching GitHub Release archive and installs `dicta`
into `~/.local/bin` by default. Release assets cover macOS arm64, Linux
x86_64/arm64, and Windows x86_64/arm64. macOS arm64 archives also include the
Apple Speech adapter.

Update or remove an installed release:

```console
$ dicta update
$ dicta update --version 0.10.4
$ dicta uninstall
$ dicta uninstall --yes
```

`dicta update` installs into the directory containing the running `dicta` binary unless
`--install-dir` or `DICTA_INSTALL_DIR` is set. `dicta uninstall` removes `dicta`,
`dicta-tray`, and bundled adapter binaries from that install directory. User
configuration under `~/.config/dicta` is left in place.

## Usage

Live mode:

```console
$ dicta                                  # macOS 26: Apple on-device; otherwise Doubao
$ dicta --src en-US --dst ja-JP          # Transcribe and translate
$ dicta --no-speaker                     # Mic only
$ dicta --no-mic --src en-US --dst ja-JP # Speaker only on Apple mode
$ dicta --select-device                  # Pick and pin mic / speaker at startup
$ dicta --json | jq                      # JSONL output for piping
$ dicta --capabilities                   # ASR capability diagnostics
$ dicta --doctor                         # Environment diagnostics
```

Status bar mode:

```console
$ dicta --ui                             # Open the status bar provider switcher
$ dicta --ui --live                      # Open the switcher and start live mode
$ dicta --ui --provider doubao --live    # Set the initial provider, then start live
```

The status bar UI is the Rust `dicta-tray` companion binary. Left-clicking the
status item opens a compact provider panel; right-clicking keeps a native menu
fallback. It lists built-in and configured provider profiles and runs live
transcription as a supervised `dicta --provider active --live` worker. Without a
saved active provider, `active` defaults to Apple on supported macOS systems and
Doubao elsewhere. Switching providers from the panel stores the active selection
in `~/.config/dicta/active-provider.json` and restarts the worker, so a failed
provider can be replaced without leaving the status bar UI.

Doubao does not require an API key:

```console
$ dicta --asr doubao --src zh-CN
$ dicta --asr doubao --json
$ dicta --asr doubao --src zh-CN --live-chunk 3
$ dicta --input meeting.wav --asr doubao --src zh-CN
$ dicta --mic-duration 5 \
    --asr doubao \
    --src zh-CN
```

Qianwen Shell live transcription:

```console
$ dicta --asr qianwen \
    --live \
    --qianwen-host-bundle-path ../qw \
    --src zh-CN
$ DICTA_QIANWEN_HOST_BUNDLE_PATH=/Users/kingsword09/Documents/code/ai/qw \
    dicta --provider qianwen --live --src zh-CN
```

Qianwen support loads the local `libQianwenShellEmbedded.dylib` runtime from a
supplied `qw` bundle. Configure it with `--qianwen-host-bundle-path` or
`--qianwen-runtime-path`. When the bundle does not expose Qianwen's WSG signing
library, `dicta` supplies a small local shim backed by `libqianwen_unet_runtime.dylib`.
Use `--qianwen-wsg-impl-path` only to override that path, or set
`DICTA_QIANWEN_ASR_QUERY_SIGN` for the runtime's direct ASR query-sign debug hook.
Batch file transcription is not exposed by this local voice-input runtime.

OpenAI-compatible transcription:

```console
$ dicta --input meeting.wav \
    --asr openai-compatible \
    --api-base https://api.openai.com \
    --api-key "$DICTA_ASR_API_KEY" \
    --api-model whisper-1
$ dicta --mic-duration 5 \
    --asr openai-compatible \
    --api-base https://api.openai.com \
    --api-key "$DICTA_ASR_API_KEY" \
    --api-model whisper-1
$ dicta --input meeting.wav --provider openai
```

Named provider profiles let OpenAI-compatible services share the same Rust
implementation without adding a new backend. Built-in profiles can be selected
with `--provider`; custom profiles are loaded from `~/.config/dicta/providers.toml`
or `--provider-config` and may use either `api_key_env` or direct `api_key`.

Installable provider packages:

```console
$ dicta provider install ./doubaoime-asr-0.1.0.tgz
$ dicta provider install ./qianwenime-asr
$ dicta provider list
$ dicta provider set qianwenime-asr
```

`dicta provider install` accepts a local provider directory, a `.tgz`, or an
npm registry package name. It downloads and unpacks the provider package
directly into Dicta's provider directory; it does not run `npm install`, does
not create `node_modules`, and does not require Node at runtime.

Local OpenAI-compatible ASR server:

```console
$ dicta --provider active serve --cors-origin http://localhost:5173
$ curl -s http://127.0.0.1:4777/health
$ curl -s \
    -F file=@meeting.wav \
    -F model=dicta \
    -F language=zh \
    http://127.0.0.1:4777/v1/audio/transcriptions
```

`dicta serve` exposes the selected provider through
`POST /v1/audio/transcriptions` for local projects that want to use `fetch`,
OpenAI-style multipart uploads, or a small backend gateway. It accepts `file`,
`model`, `language`, `prompt`, and `response_format=json|text`. Use `model=dicta`
or `model=default` to keep the server's configured provider model. Streaming,
timestamp granularities, SRT/VTT, and verbose JSON are intentionally not
advertised until a provider capability can back them.

Apple on-device mode requires macOS 26 and an Apple Speech adapter:

```console
$ dicta --live \
    --asr apple \
    --native-adapter adapters/apple-speech/.build/release/dicta-adapter-apple-speech \
    --src en-US
$ dicta --input meeting.wav \
    --asr apple \
    --native-adapter adapters/apple-speech/.build/release/dicta-adapter-apple-speech \
    --src en-US
```

Output:

```console
$ dicta --input meeting.wav --json
$ dicta --input meeting.wav --transcript meeting.txt
$ dicta --input meeting.wav --json --transcript meeting.jsonl
$ dicta --doctor
$ dicta --doctor --json
$ dicta --capabilities
$ dicta --capabilities --asr doubao --json
$ dicta --capabilities --provider openai --json
$ dicta provider list
$ dicta provider current
$ dicta provider set doubao
```

`--input` and `--mic-duration` are mutually exclusive. Without either flag, `dicta`
enters live mode. Environment variables mirror the provider flags; run
`dicta --help`, `dicta --capabilities`, and `dicta --doctor` for local details.

## Web

For browser integration without a backend:

```console
$ python3 -m http.server 8765
$ open http://127.0.0.1:8765/web/direct/
```

`web/direct` provides a dependency-free browser transcription module plus a
static integration demo for Web Components, plain JavaScript, and React voice UI
components such as AI Elements `SpeechInput`. `crates/dicta-web` is the browser
WASM boundary for provider, microphone, and storage APIs. See
[docs/web-direct.md](docs/web-direct.md) and [docs/web-wasm.md](docs/web-wasm.md).

## Development

```console
$ ./scripts/build.sh
$ ./scripts/test.sh
$ ./scripts/build-apple-speech-adapter.sh
$ ./scripts/test-apple-speech-adapter.sh
```

## Architecture

```text
crates/
  dicta-core/                    shared transcript schema
  dicta-asr*/                    ASR provider crates
  dicta-audio/                   microphone recording
  dicta-cli/                     command-line entry point
  dicta-tray/                    Rust status bar provider switcher
  dicta-web/                     browser WASM provider/audio/storage APIs
web/direct/                   static browser direct-provider tool
adapters/apple-speech/        macOS 26 Apple Speech adapter
```

The primary runtime path is Rust. There is no Python sidecar or local FastAPI
service. Browser direct mode requires provider CORS support, and browser-visible
API keys are only appropriate for personal/local workflows.

Provider implementations declare their maximum batch and live capabilities.
Named profiles can narrow those capabilities for OpenAI-compatible services
without new Rust code. Apple live is streaming and can emit
partial/final/translation events. Doubao live is chunked microphone
transcription with chunk status and finalized text only; use `--capabilities` to
inspect the resolved provider and `--doctor` for full environment diagnostics.

## License

[MIT License](LICENSE)

This project is MIT licensed.
