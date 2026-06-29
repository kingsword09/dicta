import Foundation
import Testing
@testable import vo

/// Drives a `StreamRenderer` in JSONL mode against an in-memory pipe and returns the
/// emitted lines parsed back into dictionaries, so tests can assert on the wire format
/// without touching any audio / Speech / Translation framework.
private func renderJSONL(
    translationEnabled: Bool,
    showChannelLabel: Bool = true,
    _ body: (StreamRenderer) async -> Void
) async -> [[String: Any]] {
    let pipe = Pipe()
    let renderer = StreamRenderer(
        mode: .jsonl,
        sourceLang: "en-US",
        targetLang: translationEnabled ? "ja-JP" : "",
        translationEnabled: translationEnabled,
        showChannelLabel: showChannelLabel,
        out: pipe.fileHandleForWriting
    )

    await body(renderer)
    await renderer.handle(.eof)
    await renderer.flush()

    try? pipe.fileHandleForWriting.close()
    let data = pipe.fileHandleForReading.readDataToEndOfFile()
    try? pipe.fileHandleForReading.close()
    let text = String(decoding: data, as: UTF8.self)
    return text
        .split(separator: "\n")
        .compactMap {
            guard let d = $0.data(using: .utf8) else { return nil }
            return (try? JSONSerialization.jsonObject(with: d)) as? [String: Any]
        }
}

/// Same driver as `renderJSONL` but returns the raw emitted lines, so tests can assert
/// on the exact wire text (e.g. that a number is not leaking binary-float artifacts).
private func renderJSONLLines(
    translationEnabled: Bool,
    _ body: (StreamRenderer) async -> Void
) async -> [String] {
    let pipe = Pipe()
    let renderer = StreamRenderer(
        mode: .jsonl,
        sourceLang: "en-US",
        targetLang: translationEnabled ? "ja-JP" : "",
        translationEnabled: translationEnabled,
        showChannelLabel: true,
        out: pipe.fileHandleForWriting
    )

    await body(renderer)
    await renderer.handle(.eof)
    await renderer.flush()

    try? pipe.fileHandleForWriting.close()
    let data = pipe.fileHandleForReading.readDataToEndOfFile()
    try? pipe.fileHandleForReading.close()
    return String(decoding: data, as: UTF8.self)
        .split(separator: "\n")
        .map(String.init)
}

private func renderEventJSONL(
    translationEnabled: Bool,
    targetLang: String? = "ja-JP",
    _ body: (EventJSONRenderer) async -> Void
) async -> [[String: Any]] {
    let pipe = Pipe()
    let renderer = EventJSONRenderer(
        sourceLang: "en-US",
        targetLang: targetLang,
        translationEnabled: translationEnabled,
        out: pipe.fileHandleForWriting
    )

    await body(renderer)
    await renderer.flush()

    try? pipe.fileHandleForWriting.close()
    let data = pipe.fileHandleForReading.readDataToEndOfFile()
    try? pipe.fileHandleForReading.close()
    let text = String(decoding: data, as: UTF8.self)
    return text
        .split(separator: "\n")
        .compactMap {
            guard let d = $0.data(using: .utf8) else { return nil }
            return (try? JSONSerialization.jsonObject(with: d)) as? [String: Any]
        }
}

private let timing = ChunkTiming(
    timestamp: Date(timeIntervalSince1970: 1_700_000_000),
    audioStart: 0.124,
    audioEnd: 1.582
)

private func src(_ obj: [String: Any]) -> [String: Any]? { obj["src"] as? [String: Any] }
private func dst(_ obj: [String: Any]) -> [String: Any]? { obj["dst"] as? [String: Any] }

@Suite("StreamRenderer JSONL")
struct RendererTests {
    /// Invariant: strict source-order commit. A translation that arrives early for a
    /// later chunk must not jump ahead of an untranslated earlier chunk.
    @Test func commitsInSourceOrderDespiteOutOfOrderTranslations() async {
        let objs = await renderJSONL(translationEnabled: true) { r in
            await r.handle(.finalized(channel: .mic, seq: 0, source: "Hello", timing: timing, confidence: nil))
            await r.handle(.finalized(channel: .speaker, seq: 1, source: "World", timing: timing, confidence: nil))
            // seq 1 finishes translating first, but seq 0 is still pending.
            await r.handle(.translated(seq: 1, target: "世界"))
            await r.handle(.translated(seq: 0, target: "こんにちは"))
        }

        #expect(objs.count == 2)
        #expect(objs.compactMap { $0["seq"] as? Int } == [0, 1])
        #expect(src(objs[0])?["text"] as? String == "Hello")
        #expect(dst(objs[0])?["text"] as? String == "こんにちは")
        #expect(src(objs[1])?["text"] as? String == "World")
        #expect(dst(objs[1])?["text"] as? String == "世界")
    }

