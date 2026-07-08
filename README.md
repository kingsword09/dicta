# dicta

`dicta` is a Rust-first transcription toolkit for live and file-based speech
transcription. The default release keeps only portable, redistributable pieces
in the main CLI and lets provider packages supply private or platform-specific
ASR runtimes when needed.

Built-in providers cover Apple on-device speech on supported macOS systems and
OpenAI-compatible batch APIs. Provider packages cover services that need custom
protocols, private libraries, or platform-specific binaries.

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

## Providers

List built-in, configured, and installed providers:

```console
$ dicta provider list
$ dicta provider current
$ dicta --capabilities --provider active
```

Discover and install external provider packages:

```console
$ dicta provider available
$ dicta provider install <provider-name>
$ dicta provider update
$ dicta provider update <provider-name>
$ dicta provider remove <provider-name> --yes
```

Provider packages are downloaded directly from the npm registry as tarballs.
Bare names resolve to the default `@dicta-asr` scope. `dicta` reads npm
metadata, selects a matching platform artifact when a provider uses
`optionalDependencies`, verifies npm integrity when present, and unpacks the
provider into Dicta's provider directory. It does not run `npm install`, does not
create `node_modules`, and does not require Node at runtime.

Local provider directories and `.tgz` files are also supported:

```console
$ dicta provider install ./my-provider
$ dicta provider install ./my-provider-0.1.0.tgz --force
```

See [docs/providers.md](docs/providers.md) for provider manifests, process
protocol details, active-provider state, and custom OpenAI-compatible profiles.

## Usage

Realtime transcription:

```console
$ dicta --ptt                            # Push-to-talk using the active provider
$ dicta                                  # Continuous live mode using the active live provider
$ dicta --src en-US --dst ja-JP          # Continuous transcribe and translate
$ dicta --no-speaker                     # Mic-only continuous mode
$ dicta --no-mic --src en-US --dst ja-JP # Speaker-only continuous mode on Apple
$ dicta --select-device                  # Pick and pin mic / speaker at startup
$ dicta --json | jq                      # JSONL output for piping
$ dicta --capabilities                   # ASR capability diagnostics
$ dicta doctor                           # Environment diagnostics
```

Push-to-talk mode is the recommended foreground dictation path for installed IME
providers such as Qianwen and Doubao. Press Enter once to start an utterance and
again to stop/finalize it; press Ctrl-C to quit. When no provider is passed,
`dicta --ptt` resolves `--provider active`. Continuous live mode remains
available for captioning, Apple translation, and providers that expose a stable
long-running stream.

Status bar mode:

```console
$ dicta --ui                             # Open the status bar provider switcher
$ dicta --ui --ptt                       # Open the switcher and start PTT
$ dicta --ui --live                      # Open the switcher and start continuous live mode
$ dicta --ui --hotkey ctrl+alt+space     # Enable a global tray hotkey
$ dicta --ui --provider active --live    # Use the saved active provider
```

The status bar UI is the Rust `dicta-tray` companion binary. Plain `dicta --ui`
launches it, opens the provider panel, then returns control to the shell; quit
from the status item menu when you want to stop the tray. `dicta --ptt --ui` and
`dicta --live --ui` are foreground realtime sessions: the terminal keeps the
normal PTT or live behavior while the tray controls that same session through a
small supervisor channel. Left-clicking the status item reopens the compact
provider panel; right-clicking keeps a native menu fallback. The panel lists
built-in and configured provider profiles. In plain `dicta --ui`, it can start a
tray-managed `dicta --provider active ...` worker. In `--ptt --ui` or
`--live --ui`, provider switching restarts the foreground session worker instead
of launching a second independent worker. The plain `dicta --ui` panel stays
idle until you start recording. Its default activation is automatic: PTT-capable
providers use `--ptt`, and other live providers use `--live`. Pass `--ui --live`
to force continuous live mode or `--ui --ptt` to force PTT.
Global hotkeys are disabled unless `--hotkey` or
`DICTA_UI_HOTKEY` is set. The hotkey syntax is the same as
`ctrl+alt+space`, `shift+alt+KeyD`, or `cmd+shift+KeyD`; use `off` to disable
an inherited environment value. With PTT providers the hotkey is hold-to-talk;
with continuous live providers it toggles the worker. Without a saved active
provider, `active` defaults to Apple only on supported macOS systems. On other
systems, install a live provider package and select it with
`dicta provider set <provider-name>`. Switching providers from the panel stores
the active selection in `~/.config/dicta/active-provider.json` and restarts the
worker, so a failed provider can be replaced without leaving the status bar UI.

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
See [docs/openai-compatible-provider.md](docs/openai-compatible-provider.md) for
a step-by-step provider profile setup guide.

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
$ dicta doctor
$ dicta --json doctor
$ dicta --capabilities
$ dicta --capabilities --provider openai --json
$ dicta provider list
$ dicta provider current
$ dicta provider set openai
```

`--input` and `--mic-duration` are mutually exclusive. Without either flag, `dicta`
enters live mode. Environment variables mirror the provider flags; run
`dicta --help`, `dicta --capabilities`, and `dicta doctor` for local details.

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
  dicta-asr/                     provider traits and capability contracts
  dicta-asr-*/                   provider implementation crates
  dicta-asr-native-adapter/      JSONL process bridge for native adapters
  dicta-audio/                   microphone recording
  dicta-cli/                     command-line entry point
  dicta-tray/                    Rust status bar provider switcher
  dicta-web/                     browser WASM provider/audio/storage APIs
web/direct/                      static browser direct-provider tool
adapters/apple-speech/           macOS 26 Apple Speech adapter
```

The primary runtime path is Rust. There is no Python sidecar or local FastAPI
service. Browser direct mode requires provider CORS support, and browser-visible
API keys are only appropriate for personal/local workflows.

Provider implementations declare their maximum batch and live capabilities.
Named profiles can narrow those capabilities for OpenAI-compatible services
without new Rust code. Installed provider packages run as separate processes
through the same JSONL provider protocol used by the CLI. External providers can
also declare `live.ptt = true` and implement the `--ptt-json` control protocol.
Apple live is streaming and can emit partial/final/translation events; use
`--capabilities` to inspect the resolved provider and `dicta doctor` for full
environment diagnostics.

## License

[MIT License](LICENSE)

This project is MIT licensed.

## Acknowledgements

`dicta` started as `vo` and keeps the same focused spirit: a small transcription
CLI that stays close to the operating system and avoids unnecessary runtime
services. It also takes inspiration from [k1LoW/vo](https://github.com/k1LoW/vo),
especially its focused macOS on-device transcription CLI experience.
