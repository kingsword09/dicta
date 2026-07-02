# ASR providers

`dicta` uses Rust provider crates instead of a Python sidecar.

## OpenAI-compatible

The current provider crate is `dicta-asr-openai-compatible`. It sends multipart
audio to:

```text
POST {api_base}/v1/audio/transcriptions
```

Required request fields:

```text
file=<audio bytes>
model=<model id>
response_format=json
```

Optional request fields:

```text
language=<BCP-47 or provider-specific language id>
prompt=<provider prompt>
```

Expected response:

```json
{"text":"transcript text"}
```

`language` in the response is accepted when present.

## Local OpenAI-compatible server

`dicta serve` exposes the currently selected Rust provider as an OpenAI-compatible
batch ASR HTTP API:

```console
$ dicta --provider active serve --cors-origin http://localhost:5173
$ curl -s \
    -F file=@audio.wav \
    -F model=dicta \
    -F language=zh-CN \
    http://127.0.0.1:4777/v1/audio/transcriptions
```

Endpoints:

```text
GET  /health
GET  /v1/models
POST /v1/audio/transcriptions
```

The transcription endpoint accepts `multipart/form-data`. `file` is required.
`model`, `language`, `prompt`, `response_format`, and `temperature` are accepted
for OpenAI-compatible clients. `model=dicta` and `model=default` use the server's
configured provider model; any other non-empty `model` acts as a per-request
`--api-model` override where that is meaningful. The provider selected at server
startup remains authoritative. `response_format=json` returns
`{"text":"..."}` plus `language` when known, and `response_format=text` returns
plain text.

`dicta serve` is batch-only. It rejects `stream=true`, timestamp granularities,
`verbose_json`, SRT, and VTT until provider capabilities expose those results
directly. The default bind address is `127.0.0.1:4777`; pass `--cors-origin`
for browser projects and keep secrets in CLI flags, environment variables, or
provider profiles on the server side.

## Doubao

`--asr doubao` uses `dicta-asr-doubao`. That crate implements the unofficial
Doubao IME ASR protocol directly in Rust: it auto-registers a virtual Android
device, caches credentials, obtains the ASR app key, encodes WAV input as Opus
frames, and sends protobuf messages over WebSocket.

```console
$ dicta --input audio.wav \
    --asr doubao \
    --src zh-CN
```

This deliberately replaces the old `doubao_asr_api.py` runtime. No Python,
FastAPI, or `doubaoime_asr` dependency is required by the Rust CLI.

`--asr auto` also resolves to `doubao` on systems where Apple on-device ASR is
not available. The provider does not require `--api-base` or `--api-key`.

By default credentials are cached at `~/.config/dicta/doubao-credentials.json`.
Override that path with `--doubao-credential-path` or provide existing
credentials with `--doubao-device-id` and `--doubao-token`.

The protocol is not official and may break if Doubao changes its input method
service. The current Rust provider accepts 16-bit PCM WAV input and resamples it
to 16 kHz mono before Opus encoding. `--mic-duration` works because the Rust
audio path records WAV.

Doubao live mode is a chunked live provider: the CLI records short microphone
chunks, sends each chunk to Doubao, and renders finalized text when a response is
available. While a chunk is active, the live event stream reports status updates
such as recording, transcribing, and recoverable chunk failures. It does not
expose partial results, speaker capture, or translation. Tune chunk size with
`--live-chunk`, for example:

```console
$ dicta --asr doubao --src zh-CN --live-chunk 3
```

## Qianwen Shell runtime

`--asr qianwen` uses `dicta-asr-qianwen`. It loads Qianwen's local
`libQianwenShellEmbedded.dylib` from a supplied Qianwen IME `qw` bundle and
drives the same embedded voice-input runtime used by the installed input method.

```console
$ dicta --asr qianwen \
    --live \
    --qianwen-host-bundle-path ../qw \
    --src zh-CN
$ DICTA_QIANWEN_HOST_BUNDLE_PATH=/Users/kingsword09/Documents/code/ai/qw \
    dicta --provider qianwen --live --src zh-CN
```

The primary Qianwen-specific runtime input is the local runtime location:

```console
$ dicta --asr qianwen \
    --live \
    --qianwen-runtime-path ../qw/Frameworks/qianwen_shell/libQianwenShellEmbedded.dylib
```

