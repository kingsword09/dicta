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
