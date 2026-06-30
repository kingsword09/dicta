# ASR providers

`vo` uses Rust provider crates instead of a Python sidecar.

## OpenAI-compatible

The current provider crate is `vo-asr-openai-compatible`. It sends multipart
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

## Doubao

`--asr doubao` uses `vo-asr-doubao`. That crate implements the unofficial
Doubao IME ASR protocol directly in Rust: it auto-registers a virtual Android
device, caches credentials, obtains the ASR app key, encodes WAV input as Opus
frames, and sends protobuf messages over WebSocket.

```console
$ vo --input audio.wav \
    --asr doubao \
    --src zh-CN
```

This deliberately replaces the old `doubao_asr_api.py` runtime. No Python,
FastAPI, or `doubaoime_asr` dependency is required by the Rust CLI.

`--asr auto` also resolves to `doubao` on systems where Apple on-device ASR is
not available. The provider does not require `--api-base` or `--api-key`.

By default credentials are cached at `~/.config/vo/doubao-credentials.json`.
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
$ vo --asr doubao --src zh-CN --live-chunk 3
```

## Native adapter / Apple Speech

`--asr apple` uses `vo-asr-native-adapter`. The provider is intentionally a
bridge: it runs a platform-native adapter binary. The current implementation is
`adapters/apple-speech`, a macOS 26 Swift adapter. Batch mode parses finalized
transcript JSONL; live mode reads typed adapter events and lets the Rust CLI own
rendering, transcript logging, and exit prompts.

```console
$ vo --input audio.wav \
    --asr apple \
    --native-adapter adapters/apple-speech/.build/release/vo \
    --src en-US
```

The Rust CLI can also host the adapter's live mic/speaker mode:

```console
$ vo --live \
    --asr apple \
    --native-adapter adapters/apple-speech/.build/release/vo \
    --src en-US
```

The adapter is only available when the current OS supports Apple on-device ASR.
Live capture remains implemented inside `adapters/apple-speech`; the Rust CLI
owns the top-level command surface and delegates Apple-only system capture/events
to that adapter. `--apple-adapter` and `VO_APPLE_ADAPTER` are still accepted as
compatibility aliases for older scripts.

## Provider capabilities

Every ASR provider exposes a single provider-level capability declaration through
`vo_asr::ProviderCapabilities`. This is the source of truth for CLI validation,
`vo --capabilities`, and the capability section in `vo --doctor`.

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
$ vo --input audio.wav --provider openai
$ vo --capabilities --provider openai --json
```

Custom profiles are loaded from `~/.config/vo/providers.toml`, or from an
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
$ vo --input audio.wav --provider siliconflow
$ vo --capabilities --provider siliconflow --json
```

Effective capabilities are the provider implementation's maximum capabilities
intersected with the profile declaration. A profile cannot enable functionality
that the implementation does not support; for example, an `openai-compatible`
profile that sets `live_enabled = true` reports a local configuration error.
For API keys, `--api-key` / `VO_ASR_API_KEY` wins over profile settings; profile
`api_key` wins over `api_key_env`.

Capability diagnostics are local and explicit. `vo --capabilities` resolves the
selected backend, checks local configuration requirements such as a native
adapter path, and prints the declared capability flags. It does not contact
remote ASR services by default because OpenAI-compatible providers do not share a
standard capability-discovery API.

```console
$ vo --capabilities
$ vo --capabilities --asr doubao --json
$ vo --capabilities --asr apple --native-adapter ./vo-adapter-apple-speech
$ vo --capabilities --provider siliconflow --provider-config ./providers.toml
```

## Live provider events

Live providers use the shared `vo-core::LiveEvent` stream:

```text
Meta -> Status? -> Volatile? -> Finalized -> Translated? -> Eof
```

Apple live is a streaming provider: it supports mic/speaker capture, partial
results, finalized results, translation, voice processing, and device selection.
Doubao live is a chunked provider: it supports microphone capture and finalized
results only, with status events for the current chunk phase.
