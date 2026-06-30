# vo

`vo` is a Rust-first transcription toolkit inspired by
[k1LoW/vo](https://github.com/k1LoW/vo). It keeps the original macOS live command
shape while adding Doubao, OpenAI-compatible providers, and a browser WASM path.

## Install

```console
$ curl -fsSL https://raw.githubusercontent.com/kingsword09/vo/main/install.sh | sh
$ curl -fsSL https://raw.githubusercontent.com/kingsword09/vo/main/install.sh \
    | VO_VERSION=0.10.0 VO_INSTALL_DIR="$HOME/bin" sh
```

The installer downloads the matching GitHub Release archive and installs `vo`
into `~/.local/bin` by default. Release assets cover macOS arm64, Linux
x86_64/arm64, and Windows x86_64/arm64. macOS arm64 archives also include the
Apple Speech adapter.

## Usage

Live mode:

```console
$ vo                                  # macOS 26: Apple on-device; otherwise Doubao
$ vo --src en-US --dst ja-JP          # Transcribe and translate
$ vo --no-speaker                     # Mic only
$ vo --no-mic --src en-US --dst ja-JP # Speaker only on Apple mode
$ vo --select-device                  # Pick and pin mic / speaker at startup
$ vo --json | jq                      # JSONL output for piping
$ vo --doctor                         # Environment diagnostics
```

Doubao does not require an API key:

```console
$ vo --asr doubao --src zh-CN
$ vo --asr doubao --json
$ vo --asr doubao --src zh-CN --live-chunk 3
$ vo --input meeting.wav --asr doubao --src zh-CN
$ vo --mic-duration 5 \
    --asr doubao \
    --src zh-CN
```

OpenAI-compatible transcription:

```console
$ vo --input meeting.wav \
    --asr openai-compatible \
    --api-base https://api.openai.com \
    --api-key "$VO_ASR_API_KEY" \
    --api-model whisper-1
$ vo --mic-duration 5 \
    --asr openai-compatible \
    --api-base https://api.openai.com \
    --api-key "$VO_ASR_API_KEY" \
    --api-model whisper-1
```

Apple on-device mode requires macOS 26 and an Apple Speech adapter:

```console
$ vo --live \
    --asr apple \
    --native-adapter adapters/apple-speech/.build/release/vo \
    --src en-US
$ vo --input meeting.wav \
    --asr apple \
    --native-adapter adapters/apple-speech/.build/release/vo \
    --src en-US
```

Output:

```console
$ vo --input meeting.wav --json
$ vo --input meeting.wav --transcript meeting.txt
$ vo --input meeting.wav --json --transcript meeting.jsonl
$ vo --doctor
$ vo --doctor --json
```

`--input` and `--mic-duration` are mutually exclusive. Without either flag, `vo`
enters live mode. Environment variables mirror the provider flags; run
`vo --help` and `vo --doctor` for local details.

## Web

For browser integration without a backend:

```console
$ python3 -m http.server 8765
$ open http://127.0.0.1:8765/web/direct/
```

`web/direct` provides a dependency-free browser transcription module plus a
static integration demo for Web Components, plain JavaScript, and React voice UI
components such as AI Elements `SpeechInput`. `crates/vo-web` is the browser
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
  vo-core/                    shared transcript schema
  vo-asr*/                    ASR provider crates
  vo-audio/                   microphone recording
  vo-cli/                     command-line entry point
  vo-web/                     browser WASM provider/audio/storage APIs
web/direct/                   static browser direct-provider tool
adapters/apple-speech/        macOS 26 Apple Speech adapter
```

The primary runtime path is Rust. There is no Python sidecar or local FastAPI
service. Browser direct mode requires provider CORS support, and browser-visible
API keys are only appropriate for personal/local workflows.

Live providers declare their capabilities. Apple live is streaming and can emit
partial/final/translation events. Doubao live is chunked microphone transcription
with chunk status and finalized text only; use `--doctor` to inspect the active
backend.

## License

[MIT License](LICENSE)

This project keeps the MIT license. Thanks to
[k1LoW/vo](https://github.com/k1LoW/vo) for the original macOS on-device
transcription and translation CLI design.
