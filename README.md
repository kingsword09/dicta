# vo

`vo` is a Rust-first transcription toolkit for live and file-based speech
transcription. It supports Doubao, OpenAI-compatible providers, Apple on-device
speech on supported macOS systems, and browser integration paths.

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

Update or remove an installed release:

```console
$ vo update
$ vo update --version 0.10.4
$ vo uninstall
$ vo uninstall --yes
```

`vo update` installs into the directory containing the running `vo` binary unless
`--install-dir` or `VO_INSTALL_DIR` is set. `vo uninstall` removes `vo`,
`vo-tray`, and bundled adapter binaries from that install directory. User
configuration under `~/.config/vo` is left in place.

## Usage

Live mode:

```console
$ vo                                  # macOS 26: Apple on-device; otherwise Doubao
$ vo --src en-US --dst ja-JP          # Transcribe and translate
$ vo --no-speaker                     # Mic only
$ vo --no-mic --src en-US --dst ja-JP # Speaker only on Apple mode
$ vo --select-device                  # Pick and pin mic / speaker at startup
$ vo --json | jq                      # JSONL output for piping
$ vo --capabilities                   # ASR capability diagnostics
$ vo --doctor                         # Environment diagnostics
```

Status bar mode:

```console
$ vo --ui                             # Open the status bar provider switcher
$ vo --ui --live                      # Open the switcher and start live mode
$ vo --ui --provider doubao --live    # Set the initial provider, then start live
```

The status bar UI is the Rust `vo-tray` companion binary. Left-clicking the
status item opens a compact provider panel; right-clicking keeps a native menu
fallback. It lists built-in and configured provider profiles and runs live
transcription as a supervised `vo --provider active --live` worker. Without a
saved active provider, `active` defaults to Apple on supported macOS systems and
Doubao elsewhere. Switching providers from the panel stores the active selection
in `~/.config/vo/active-provider.json` and restarts the worker, so a failed
provider can be replaced without leaving the status bar UI.

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
$ vo --input meeting.wav --provider openai
```

Named provider profiles let OpenAI-compatible services share the same Rust
implementation without adding a new backend. Built-in profiles can be selected
with `--provider`; custom profiles are loaded from `~/.config/vo/providers.toml`
or `--provider-config` and may use either `api_key_env` or direct `api_key`.

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
$ vo --capabilities
$ vo --capabilities --asr doubao --json
$ vo --capabilities --provider openai --json
$ vo provider list
$ vo provider current
$ vo provider set doubao
```

`--input` and `--mic-duration` are mutually exclusive. Without either flag, `vo`
enters live mode. Environment variables mirror the provider flags; run
`vo --help`, `vo --capabilities`, and `vo --doctor` for local details.

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
  vo-tray/                    Rust status bar provider switcher
  vo-web/                     browser WASM provider/audio/storage APIs
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

## Acknowledgements

`vo` takes inspiration from [k1LoW/vo](https://github.com/k1LoW/vo), especially
its focused macOS on-device transcription CLI experience.