    /// Invariant: volatile (partial) updates never reach JSONL.
    @Test func volatileUpdatesAreNotEmitted() async {
        let objs = await renderJSONL(translationEnabled: true) { r in
            await r.handle(.volatile(channel: .mic, text: "in prog"))
            await r.handle(.finalized(channel: .mic, seq: 0, source: "final", timing: timing, confidence: nil))
            await r.handle(.translated(seq: 0, target: "確定"))
        }

        #expect(objs.count == 1)
        #expect(src(objs[0])?["text"] as? String == "final")
    }

    /// Without a target locale the renderer is transcribe-only: no `dst` key, and the
    /// chunk commits immediately (no translation gate).
    @Test func transcribeOnlyOmitsDst() async {
        let objs = await renderJSONL(translationEnabled: false, showChannelLabel: false) { r in
            await r.handle(.finalized(channel: .mic, seq: 0, source: "no translation", timing: timing, confidence: nil))
        }

        #expect(objs.count == 1)
        #expect(objs[0]["dst"] == nil)
        #expect(src(objs[0])?["text"] as? String == "no translation")
    }

    /// On EOF, a chunk whose translation never arrived is force-committed with an
    /// explicit JSON null `dst.text`.
    @Test func eofForceCommitsUntranslatedWithNullTarget() async {
        let objs = await renderJSONL(translationEnabled: true) { r in
            await r.handle(.finalized(channel: .mic, seq: 0, source: "lonely", timing: timing, confidence: nil))
        }

        #expect(objs.count == 1)
        #expect(dst(objs[0])?["lang"] as? String == "ja-JP")
        #expect(dst(objs[0])?["text"] is NSNull)
    }

    /// Audio range from the transcriber is echoed into the JSONL `audio` object.
    @Test func audioRangeIsEmitted() async {
        let objs = await renderJSONL(translationEnabled: false) { r in
            await r.handle(.finalized(channel: .speaker, seq: 0, source: "x", timing: timing, confidence: nil))
        }

        let audio = objs[0]["audio"] as? [String: Any]
        #expect(audio?["start"] as? Double == 0.124)
        #expect(audio?["end"] as? Double == 1.582)
        #expect(objs[0]["channel"] as? String == "speaker")
    }

    /// Audio offsets are serialized as a base-10 Decimal at three places, so a CMTime
    /// like 21.54s does not leak as 21.539999999999999 onto the wire.
    @Test func audioRangeSerializesWithoutFloatArtifacts() async {
        let t = ChunkTiming(timestamp: Date(timeIntervalSince1970: 1_700_000_000), audioStart: 19.32, audioEnd: 21.54)
        let lines = await renderJSONLLines(translationEnabled: false) { r in
            await r.handle(.finalized(channel: .speaker, seq: 0, source: "x", timing: t, confidence: nil))
        }

        #expect(lines.count == 1)
        #expect(lines[0].contains("\"start\":19.32"))
        #expect(lines[0].contains("\"end\":21.54"))
        #expect(!lines[0].contains("21.539999999999999"))
    }

    /// Confidence, when present, is emitted as a nested `{mean, min}` object under `src`.
    @Test func confidenceEmittedUnderSrc() async {
        let objs = await renderJSONL(translationEnabled: false) { r in
            await r.handle(.finalized(channel: .mic, seq: 0, source: "x", timing: timing, confidence: ChunkConfidence(mean: 0.73, min: 0.28)))
        }

        let conf = src(objs[0])?["confidence"] as? [String: Any]
        #expect(conf?["mean"] as? Double == 0.73)
        #expect(conf?["min"] as? Double == 0.28)
    }

    /// Confidence is serialized as a base-10 Decimal at three places, not a raw Double,
    /// so values like 0.948 do not leak as 0.94799999999999995 onto the wire.
    @Test func confidenceSerializesWithoutFloatArtifacts() async {
        let lines = await renderJSONLLines(translationEnabled: false) { r in
            await r.handle(.finalized(channel: .mic, seq: 0, source: "x", timing: timing, confidence: ChunkConfidence(mean: 0.948, min: 0.511)))
        }

        #expect(lines.count == 1)
        #expect(lines[0].contains("\"mean\":0.948"))
        #expect(lines[0].contains("\"min\":0.511"))
        #expect(!lines[0].contains("0.94799999999999995"))
    }

    /// A chunk with no per-run confidence omits the `confidence` key entirely.
    @Test func confidenceOmittedWhenNil() async {
        let objs = await renderJSONL(translationEnabled: false) { r in
            await r.handle(.finalized(channel: .mic, seq: 0, source: "x", timing: timing, confidence: nil))
        }

        #expect(src(objs[0])?["confidence"] == nil)
    }