For installed bundles that do not expose Qianwen's WSG implementation in a
standard framework location, `dicta` writes a local WSG-compatible shim backed by
`libqianwen_unet_runtime.dylib` and points the embedded runtime at it. To
override that path explicitly:

```console
$ dicta --asr qianwen \
    --live \
    --qianwen-host-bundle-path ../qw \
    --qianwen-wsg-impl-path /path/to/libwsg_impl.dylib
```

The embedded runtime also honors an ASR query-sign override. `dicta` maps
`DICTA_QIANWEN_ASR_QUERY_SIGN` or `--qianwen-asr-query-sign` to Qianwen's
`QWEN_SHELL_ASR_QUERY_SIGN` environment variable before starting the runtime.
This is a narrow escape hatch for local debugging; the normal path is the
provider-managed WSG shim or an explicitly supplied WSG library.

By default, `dicta` looks for the embedded runtime under `../qw`, `../../qw`, and
common installed app bundle locations. Browser-downloaded app bundles can carry
macOS quarantine attributes that prevent dyld from loading local dylibs; clear
them from the supplied bundle if the runtime cannot be loaded:

```console
$ xattr -dr com.apple.quarantine ../qw
```

The current Qianwen provider is live-only because this local runtime exposes the
IME voice shortcut flow, not a file transcription API. Batch file transcription,
speaker capture, translation, timestamps, and OpenAI-compatible HTTP serving are
not advertised for this backend.

## Native adapter / Apple Speech

`--asr apple` uses `dicta-asr-native-adapter`. The provider is intentionally a
bridge: it runs a platform-native adapter binary. The current implementation is
`adapters/apple-speech`, a macOS 26 Swift adapter. Batch mode parses finalized
transcript JSONL; live mode reads typed adapter events and lets the Rust CLI own
rendering, transcript logging, and exit prompts.

```console
$ dicta --input audio.wav \
    --asr apple \
    --native-adapter adapters/apple-speech/.build/release/dicta-adapter-apple-speech \
    --src en-US
```

The Rust CLI can also host the adapter's live mic/speaker mode:

```console
$ dicta --live \
    --asr apple \
    --native-adapter adapters/apple-speech/.build/release/dicta-adapter-apple-speech \
    --src en-US
```

The adapter is only available when the current OS supports Apple on-device ASR.
Live capture remains implemented inside `adapters/apple-speech`; the Rust CLI
owns the top-level command surface and delegates Apple-only system capture/events
to that adapter through `--native-adapter` or `DICTA_NATIVE_ADAPTER`.

## Provider capabilities

Every ASR provider exposes a single provider-level capability declaration through
`dicta_asr::ProviderCapabilities`. This is the source of truth for CLI validation,
`dicta --capabilities`, and the capability section in `dicta --doctor`.

When adding a provider, keep the declaration in the provider crate and include:

- `batch`: file transcription support, batch streaming support, and whether the
  provider requires network access.
- `live`: optional live-mode support and flags for mic, speaker, streaming
  audio, partial results, finalized results, translation, voice processing,
  device selection, network access, and expected latency.
- `notes`: short operator-facing caveats, such as adapter requirements or
  non-standard remote capability discovery.

Only add Rust provider code when the protocol or runtime integration is new. For
another service that already speaks an existing protocol, add a provider profile
instead. Profiles describe a configured service instance while reusing the
implementation capability ceiling from its `kind`.

Built-in profiles can be selected directly:

```console
$ dicta --input audio.wav --provider doubao
$ dicta --input audio.wav --provider apple --native-adapter ./dicta-adapter-apple-speech
$ dicta --input audio.wav --provider openai
$ dicta --provider qianwen --live --qianwen-host-bundle-path ../qw
$ dicta --capabilities --provider openai --json
```

Custom profiles are loaded from `~/.config/dicta/providers.toml`, or from an
explicit `--provider-config` path:

```toml
[providers.siliconflow]
kind = "openai-compatible"
api_base = "https://api.siliconflow.cn"
default_model = "FunAudioLLM/SenseVoiceSmall"
api_key_env = "SILICONFLOW_API_KEY"
# api_key = "sk-..." # optional direct key for local/private configs
notes = ["SiliconFlow OpenAI-compatible profile."]
batch_file = true
streaming = false
requires_network = true
live_enabled = false
```

