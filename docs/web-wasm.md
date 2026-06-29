# Web WASM boundary

`crates/vo-web` is the browser-only WASM crate. It is separate from the Rust CLI
and native provider crates so browser builds do not pull in `tokio`, `reqwest`,
`cpal`, platform adapters, or the Doubao native WebSocket stack.

The crate shares `vo-core` transcript schema. A successful transcription returns
this shape:

```json
{
  "provider": "openai-compatible",
  "source": {
    "lang": "zh-CN",
    "text": "..."
  },
  "raw": {
    "text": "..."
  }
}
```

## Provider

The first web provider is OpenAI-compatible HTTP transcription:

```text
POST {apiBase}/v1/audio/transcriptions
```

Exported functions:

```js
transcription_url(apiBase)
await transcribe_file(config, file)
```

Example config:

```js
const config = {
  provider: "openai-compatible",
  apiBase: "https://api.example.com",
  apiKey: "optional",
  model: "whisper-1",
  language: "zh-CN",
  prompt: "",
};
```

`apiKey`, `language`, and `prompt` are optional. If `apiBase` already ends in
`/v1`, only `/audio/transcriptions` is appended.

Doubao can be used in web mode when a CORS-enabled HTTP endpoint accepts the
same OpenAI-compatible multipart contract. The native Doubao IME protocol is not
available in browser direct mode because the current unofficial WebSocket
handshake depends on headers that browser WebSocket APIs cannot set.

## Audio

Exported functions:

```js
recommended_recording_mime_type()
recommended_recording_extension(mimeType)
await record_microphone(seconds)
```

`record_microphone` uses `navigator.mediaDevices.getUserMedia` and
`MediaRecorder`, then returns a browser `File` that can be passed directly to
`transcribe_file`.

Browser microphone capture requires a secure context. Local files and
`http://localhost` are typically accepted by desktop browsers.

## Storage

Exported functions:

```js
save_provider_config(config)
load_provider_config()
delete_provider_config()
```

The initial storage backend is `localStorage` for small provider config. It is a
web-only implementation and should be treated as browser-visible storage; do not
use it for a public hosted app where provider keys must remain private.

IndexedDB can be added later for larger artifacts such as cached audio blobs or
session history without changing `vo-core`.
