# vo

`vo` is an on-device live transcription and translation CLI for macOS 26+.

## Features

- Live transcription via Apple's [`SpeechTranscriber`](https://developer.apple.com/documentation/speech/speechtranscriber) (on-device, no network)
- Live translation via [`TranslationSession`](https://developer.apple.com/documentation/translation/translationsession) (on-device)
- Mic and system speaker captured as separate channels
- Strict source-order output, even when translations arrive out of order
- TTY and JSONL output modes (auto-detected)
- Wall-clock timestamps and audio time range per chunk
- Optional voice processing (echo cancellation + noise reduction)
- Environment diagnostics via `--doctor`

## Install

**homebrew tap:**

```console
$ brew install k1LoW/tap/vo
```

> [!NOTE]
> `vo` is distributed as an ad-hoc signed binary. macOS attaches `com.apple.quarantine` to the downloaded tarball at the network-stack layer, so the formula runs `xattr -cr` against the installed binary in its `install` step to strip the attribute. Without this step, Gatekeeper would block the binary from launching on first run.

**manually:**

Download the binary from the [releases page](https://github.com/k1LoW/vo/releases), then strip the quarantine attribute and place it on your `PATH`.

```console
$ tar -xzf vo_*_darwin_arm64.tar.gz
$ xattr -cr vo
$ chmod +x vo
$ mv vo /usr/local/bin/
```

> [!NOTE]
> `xattr -cr` strips `com.apple.quarantine` that macOS attaches to downloaded archives. Without it, Gatekeeper blocks the ad-hoc signed binary from launching.

## Usage

```console
$ vo                                  # Listen to mic + speaker, transcribe only
$ vo --src en-US --dst ja-JP          # Transcribe and translate
$ vo --no-speaker                     # Mic only
$ vo --no-mic --src en-US --dst ja-JP # Speaker only, with translation
$ vo --json | jq                      # JSONL output for piping
$ vo --doctor                         # Environment diagnostics
```

`vo` opens the mic and system audio simultaneously, transcribes each channel with `SpeechTranscriber`, optionally translates each finalized chunk with `TranslationSession`, and prints results as you speak. Press `Ctrl-C` to stop and see a summary.

### TTY output

```
vo 0.1.0 — listening on mic + speaker (en-US → ja-JP)

08:34:56 [mic]  How are you doing?
                元気ですか？
08:34:58 [spk]  I'm fine, thanks.
                元気だよ、ありがとう。
```

The `[mic]` / `[spk]` label is shown only when both channels are active; with `--no-mic` or `--no-speaker` it is omitted and the timestamp alone marks each line. The timestamp shares the channel's tint (amber for mic, teal for speaker), so a single glance tells you which side just spoke.

Translation lines are shown in dim text under the source. Pairs are emitted in source order, so a slow translation holds back subsequent pairs to keep the output coherent.

### JSONL output

`--json` forces JSONL. When STDOUT is not a TTY, JSONL is selected automatically.

```jsonl
{"seq":0,"channel":"mic","timestamp":"2026-06-10T08:34:56.234+09:00","audio":{"start":0.124,"end":1.582},"src":{"lang":"en-US","text":"Hey, Tim."},"dst":{"lang":"ja-JP","text":"ねえ、ティム。"}}
```

`dst` is present only when `--dst` is given. `seq` is monotonic across both channels. `audio.start` / `audio.end` come from `SpeechTranscriber.Result.range`.

### Voice processing

`--voice-processing` turns on Apple's voice IO (echo cancellation + noise reduction + AGC). Useful when running mic + speaker on the same physical device without headphones. The trade-off is that the macOS audio session enters communication mode, which lowers system speaker volume while `vo` is running. Off by default.

### Flags

| Flag | Default | Description |
|------|---------|-------------|
| `--src` | system locale | Source locale (BCP-47, must be in `SpeechTranscriber.supportedLocales`) |
| `--dst` | (none) | Target locale. Omit to skip translation |
| `--no-mic` | (off, mic on) | Disable microphone capture |
| `--no-speaker` | (off, speaker on) | Disable system audio capture |
| `--voice-processing` | off | Apply echo cancellation on mic input |
| `--json` | | Force JSONL output |
| `--transcript <path>` | (none; prompts at exit in TTY) | Stream finalized chunks as JSONL to `<path>` incrementally. Skips the interactive save prompt |
| `--doctor` | | Print full environment diagnostics and exit |

`vo --doctor` lists supported locales, installed speech models, available translation language pairs, and audio input devices. Run it first if something behaves unexpectedly.

## Requirements

- macOS 26+
- Apple Silicon (Neural Engine)
- TCC permissions: Microphone, Speech Recognition, and Audio Recording (only when `--speaker` is enabled)

## Build

```console
$ swift build                 # debug build
$ ./scripts/build.sh          # release + embed Info.plist + ad-hoc codesign
```

## License

[MIT License](LICENSE)