```console
$ dicta --input audio.wav --provider siliconflow
$ dicta --capabilities --provider siliconflow --json
```

Effective capabilities are the provider implementation's maximum capabilities
intersected with the profile declaration. A profile cannot enable functionality
that the implementation does not support; for example, an `openai-compatible`
profile that sets `live_enabled = true` reports a local configuration error.
For API keys, `--api-key` / `DICTA_ASR_API_KEY` wins over profile settings; profile
`api_key` wins over `api_key_env`.

## Active provider and status bar mode

Provider profiles can also be selected through a small state file. This is the
control surface used by `dicta --ui`:

```console
$ dicta provider list
$ dicta provider set doubao
$ dicta provider current
$ dicta --provider active --live
```

`dicta provider set <name>` writes `~/.config/dicta/active-provider.json` by default.
`--provider active` resolves that file to a concrete built-in or configured
profile. When the state file is missing, `active` defaults to the built-in Apple
provider on supported macOS systems and the built-in Doubao provider elsewhere,
so first launch does not require a manual selection. The state file stores only
the selected provider name; secrets remain in CLI flags, environment variables,
or `providers.toml`.

`dicta --ui` launches the Rust `dicta-tray` companion binary. The status item opens a
compact provider panel on left click and keeps a right-click native menu as a
fallback. The companion reads the same provider list and starts live
transcription as a supervised `dicta --provider active --live` worker. When a
provider is selected from the panel, the companion updates the active provider
and restarts the worker if it is running. This gives immediate recovery when the
current provider is unavailable without forcing every ASR provider
implementation to support an in-process hot-swap protocol.

Release archives install `dicta-tray` next to `dicta`. In a source checkout, `dicta --ui`
uses the sibling `target/debug/dicta-tray` binary when it exists, or falls back to
`cargo run -p dicta-tray`. The UI is Rust-owned and uses the operating system
WebView for the panel, so switching providers requires no npm, Node, or bundled
frontend build step.

Custom OpenAI-compatible profiles appear in the panel automatically after they
are added to `providers.toml`; no Rust code is needed unless the provider uses a
new protocol or runtime integration.

## Installable provider packages

Provider packages can live outside this repository and be installed with:

```console
$ dicta provider install ./doubaoime-asr-0.1.0.tgz
$ dicta provider install ./qianwenime-asr
$ dicta provider install @dicta-asr/qianwenime-asr --registry https://registry.npmjs.org
```

The installer treats npm as a tarball registry only. It queries metadata,
downloads the package tarball, verifies npm integrity when present, and unpacks
the package into Dicta's provider directory. It does not run `npm install`, does
not create `node_modules`, and does not require Node at runtime.

An installed provider package must include `provider.toml`:

```toml
id = "qianwenime-asr"
name = "Qianwen IME ASR"
version = "0.1.0"
protocol = "dicta-provider-jsonl-v1"
command = "bin/dicta-provider-qianwenime-asr"
model = "qianwenime-asr"

[batch]
file = false
streaming = false
requires_network = true

[live]
mode = "streaming"
mic = true
partial_results = true
finalized_results = true
requires_network = true
```

The command is run as a separate provider process. Batch mode receives
`--input`, `--json`, and optional `--src`. Live mode receives `--json`,
`--event-json`, source/target language flags, and capture flags. The provider
prints `dicta-core` compatible JSONL events.

Capability diagnostics are local and explicit. `dicta --capabilities` resolves the
selected backend, checks local configuration requirements such as a native
adapter path, and prints the declared capability flags. It does not contact
remote ASR services by default because OpenAI-compatible providers do not share a
standard capability-discovery API.

```console
$ dicta --capabilities
$ dicta --capabilities --asr doubao --json
$ dicta --capabilities --asr apple --native-adapter ./dicta-adapter-apple-speech
$ dicta --capabilities --provider siliconflow --provider-config ./providers.toml
```

## Live provider events

Live providers use the shared `dicta-core::LiveEvent` stream:

```text
Meta -> Status? -> Volatile? -> Finalized -> Translated? -> Eof
```

Apple live is a streaming provider: it supports mic/speaker capture, partial
results, finalized results, translation, voice processing, and device selection.
Doubao live is a chunked provider: it supports microphone capture and finalized
results only, with status events for the current chunk phase.