    /// Per-chunk `srcLangOverride` takes precedence over the renderer's
    /// constructor `sourceLang`. This is how the multi-source reconciler
    /// surfaces the detected locale per utterance; without the override
    /// JSONL would always show the constructor default.
    @Test func srcLangOverrideOverridesConstructorDefault() async {
        let objs = await renderJSONL(translationEnabled: false) { r in
            await r.handle(.finalized(channel: .mic, seq: 0, source: "Bonjour", timing: timing, confidence: nil, srcLangOverride: "fr-FR", dstLangOverride: nil))
        }

        #expect(objs.count == 1)
        #expect(src(objs[0])?["lang"] as? String == "fr-FR")
    }

    /// Per-chunk `dstLangOverride` takes precedence over the renderer's
    /// constructor `targetLang`. Mirrors `srcLangOverride`; both knobs are
    /// how the bidi pipeline tells the renderer which `(src → dst)` pair the
    /// per-utterance winner belongs to.
    @Test func dstLangOverrideOverridesConstructorDefault() async {
        let objs = await renderJSONL(translationEnabled: true) { r in
            await r.handle(.finalized(channel: .mic, seq: 0, source: "Hello", timing: timing, confidence: nil, srcLangOverride: nil, dstLangOverride: "zh-CN"))
            await r.handle(.translated(seq: 0, target: "你好"))
        }

        #expect(objs.count == 1)
        #expect(dst(objs[0])?["lang"] as? String == "zh-CN")
    }

    /// Bidi scenario: a single renderer handles two utterances whose detected
    /// source / destination locales swap. Each chunk's `src.lang` and
    /// `dst.lang` reflects its own per-event overrides, not the constructor
    /// defaults, so a downstream reader sees the actual routing per chunk.
    @Test func perChunkOverridesSupportBidirectionalRouting() async {
        let objs = await renderJSONL(translationEnabled: true) { r in
            await r.handle(.finalized(channel: .mic, seq: 0, source: "Hello", timing: timing, confidence: nil, srcLangOverride: "en-US", dstLangOverride: "ja-JP"))
            await r.handle(.translated(seq: 0, target: "こんにちは"))
            await r.handle(.finalized(channel: .speaker, seq: 1, source: "元気ですか", timing: timing, confidence: nil, srcLangOverride: "ja-JP", dstLangOverride: "en-US"))
            await r.handle(.translated(seq: 1, target: "How are you"))
        }

        #expect(objs.count == 2)
        #expect(src(objs[0])?["lang"] as? String == "en-US")
        #expect(dst(objs[0])?["lang"] as? String == "ja-JP")
        #expect(src(objs[1])?["lang"] as? String == "ja-JP")
        #expect(dst(objs[1])?["lang"] as? String == "en-US")
    }

    /// When neither override is set, the renderer falls back to its
    /// constructor `sourceLang` / `targetLang`. Single-source mode relies on
    /// this fallback; spelling it out as a test guards against a future
    /// refactor that always emits the override values.
    @Test func nilOverridesFallBackToConstructorDefaults() async {
        let objs = await renderJSONL(translationEnabled: true) { r in
            await r.handle(.finalized(channel: .mic, seq: 0, source: "Hello", timing: timing, confidence: nil))
            await r.handle(.translated(seq: 0, target: "こんにちは"))
        }

        #expect(objs.count == 1)
        #expect(src(objs[0])?["lang"] as? String == "en-US")
        #expect(dst(objs[0])?["lang"] as? String == "ja-JP")
    }
}

@Suite("EventJSONRenderer JSONL")
struct EventJSONRendererTests {
    @Test func emitsTypedLiveEventsForRustCli() async {
        let objs = await renderEventJSONL(translationEnabled: true) { r in
            await r.handle(.meta(
                backend: "apple",
                src: "en-US",
                dst: "ja-JP",
                mic: true,
                speaker: false,
                devices: [LiveDeviceEvent(channel: .mic, name: "Built-in Mic", pinned: false)]
            ))
            await r.handle(.volatile(channel: .mic, text: "hel"))
            await r.handle(.finalized(channel: .mic, seq: 7, source: "hello", timing: timing, confidence: nil))
            await r.handle(.translated(seq: 7, target: "こんにちは", dstLangOverride: "ja-JP"))
            await r.handle(.eof)
        }

        #expect(objs.compactMap { $0["type"] as? String } == ["meta", "volatile", "finalized", "translated", "eof"])
        #expect(objs[0]["backend"] as? String == "apple")
        #expect((objs[0]["devices"] as? [[String: Any]])?.first?["name"] as? String == "Built-in Mic")
        #expect(objs[1]["text"] as? String == "hel")
        #expect(src(objs[2])?["text"] as? String == "hello")
        #expect(objs[2]["dst"] == nil)
        #expect(objs[3]["lang"] as? String == "ja-JP")
        #expect(objs[3]["text"] as? String == "こんにちは")
    }
}
