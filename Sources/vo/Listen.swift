import ArgumentParser
import Foundation

/// Run the live capture loop: mic + speaker -> SpeechTranscriber -> TranslationSession -> Renderer.
func runListen(
    src: String,
    dst: String?,
    json: Bool,
    mic: Bool,
    speaker: Bool,
    voiceProcessing: Bool,
    log: String?
) async throws {
    guard mic || speaker else {
        throw ValidationError("Cannot disable both mic and speaker. Drop one of --no-mic / --no-speaker.")
    }

    let sourceLocale = Locale(identifier: src)
    let targetLocale = dst.map { Locale(identifier: $0) }

    let mode = detectRenderMode(jsonForced: json)
    let isTTY = (mode == .tty)

    // Stream JSONL to disk as utterances finalize, so memory stays bounded for long
    // sessions. With --log we write straight to the user-specified path. Without it,
    // we use a temp file and decide at shutdown whether to keep or discard.
    let sessionLog = try SessionLog.open(explicitPath: log)

    // If we throw out of this function before finalizeSession runs (e.g. the pipeline
    // errors mid-stream), make sure no temp file is left behind. In explicit mode we
    // leave the user-supplied file in place.
    defer {
        sessionLog.close()
        if !sessionLog.isExplicit {
            sessionLog.discard()
        }
    }

    let renderer = StreamRenderer(
        mode: mode,
        sourceLang: sourceLocale.identifier(.bcp47),
        targetLang: targetLocale?.identifier(.bcp47) ?? "",
        translationEnabled: targetLocale != nil,
        logSink: sessionLog
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
            let count = await renderer.utteranceCount
            await finalizeSession(
                sessionLog: sessionLog,
                isTTY: isTTY,
                count: count,
                duration: Date().timeIntervalSince(startedAt)
            )
            Foundation.exit(0)
        }
    }
    signal(SIGINT, SIG_IGN)
    signalSource.resume()

    try await pipeline.run()

    let count = await renderer.utteranceCount
    await finalizeSession(
        sessionLog: sessionLog,
        isTTY: isTTY,
        count: count,
        duration: Date().timeIntervalSince(startedAt)
    )
}

/// Close the session log, run save/discard logic, then print the exit summary.
/// Called from both the SIGINT handler and the natural-completion path.
private func finalizeSession(
    sessionLog: SessionLog,
    isTTY: Bool,
    count: Int,
    duration: TimeInterval
) async {
    let status = resolveSessionLog(sessionLog: sessionLog, canPrompt: canPromptForLog())
    if isTTY {
        if let status { print(status) }
        printSummary(count: count, duration: duration)
    } else if let status, sessionLog.isExplicit {
        FileHandle.standardError.write(Data((status + "\n").utf8))
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
