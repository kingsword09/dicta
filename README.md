# vo

`vo` is a Rust-first transcription toolkit derived from and inspired by
[k1LoW/vo](https://github.com/k1LoW/vo). The Rust CLI keeps the original
macOS live command shape while adding pluggable ASR providers such as Doubao and
OpenAI-compatible endpoints.

## Current status

- Rust workspace is the primary build target.
- The Rust CLI supports on-disk audio transcription through an OpenAI-compatible
  ASR provider.
- The Rust CLI can record the default microphone for a fixed duration and submit
  the captured WAV to the same provider path.
- The Rust CLI has `--doctor` diagnostics for backend resolution, API config,
  default input audio, and runtime mode.
- `--asr doubao` uses the Rust `vo-asr-doubao` provider. It implements the
  Doubao IME protocol directly, auto-registers device credentials, and does not
  use the old Python sidecar.
- A no-backend browser tool exists at `web/direct/index.html` for local direct
  provider calls with file upload or browser microphone recording.
- On macOS 26, `vo` without `--input` keeps the original live mic/speaker command
  shape. The bundled Apple adapter produces JSONL events and the Rust CLI owns
  rendering, JSONL, and transcript output.
- On lower macOS versions and other platforms, `vo` without `--input` resolves
  to Doubao live mic transcription unless another backend is selected.

## Architecture

```text
crates/
  vo-core/                    shared transcript schema and audio input types
  vo-asr/                     ASR provider trait and common option/result types
  vo-asr-openai-compatible/   direct /v1/audio/transcriptions provider
  vo-asr-doubao/              Doubao provider entry point, no Python runtime
  vo-asr-native-adapter/      bridge to platform-native adapter binaries
  vo-audio/                   cross-platform default microphone recording
  vo-cli/                     Rust command-line entry point
  web/direct/                 static browser direct-provider tool

adapters/
  apple-speech/               macOS 26 Apple Speech native adapter
```

The Rust CLI calls remote providers directly. There is no Python runtime and no
local FastAPI service in the primary path.

## Install

Install the latest release:

```console
$ curl -fsSL https://raw.githubusercontent.com/kingsword09/vo/main/install.sh | sh
```

Install a specific version or directory:

```console
$ curl -fsSL https://raw.githubusercontent.com/kingsword09/vo/main/install.sh \
    | VO_VERSION=0.10.0 VO_INSTALL_DIR="$HOME/bin" sh
```

The installer downloads the matching GitHub Release archive for the current
platform and installs `vo` into `~/.local/bin` by default. macOS arm64 release
archives also include `vo-adapter-apple-speech` next to `vo` so macOS 26
device-model capture works through the Rust CLI without extra flags. Prebuilt
assets are published for macOS arm64, Linux x86_64/arm64, and Windows
x86_64/arm64.

Validate the installer locally before publishing a release:

```console
$ ./scripts/build.sh
$ tmp_release="$(mktemp -d)"
$ stage="$(mktemp -d)"
$ tmp_install="$(mktemp -d)"
$ cp target/release/vo "$stage/vo"
$ cp LICENSE README.md "$stage/"
$ tar -C "$stage" -czf "$tmp_release/vo_darwin_arm64.tar.gz" .
$ VO_ARCHIVE="$tmp_release/vo_darwin_arm64.tar.gz" \
    VO_INSTALL_DIR="$tmp_install/bin" \
    ./install.sh
$ "$tmp_install/bin/vo" --version
$ "$tmp_install/bin/vo" --doctor --json
$ VO_INSTALL_DIR="$tmp_install/bin" ./install.sh --uninstall
```

## Build and test

```console
$ ./scripts/build.sh
$ ./scripts/test.sh
```

The resulting binary is:

```console
$ ./target/release/vo --help
$ ./target/release/vo --doctor
```

The isolated Apple on-device adapter is still available when you need the macOS
26 device model path:

```console
$ ./scripts/build-apple-speech-adapter.sh
$ ./scripts/test-apple-speech-adapter.sh
```

## Usage

On macOS 26, the original live interaction is preserved:

```console
$ vo                                  # Listen to mic + speaker, transcribe only
$ vo --src en-US --dst ja-JP          # Transcribe and translate
$ vo --no-speaker                     # Mic only
$ vo --no-mic --src en-US --dst ja-JP # Speaker only, with translation
$ vo --select-device                  # Pick and pin mic / speaker at startup
$ vo --json | jq                      # JSONL output for piping
$ vo --doctor                         # Environment diagnostics
```

This path is implemented by the Rust CLI launching the Apple Speech native
adapter bundled as `vo-adapter-apple-speech`, reading typed live events from it,
then rendering and logging the session itself. You can point to a custom adapter
with `--native-adapter` or `VO_NATIVE_ADAPTER`. The older `--apple-adapter` /
`VO_APPLE_ADAPTER` names remain compatibility aliases.

Use Doubao live ASR on systems without Apple on-device support, or explicitly
with `--asr doubao`:

```console
$ vo --asr doubao --src zh-CN
$ vo --asr doubao --json
$ vo --asr doubao --transcript meeting.jsonl
```

Doubao live mode records the default microphone in short chunks and sends them
directly to Doubao IME ASR. It does not require an API key. It does not provide
Apple-only features such as system speaker capture, on-device translation,
voice processing, or interactive device selection.

Transcribe a file through an OpenAI-compatible ASR endpoint:

```console
$ vo --input meeting.wav \
    --asr openai-compatible \
    --api-base https://api.openai.com \
    --api-key "$VO_ASR_API_KEY" \
    --api-model whisper-1
```

Record the default microphone for five seconds, then transcribe the captured WAV:

```console
$ vo --mic-duration 5 \
    --asr openai-compatible \
    --api-base https://api.openai.com \
    --api-key "$VO_ASR_API_KEY" \
    --api-model whisper-1
```

Use Doubao IME ASR. No API key is required; the first run auto-registers a
virtual device and caches credentials under `~/.config/vo`:

```console
$ vo --input meeting.wav \
    --asr doubao \
    --src zh-CN
```

Record the default microphone for five seconds and send the captured WAV to
Doubao:

```console
$ vo --mic-duration 5 \
    --asr doubao \
    --src zh-CN
```

Use the macOS 26 on-device Apple path through an Apple Speech adapter binary:

```console
$ vo --input meeting.wav \
    --asr apple \
    --native-adapter adapters/apple-speech/.build/release/vo \
    --src en-US
```

Run live macOS 26 on-device mic/speaker transcription explicitly:

```console
$ vo --live \
    --asr apple \
    --native-adapter adapters/apple-speech/.build/release/vo \
    --src en-US
```

Live Apple mode also supports the adapter's capture controls:

```console
$ vo --live \
    --asr apple \
    --native-adapter adapters/apple-speech/.build/release/vo \
    --no-speaker \
    --json \
    --transcript meeting.jsonl
```

Emit JSONL:

```console
$ vo --input meeting.wav --json
```

Write the finalized transcript to a file:

```console
$ vo --input meeting.wav --transcript meeting.txt
$ vo --input meeting.wav --json --transcript meeting.jsonl
```

Check the local environment:

```console
$ vo --doctor
$ vo --doctor --json
```

`--input` and `--mic-duration` are mutually exclusive. Microphone mode records a
temporary WAV file with the system default input device, sends it to the selected
provider, then removes the temporary file.

Without `--input` or `--mic-duration`, `vo` enters live mode. `--asr auto`
selects Apple live on macOS 26 and Doubao live where Apple on-device ASR is not
available. Batch-only providers such as `openai-compatible` require `--input` or
`--mic-duration`.

`--transcript` writes the single finalized result after the provider returns.
With `--json`, it writes one transcript JSONL line plus a trailing newline. Without
`--json`, it writes plain text plus a trailing newline. In live mode, the Rust
renderer writes the same finalized transcript JSONL schema for Apple and Doubao.
Internal volatile/meta events are used only to drive the TTY view. With
`--transcript`, it writes directly to the requested path; in interactive TTY mode
without `--transcript`, it keeps a temporary JSONL session log and prompts at
exit to save or discard it.

Environment variables mirror the provider flags:

```text
VO_ASR_BACKEND=auto|openai-compatible|doubao|apple
VO_ASR_API_BASE=https://api.example.com
VO_ASR_API_KEY=...
VO_ASR_API_MODEL=...
VO_DOUBAO_CREDENTIAL_PATH=~/.config/vo/doubao-credentials.json
VO_DOUBAO_DEVICE_ID=...
VO_DOUBAO_TOKEN=...
VO_NATIVE_ADAPTER=adapters/apple-speech/.build/release/vo
VO_SRC=en-US
VO_DST=ja-JP
```

`--asr auto` is platform-aware. In live mode it selects Apple live on macOS 26+
and Doubao live where Apple on-device ASR is unavailable. In batch mode it
resolves to Doubao on systems without Apple support, but keeps the generic
`openai-compatible` HTTP path on macOS 26+ unless you explicitly choose
`--asr apple --native-adapter ...`. Supplying `--api-model doubaoime-asr` or the
compatibility alias `doubao-asr` also resolves to `doubao`; any other explicit model
resolves to `openai-compatible`.

`--asr apple` is available only when the current OS supports Apple on-device ASR
and `--native-adapter` / `VO_NATIVE_ADAPTER` points to a built native adapter
binary such as `adapters/apple-speech`. On systems below macOS 26, `--asr apple`
reports that Apple on-device mode is unavailable and remote/provider mode should
be used.

`--doctor` does not call any ASR provider. It reports how the backend would be
resolved, whether API base/key are configured, default microphone availability,
whether Apple on-device mode is supported on the current OS, and confirms that
the Rust path does not require a Python sidecar.

## Provider model

The first Rust provider is `vo-asr-openai-compatible`, which posts multipart
audio to:

```text
POST {api_base}/v1/audio/transcriptions
```

If `api_base` already ends in `/v1`, the provider appends only
`/audio/transcriptions`. The expected response is a JSON object with at least:

```json
{"text":"..."}
```

`doubao_asr_api.py` is not part of the new runtime path. `vo-asr-doubao`
implements the Doubao IME protocol in Rust, including device registration,
credential caching, Opus frame encoding, and protobuf WebSocket requests. The
protocol is unofficial and may change upstream.

`vo-asr-native-adapter` runs platform-native binaries as external adapters. The
current implementation is `adapters/apple-speech`, a Swift adapter for macOS 26.
In live mode the adapter is headless from the user's perspective: it emits typed
live events and the Rust CLI owns the command-line interaction.

## Web direct mode

For local browser use without a backend:

```console
$ open web/direct/index.html
```

The page lets you choose an audio file or record from the browser microphone,
then enter API Base, API Key, Model, language hint, and prompt. It sends the
request directly from the browser to the provider. This only works when the
provider allows CORS, accepts the browser's recorded audio format, and the API
key being visible to the browser process is acceptable. Use it for personal/local
workflows, not public hosted deployments.

See [docs/web-direct.md](docs/web-direct.md).

## Design notes

- KISS: the CLI directly calls provider crates instead of routing through a local
  HTTP sidecar.
- YAGNI: Web API and WASM crates are not created until the browser-facing product
  needs them.
- DRY: CLI, future web server, and future desktop shells should reuse `vo-core`
  and `vo-asr`.
- SOLID: ASR providers are isolated behind `AsrProvider`; audio capture, rendering,
  and model protocols can evolve independently.

## Native Adapters

The Swift code in `adapters/apple-speech` is the macOS 26+ Apple Silicon native
adapter. It performs on-device live transcription with Apple's
`SpeechTranscriber`, optional on-device translation with `TranslationSession`,
and macOS-specific mic/speaker capture, then emits typed live events for the Rust
CLI.

Use `--asr apple --native-adapter <path>` from the Rust CLI for batch file
transcription. Add `--live` when you want the Rust CLI to launch the adapter for
live mic/speaker capture while keeping rendering in Rust.

## License

[MIT License](LICENSE)

This project keeps the MIT license. Thanks to
[k1LoW/vo](https://github.com/k1LoW/vo) for the original macOS on-device
transcription and translation CLI design.
