# Web direct mode

`web/direct/index.html` is a browser-only transcription tool. It does not run a
backend and does not require WASM. It can submit either a selected audio file or
a short browser microphone recording.

Open it directly:

```console
$ open web/direct/index.html
```

Or serve the repository with any static file server. This is only for loading the
HTML file; it is not a `vo` backend and it does not proxy or store API keys.

```console
$ static-server-command 8080
```

Then open:

```text
http://127.0.0.1:8080/web/direct/
```

## How it works

The page sends a browser `fetch` request with `FormData` to:

```text
POST {api_base}/v1/audio/transcriptions
```

Fields sent to the provider:

```text
file=<selected audio file>
model=<model id>
response_format=json
language=<optional language hint>
prompt=<optional prompt>
```

If an API key is provided, it is sent as:

```text
Authorization: Bearer <api key>
```

## Constraints

- The target provider must allow browser CORS requests.
- The API key is present in the browser process. This is acceptable for a local
  personal tool, but not for a public hosted site.
- Doubao direct mode expects a Doubao endpoint that speaks the same
  OpenAI-compatible transcription contract.
- Microphone mode uses the browser's `MediaRecorder` API. It records a local
  blob, then uploads it as the `file` field. It is not a live streaming protocol.
- Browser microphone access usually requires a secure context. `file://` and
  `http://localhost` are accepted by modern desktop browsers; arbitrary
  non-local HTTP origins are not.
- The provider must accept the browser's recorded format, commonly WebM/Opus on
  Chromium-based browsers.

For public deployments, add a server-side gateway later and keep provider keys on
the server.

For a WASM-backed web shell, use `crates/vo-web`. It exposes the same direct
provider contract plus browser microphone recording and browser-side config
storage while sharing `vo-core` transcript data. See [web-wasm.md](web-wasm.md).
