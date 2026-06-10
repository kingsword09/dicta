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
            await r.handle(.finalized(channel: .mic, seq: 0, source: "Hello", timing: timing))
            await r.handle(.finalized(channel: .speaker, seq: 1, source: "World", timing: timing))
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
            await r.handle(.finalized(channel: .mic, seq: 0, source: "final", timing: timing))
            await r.handle(.translated(seq: 0, target: "確定"))
        }

        #expect(objs.count == 1)
        #expect(src(objs[0])?["text"] as? String == "final")
    }

    /// Without a target locale the renderer is transcribe-only: no `dst` key, and the
    /// chunk commits immediately (no translation gate).
    @Test func transcribeOnlyOmitsDst() async {
        let objs = await renderJSONL(translationEnabled: false, showChannelLabel: false) { r in
            await r.handle(.finalized(channel: .mic, seq: 0, source: "no translation", timing: timing))
        }

        #expect(objs.count == 1)
        #expect(objs[0]["dst"] == nil)
        #expect(src(objs[0])?["text"] as? String == "no translation")
    }

    /// On EOF, a chunk whose translation never arrived is force-committed with an
    /// explicit JSON null `dst.text`.
    @Test func eofForceCommitsUntranslatedWithNullTarget() async {
        let objs = await renderJSONL(translationEnabled: true) { r in
            await r.handle(.finalized(channel: .mic, seq: 0, source: "lonely", timing: timing))
        }

        #expect(objs.count == 1)
        #expect(dst(objs[0])?["lang"] as? String == "ja-JP")
        #expect(dst(objs[0])?["text"] is NSNull)
    }

    /// Audio range from the transcriber is echoed into the JSONL `audio` object.
    @Test func audioRangeIsEmitted() async {
        let objs = await renderJSONL(translationEnabled: false) { r in
            await r.handle(.finalized(channel: .speaker, seq: 0, source: "x", timing: timing))
        }

        let audio = objs[0]["audio"] as? [String: Any]
        #expect(audio?["start"] as? Double == 0.124)
        #expect(audio?["end"] as? Double == 1.582)
        #expect(objs[0]["channel"] as? String == "speaker")
    }
}
