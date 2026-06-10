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
    // we use a temp file and decide at shutdown whether to keep or discard. If the
    // session can do neither (no --log AND no save prompt — e.g. piped JSONL run),
    // skip the temp file entirely so we don't burn disk I/O writing bytes we know we
    // are about to discard, and don't leave a large temp behind on a SIGKILL.
    let canSaveLog = log != nil || (isTTY && canPromptForLog())
    let sessionLog: SessionLog? = canSaveLog ? try SessionLog.open(explicitPath: log) : nil

    // If we throw out of this function before finalizeSession runs (e.g. the pipeline
    // errors mid-stream), make sure no temp file is left behind. In explicit mode we
    // leave the user-supplied file in place. We also skip discard when resolveSessionLog
    // has flagged the temp file as preserved for manual recovery (move-target failure),
    // since that is the one case where the temp file holding the captured data is what
    // we want the user to find.
    defer {
        if let sessionLog {
            sessionLog.close()
            if !sessionLog.isExplicit && !sessionLog.preservedForRecovery {
                sessionLog.discard()
            }
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
            // Stop the audio sources before the save prompt blocks. Otherwise mic
            // and speaker capture keep running for as long as the user is reading
            // the prompt, which is wasted work and a small privacy footgun.
            await pipeline.cancel()
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
    sessionLog: SessionLog?,
    isTTY: Bool,
    count: Int,
    duration: TimeInterval
) async {
    if let sessionLog {
        // Gate the save prompt on isTTY (renderer mode), not just on canPromptForLog().
        // Otherwise `vo --json` run from an interactive shell — where STDIN and STDOUT
        // are both TTYs but the renderer is emitting machine-readable JSONL — would
        // interleave the prompt text into the JSONL stream and corrupt it.
        let status = resolveSessionLog(sessionLog: sessionLog, canPrompt: isTTY && canPromptForLog())
        if isTTY {
            if let status { print(status) }
        } else if let status, sessionLog.isExplicit {
            FileHandle.standardError.write(Data((status + "\n").utf8))
        }
    }
    if isTTY {
        printSummary(count: count, duration: duration)
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
