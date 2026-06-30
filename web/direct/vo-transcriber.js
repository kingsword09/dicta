const DEFAULT_MODEL = "doubao-asr";
const DEFAULT_PROVIDER = "openai-compatible";
const DEFAULT_MIME_CANDIDATES = [
  "audio/webm;codecs=opus",
  "audio/webm",
  "audio/mp4",
];

export class VoTranscriptionError extends Error {
  constructor(message, options = {}) {
    super(message);
    this.name = "VoTranscriptionError";
    this.status = options.status ?? null;
    this.body = options.body ?? "";
    this.cause = options.cause;
  }
}

export function transcriptionUrl(apiBase) {
  const base = String(apiBase || "").trim().replace(/\/+$/, "");
  if (!base) {
    throw new VoTranscriptionError("apiBase is required");
  }
  return base.endsWith("/v1")
    ? `${base}/audio/transcriptions`
    : `${base}/v1/audio/transcriptions`;
}

export function normalizeVoConfig(config = {}) {
  return {
    provider: config.provider || DEFAULT_PROVIDER,
    apiBase: String(config.apiBase || "").trim(),
    apiKey: String(config.apiKey || "").trim(),
    model: String(config.model || DEFAULT_MODEL).trim(),
    language: String(config.language || "").trim(),
    prompt: String(config.prompt || "").trim(),
    responseFormat: String(config.responseFormat || "json").trim(),
  };
}

export function recommendedRecordingMimeType() {
  if (typeof MediaRecorder === "undefined" || !MediaRecorder.isTypeSupported) {
    return "audio/webm";
  }
  return (
    DEFAULT_MIME_CANDIDATES.find((mimeType) =>
      MediaRecorder.isTypeSupported(mimeType)
    ) || "audio/webm"
  );
}

export function recommendedRecordingExtension(mimeType = "") {
  const normalized = mimeType.toLowerCase();
  if (normalized.includes("mp4") || normalized.includes("mpeg")) return "m4a";
  if (normalized.includes("ogg")) return "ogg";
  if (normalized.includes("wav")) return "wav";
  return "webm";
}

export function createAudioFile(audio, filename) {
  const FileCtor = globalThis.File;
  const BlobCtor = globalThis.Blob;

  if (typeof FileCtor !== "undefined" && audio instanceof FileCtor) {
    return audio;
  }
  if (typeof BlobCtor === "undefined" || !(audio instanceof BlobCtor)) {
    throw new VoTranscriptionError("audio must be a Blob or File");
  }

  const type = audio.type || recommendedRecordingMimeType();
  const name = filename || `vo-recording.${recommendedRecordingExtension(type)}`;
  if (typeof FileCtor === "undefined") {
    throw new VoTranscriptionError("File is unavailable in this environment");
  }
  return new FileCtor([audio], name, { type });
}

export function extractTranscriptText(payload, rawText = "") {
  if (typeof payload === "string") return payload;
  if (!payload || typeof payload !== "object") return rawText;
  if (typeof payload.text === "string") return payload.text;
  if (typeof payload.transcript === "string") return payload.transcript;
  if (typeof payload.source?.text === "string") return payload.source.text;
  return rawText;
}

export async function transcribeAudio(audio, config, options = {}) {
  const normalized = normalizeVoConfig(config);
  const file = createAudioFile(audio, options.filename);

  if (!normalized.apiBase) {
    throw new VoTranscriptionError("apiBase is required");
  }
  if (!normalized.model) {
    throw new VoTranscriptionError("model is required");
  }

  const body = new FormData();
  body.set("file", file, file.name);
  body.set("model", normalized.model);
  body.set("response_format", normalized.responseFormat || "json");
  if (normalized.language) body.set("language", normalized.language);
  if (normalized.prompt) body.set("prompt", normalized.prompt);

  const headers = new Headers(options.headers || {});
  if (normalized.apiKey && !headers.has("Authorization")) {
    headers.set("Authorization", `Bearer ${normalized.apiKey}`);
  }

  const startedAt = performance.now();
  const response = await fetch(transcriptionUrl(normalized.apiBase), {
    method: "POST",
    headers,
    body,
    signal: options.signal,
  });
  const rawText = await response.text();

  if (!response.ok) {
    throw new VoTranscriptionError(`Transcription failed with HTTP ${response.status}`, {
      status: response.status,
      body: rawText,
    });
  }

  let raw = rawText;
  try {
    raw = JSON.parse(rawText);
  } catch {
    raw = rawText;
  }

  const text = extractTranscriptText(raw, rawText);
  return {
    provider: normalized.provider,
    text,
    raw,
    audio: {
      name: file.name,
      size: file.size,
      type: file.type,
    },
    durationMs: Math.round(performance.now() - startedAt),
  };
}

export function createVoAudioRecordedHandler(config, options = {}) {
  return async (audioBlob) => {
    const result = await transcribeAudio(audioBlob, config, options);
    return result.text;
  };
}

export function supportsMicrophoneRecording() {
  return Boolean(
    globalThis.navigator?.mediaDevices?.getUserMedia &&
      typeof MediaRecorder !== "undefined"
  );
}

