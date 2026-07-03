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

For a step-by-step profile setup guide, see
[openai-compatible-provider.md](openai-compatible-provider.md).

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
$ dicta --input audio.wav --provider apple --native-adapter ./dicta-adapter-apple-speech
$ dicta --input audio.wav --provider openai
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
$ dicta provider set <provider-name>
$ dicta provider current
$ dicta --provider active --live
```

`dicta provider set <name>` writes `~/.config/dicta/active-provider.json` by default.
`--provider active` resolves that file to a concrete built-in, configured, or
installed provider profile. When the state file is missing, `active` defaults to
the built-in Apple provider only on supported macOS systems. On other systems,
install a live provider package and select it with `dicta provider set <name>`.
The state file stores only the selected provider name; secrets remain in CLI
flags, environment variables, or `providers.toml`.

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
$ dicta provider available
$ dicta provider install <provider-name>
$ dicta provider install ./my-provider
$ dicta provider install ./my-provider-0.1.0.tgz --force
```

The installer treats npm as a tarball registry only. It queries metadata,
downloads the package tarball, verifies npm integrity when present, and unpacks
the package into Dicta's provider directory. It does not run `npm install`, does
not create `node_modules`, and does not require Node at runtime.

An installed provider package must include `provider.toml`:

```toml
id = "example-live-asr"
name = "Example Live ASR"
version = "0.1.0"
protocol = "dicta-provider-jsonl-v1"
command = "bin/dicta-provider-example"
model = "example-live-asr"

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
External live providers declare their own streaming or chunked event shape in
`provider.toml`.
