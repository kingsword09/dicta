import ArgumentParser
import Foundation

/// Run the live capture loop: mic + speaker -> SpeechTranscriber -> TranslationSession -> Renderer.
func runListen(
    src: String,
    dst: String?,
    json: Bool,
    mic: Bool,
    speaker: Bool,
    voiceProcessing: Bool
) async throws {
    guard mic || speaker else {
        throw ValidationError("At least one of --mic or --speaker must be enabled.")
    }

    let sourceLocale = Locale(identifier: src)
    let targetLocale = dst.map { Locale(identifier: $0) }

    let mode = detectRenderMode(jsonForced: json)
    let isTTY = (mode == .tty)
    let renderer = StreamRenderer(
        mode: mode,
        sourceLang: sourceLocale.identifier(.bcp47),
        targetLang: targetLocale?.identifier(.bcp47) ?? "",
        translationEnabled: targetLocale != nil
    )

    let pipeline = Pipeline(
        sourceLocale: sourceLocale,
        targetLocale: targetLocale,
        renderer: renderer,
        enableMic: mic,
        enableSpeaker: speaker,
        voiceProcessing: voiceProcessing
    )

    let startedAt = Date()
    if isTTY {
        printBanner(sourceLocale: sourceLocale, targetLocale: targetLocale, mic: mic, speaker: speaker)
    }

    let signalSource = DispatchSource.makeSignalSource(signal: SIGINT, queue: .main)
    signalSource.setEventHandler {
        Task {
            await renderer.handle(.eof)
            await renderer.flush()
            if isTTY {
                let count = await renderer.utteranceCount
                printSummary(count: count, duration: Date().timeIntervalSince(startedAt))
            }
            Foundation.exit(0)
        }
    }
    signal(SIGINT, SIG_IGN)
    signalSource.resume()

    try await pipeline.run()

    if isTTY {
        let count = await renderer.utteranceCount
        printSummary(count: count, duration: Date().timeIntervalSince(startedAt))
    }
}

// MARK: - Banner / Summary

private func printBanner(sourceLocale: Locale, targetLocale: Locale?, mic: Bool, speaker: Bool) {
    let channels = [mic ? "mic" : nil, speaker ? "speaker" : nil]
        .compactMap { $0 }
        .joined(separator: " + ")
    let langs: String
    if let t = targetLocale {
        langs = "\(sourceLocale.identifier(.bcp47)) → \(t.identifier(.bcp47))"
    } else {
        langs = sourceLocale.identifier(.bcp47)
    }
    let version = Vo.configuration.version
    print("vo \(version) — listening on \(channels) (\(langs))")
    print("")
}

private func printSummary(count: Int, duration: TimeInterval) {
    let mins = Int(duration) / 60
    let secs = Int(duration) % 60
    print("")
    print("\(count) utterances in \(mins)m \(secs)s")
}