export class VoMicrophoneRecorder {
  constructor(options = {}) {
    this.options = options;
    this.state = "idle";
    this.recorder = null;
    this.stream = null;
    this.chunks = [];
    this.stopPromise = null;
    this.stopTimer = null;
  }

  async start() {
    if (!supportsMicrophoneRecording()) {
      throw new VoTranscriptionError("This browser does not support microphone recording");
    }
    if (this.state === "recording") {
      return this;
    }

    this.chunks = [];
    const constraints = this.options.constraints || { audio: true };
    this.stream = await navigator.mediaDevices.getUserMedia(constraints);
    const mimeType = this.options.mimeType || recommendedRecordingMimeType();

    try {
      this.recorder = new MediaRecorder(this.stream, { mimeType });
    } catch {
      this.recorder = new MediaRecorder(this.stream);
    }

    this.stopPromise = new Promise((resolve, reject) => {
      this.recorder.addEventListener("dataavailable", (event) => {
        if (event.data?.size > 0) this.chunks.push(event.data);
      });
      this.recorder.addEventListener("error", (event) => {
        reject(event.error || new VoTranscriptionError("Microphone recording failed"));
      });
      this.recorder.addEventListener("stop", () => {
        this.release();
        const type = this.recorder.mimeType || mimeType || "audio/webm";
        const blob = new Blob(this.chunks, { type });
        const filename =
          this.options.filename || `vo-recording.${recommendedRecordingExtension(type)}`;
        resolve(createAudioFile(blob, filename));
      });
    });

    this.recorder.start();
    this.state = "recording";

    const maxSeconds = Number(this.options.maxSeconds || 0);
    if (Number.isFinite(maxSeconds) && maxSeconds > 0) {
      this.stopTimer = setTimeout(() => {
        this.stop().catch(() => {});
      }, maxSeconds * 1000);
    }

    return this;
  }

  async stop() {
    if (!this.recorder || this.recorder.state === "inactive") {
      return this.stopPromise;
    }
    if (this.stopTimer) {
      clearTimeout(this.stopTimer);
      this.stopTimer = null;
    }
    this.state = "stopping";
    this.recorder.stop();
    return this.stopPromise;
  }

  cancel() {
    if (this.stopTimer) {
      clearTimeout(this.stopTimer);
      this.stopTimer = null;
    }
    if (this.recorder && this.recorder.state !== "inactive") {
      this.recorder.stop();
    }
    this.release();
    this.state = "idle";
  }

  release() {
    this.stream?.getTracks().forEach((track) => track.stop());
    this.stream = null;
  }
}

const BrowserHTMLElement =
  typeof HTMLElement === "undefined" ? class {} : HTMLElement;

export class VoSpeechRecorderElement extends BrowserHTMLElement {
  static get observedAttributes() {
    return [
      "api-base",
      "api-key",
      "model",
      "language",
      "prompt",
      "max-seconds",
      "disabled",
    ];
  }

  constructor() {
    super();
    this.state = "idle";
    this.config = {};
    this.transcriber = null;
    this.recorder = null;

    if (typeof this.attachShadow === "function") {
      this.attachShadow({ mode: "open" });
      this.handleClick = this.handleClick.bind(this);
      this.render();
    }
  }

  connectedCallback() {
    this.syncConfigFromAttributes();
    this.button?.addEventListener("click", this.handleClick);
    this.updateView();
  }

  disconnectedCallback() {
    this.button?.removeEventListener("click", this.handleClick);
    this.recorder?.cancel();
  }

  attributeChangedCallback() {
    this.syncConfigFromAttributes();
    this.updateView();
  }

  syncConfigFromAttributes() {
    this.config = {
      ...this.config,
      apiBase: this.getAttribute("api-base") || "",
      apiKey: this.getAttribute("api-key") || "",
      model: this.getAttribute("model") || DEFAULT_MODEL,
      language: this.getAttribute("language") || "",
      prompt: this.getAttribute("prompt") || "",
    };
  }

  async handleClick() {
    if (this.hasAttribute("disabled")) return;

    if (this.state === "recording") {
      await this.stopRecording();
      return;
    }

    if (this.state === "transcribing") return;
    await this.startRecording();
  }

  async startRecording() {
    try {
      const maxSeconds = Number(this.getAttribute("max-seconds") || 0);
      this.recorder = new VoMicrophoneRecorder({
        maxSeconds: Number.isFinite(maxSeconds) && maxSeconds > 0 ? maxSeconds : 0,
      });
      await this.recorder.start();
      this.setState("recording");
      this.emit("vo-recording-start", {});
      this.recorder.stopPromise
        ?.then((file) => {
          if (this.isConnected && this.state === "recording") {
            return this.transcribeRecordedFile(file);
          }
        })
        .catch((error) => {
          if (
            this.isConnected &&
            (this.state === "recording" || this.state === "stopping")
          ) {
            this.fail(error);
          }
        });
    } catch (error) {
      this.fail(error);
    }
  }

