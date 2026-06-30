# Web direct mode

`web/direct` is a dependency-free browser integration layer for direct
transcription calls. It does not run a backend and does not require WASM.

The important file is:

```text
web/direct/vo-transcriber.js
```

`web/direct/index.html` is a static demo that shows the same module used as a
Web Component, a plain JavaScript function, and a React-style
`onAudioRecorded` handler.

Serve the repository with any static file server, then open the demo:

```console
$ python3 -m http.server 8765
$ open http://127.0.0.1:8765/web/direct/
```

This is only for loading browser assets; it is not a `vo` backend and it does
not proxy or store API keys. A local server also avoids browser-specific
`file://` restrictions for ES module loading.

## Provider contract

The module sends a browser `fetch` request with `FormData` to:

```text
POST {api_base}/v1/audio/transcriptions
```

Fields sent to the provider:

```text
file=<audio file or recorded blob>
model=<model id>
response_format=json
language=<optional language hint>
prompt=<optional prompt>
```

If an API key is provided, it is sent as:

```text
Authorization: Bearer <api key>
```

## Plain JavaScript

Use `transcribeAudio` when your app already has a `Blob` or `File`.

```js
import { transcribeAudio } from "./vo-transcriber.js";

const result = await transcribeAudio(audioBlob, {
  apiBase: "https://api.example.com",
  apiKey: "...",
  model: "doubao-asr",
  language: "zh-CN",
});

console.log(result.text);
```

## AI Elements SpeechInput

AI Elements `SpeechInput` records audio with `MediaRecorder` in browsers where
Web Speech API is unavailable. Its extension point is:

```ts
onAudioRecorded?: (audioBlob: Blob) => Promise<string>
```

`createVoAudioRecordedHandler` matches that shape directly:

```tsx
import { SpeechInput } from "@/components/ai-elements/speech-input";
import { createVoAudioRecordedHandler } from "./vo-transcriber.js";

const transcribeWithVo = createVoAudioRecordedHandler({
  apiBase: "https://api.example.com",
  apiKey: "...",
  model: "doubao-asr",
  language: "zh-CN",
});

export function Composer() {
  return (
    <SpeechInput
      lang="zh-CN"
      onAudioRecorded={transcribeWithVo}
      onTranscriptionChange={(text) => setInput(text)}
    />
  );
}
```

In React, create the handler outside the component when config is static, or
memoize it when config comes from state:

```tsx
const onAudioRecorded = useMemo(
  () => createVoAudioRecordedHandler({ apiBase, apiKey, model, language }),
  [apiBase, apiKey, model, language]
);
```

## Web Component

Importing the module registers `<vo-speech-recorder>` automatically.

```html
<script type="module" src="./vo-transcriber.js"></script>

<vo-speech-recorder
  api-base="https://api.example.com"
  api-key="..."
  model="doubao-asr"
  language="zh-CN"
></vo-speech-recorder>

<script>
  document.querySelector("vo-speech-recorder").addEventListener(
    "vo-transcription",
    (event) => {
      console.log(event.detail.text);
    }
  );
</script>
```

The custom element emits:

```text
vo-recording-start
vo-audio-recorded
vo-transcription-start
vo-transcription
vo-error
vo-state-change
```

## Constraints

- The target provider must allow browser CORS requests.
- The API key is present in the browser process. This is acceptable for a local
  personal tool, but not for a public hosted site.
- Doubao direct mode expects a Doubao endpoint that speaks the same
  OpenAI-compatible transcription contract.
- Browser microphone access usually requires a secure context. `file://` and
  `http://localhost` are accepted by modern desktop browsers; arbitrary
  non-local HTTP origins are not.
- The provider must accept the browser's recorded format, commonly WebM/Opus on
  Chromium-based browsers.

For public deployments, add a server-side gateway later and keep provider keys
on the server.

For a WASM-backed web shell, use `crates/vo-web`. It exposes the same direct
provider contract plus browser microphone recording and browser-side config
storage while sharing `vo-core` transcript data. See [web-wasm.md](web-wasm.md).
