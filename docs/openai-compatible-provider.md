# OpenAI-compatible provider profiles

Use an OpenAI-compatible provider profile when an ASR service accepts multipart
requests at `/v1/audio/transcriptions` and returns a JSON body containing
`text`.

This path does not require a new Rust crate or installable provider package.
`dicta` reuses the built-in `openai-compatible` implementation and reads service
configuration from `providers.toml`.

## API contract

The provider must accept:

```text
POST {api_base}/v1/audio/transcriptions
```

`api_base` can be the service root or a URL that already ends in `/v1`; `dicta`
normalizes the transcription endpoint.

`dicta` sends multipart form fields:

```text
file=<audio bytes>
model=<model id>
response_format=json
language=<optional source language hint>
prompt=<optional provider prompt>
```

The response must include:

```json
{"text":"transcript text"}
```

An optional `language` field is accepted when present.

## Create a profile

Create or edit `~/.config/dicta/providers.toml`:

```toml
[providers.siliconflow]
kind = "openai-compatible"
api_base = "https://api.siliconflow.cn"
default_model = "FunAudioLLM/SenseVoiceSmall"
api_key_env = "SILICONFLOW_API_KEY"
batch_file = true
streaming = false
requires_network = true
live_enabled = false
notes = ["SiliconFlow OpenAI-compatible ASR profile."]
```

Use `api_key_env` for normal use so secrets stay outside the config file:

```console
$ export SILICONFLOW_API_KEY="sk-..."
$ dicta --input meeting.wav --provider siliconflow --src zh-CN
```

For local private configs only, `api_key` is also supported:

```toml
[providers.local-asr]
kind = "openai-compatible"
api_base = "http://127.0.0.1:8000"
default_model = "whisper-1"
api_key = "local-dev-key"
live_enabled = false
```

## Verify the profile

Check local configuration and declared capabilities:

```console
$ dicta --capabilities --provider siliconflow
$ dicta --capabilities --provider siliconflow --json
```

Run a batch transcription:

```console
$ dicta --input meeting.wav --provider siliconflow --src zh-CN
```

Expose the configured provider as a local OpenAI-compatible API:

```console
$ dicta --provider siliconflow serve --host 127.0.0.1 --port 4777
```

Then call it from a local project:

```console
$ curl -s \
    -F file=@meeting.wav \
    -F model=dicta \
    -F language=zh-CN \
    http://127.0.0.1:4777/v1/audio/transcriptions
```

Use `model=dicta` or `model=default` to keep the model configured in the
profile. Any other non-empty model acts as a per-request model override where
the upstream service supports it.

## When to build a provider package

Do not build a provider package for a service that already speaks the
OpenAI-compatible transcription API. A profile is simpler and avoids extra
runtime processes.

Build an installable provider package only when the provider needs a custom
protocol, native libraries, device registration, live event streaming, or other
runtime behavior that the OpenAI-compatible client cannot express.