  async stopRecording() {
    try {
      this.setState("stopping");
      const file = await this.recorder.stop();
      await this.transcribeRecordedFile(file);
    } catch (error) {
      this.fail(error);
    }
  }

  async transcribeRecordedFile(file) {
    this.emit("vo-audio-recorded", { file });
    this.setState("transcribing");
    this.emit("vo-transcription-start", { file });

    const value = this.transcriber
      ? await this.transcriber(file, this.config)
      : await transcribeAudio(file, this.config);
    const result =
      typeof value === "string"
        ? { text: value, raw: value, audio: { name: file.name, size: file.size, type: file.type } }
        : value;

    this.setState("complete", result.text || "");
    this.emit("vo-transcription", result);
  }

  fail(error) {
    const message = error instanceof Error ? error.message : String(error);
    this.setState("error", message);
    this.emit("vo-error", { error, message });
  }

  setState(state, text = "") {
    this.state = state;
    this.updateView(text);
    this.emit("vo-state-change", { state, text });
  }

  emit(type, detail) {
    this.dispatchEvent(
      new CustomEvent(type, {
        bubbles: true,
        composed: true,
        detail,
      })
    );
  }

  render() {
    this.shadowRoot.innerHTML = `
      <style>
        :host {
          --vo-bg: oklch(0.985 0.006 205);
          --vo-ink: oklch(0.2 0.016 230);
          --vo-muted: oklch(0.46 0.018 230);
          --vo-line: oklch(0.83 0.012 220);
          --vo-accent: oklch(0.55 0.12 172);
          --vo-danger: oklch(0.52 0.16 28);
          display: inline-grid;
          gap: 8px;
          color: var(--vo-ink);
          font: 500 14px/1.4 ui-sans-serif, system-ui, sans-serif;
        }

        button {
          min-height: 44px;
          display: inline-grid;
          grid-template-columns: 10px auto;
          align-items: center;
          justify-content: center;
          gap: 10px;
          border: 1px solid var(--vo-line);
          border-radius: 8px;
          padding: 0 14px;
          background: var(--vo-bg);
          color: var(--vo-ink);
          cursor: pointer;
          font: inherit;
          font-weight: 760;
        }

        button:hover {
          border-color: color-mix(in oklch, var(--vo-accent) 58%, var(--vo-line));
        }

        button:focus-visible {
          outline: 3px solid color-mix(in oklch, var(--vo-accent) 32%, transparent);
          outline-offset: 2px;
        }

        button:disabled {
          cursor: not-allowed;
          opacity: 0.55;
        }

        [part="indicator"] {
          width: 10px;
          height: 10px;
          border-radius: 999px;
          background: var(--vo-muted);
        }

        :host([data-state="recording"]) [part="indicator"] {
          background: var(--vo-danger);
          animation: vo-pulse 1.1s ease-out infinite;
        }

        :host([data-state="transcribing"]) [part="indicator"] {
          background: var(--vo-accent);
        }

        [part="status"] {
          min-height: 20px;
          color: var(--vo-muted);
          font-size: 12px;
        }

        :host([data-state="error"]) [part="status"] {
          color: var(--vo-danger);
        }

        @keyframes vo-pulse {
          0% { transform: scale(1); opacity: 1; }
          100% { transform: scale(1.9); opacity: 0.18; }
        }

        @media (prefers-reduced-motion: reduce) {
          :host([data-state="recording"]) [part="indicator"] {
            animation: none;
          }
        }
      </style>
      <button part="button" type="button">
        <span part="indicator" aria-hidden="true"></span>
        <span part="label"></span>
      </button>
      <span part="status" role="status" aria-live="polite"></span>
    `;
    this.button = this.shadowRoot.querySelector("button");
    this.label = this.shadowRoot.querySelector('[part="label"]');
    this.statusNode = this.shadowRoot.querySelector('[part="status"]');
  }

  updateView(text = "") {
    if (!this.shadowRoot) return;
    this.setAttribute("data-state", this.state);
    this.button.disabled = this.hasAttribute("disabled") || this.state === "transcribing";
    this.button.setAttribute("aria-pressed", String(this.state === "recording"));

    const labelByState = {
      idle: "Record",
      recording: "Stop",
      stopping: "Stopping",
      transcribing: "Transcribing",
      complete: "Record again",
      error: "Try again",
    };
    const statusByState = {
      idle: supportsMicrophoneRecording() ? "Ready" : "Microphone unavailable",
      recording: "Listening",
      stopping: "Saving audio",
      transcribing: "Sending audio",
      complete: text || "Transcript ready",
      error: text || "Recording failed",
    };

    this.label.textContent = labelByState[this.state] || "Record";
    this.statusNode.textContent = statusByState[this.state] || "";
  }
}

export function defineVoSpeechRecorder(name = "vo-speech-recorder") {
  if (typeof customElements === "undefined") return;
  if (!customElements.get(name)) {
    customElements.define(name, VoSpeechRecorderElement);
  }
}

defineVoSpeechRecorder();
