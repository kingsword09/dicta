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
    transcript: String?
) async throws {
    guard mic || speaker else {
        throw ValidationError("Cannot disable both mic and speaker. Drop one of --no-mic / --no-speaker.")
    }

    let sourceLocale = Locale(identifier: src)
    let targetLocale = dst.map { Locale(identifier: $0) }

    let mode = detectRenderMode(jsonForced: json)
    let isTTY = (mode == .tty)

    // Stream JSONL to disk as utterances finalize, so memory stays bounded for long
    // sessions. With --transcript we write straight to the user-specified path.
    // Without it, we use a temp file and decide at shutdown whether to keep or
    // discard. If the session can do neither (no --transcript AND no save prompt —
    // e.g. piped JSONL run), skip the temp file entirely so we don't burn disk I/O
    // writing bytes we know we are about to discard, and don't leave a large temp
    // behind on a SIGKILL.
    let canSaveTranscript = transcript != nil || (isTTY && canPromptForLog())
    let sessionLog: SessionLog? = canSaveTranscript ? try SessionLog.open(explicitPath: transcript) : nil

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
        showChannelLabel: mic && speaker,
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

    // pipeline.cancel() lets pipeline.run() below return, so after SIGINT the natural
    // completion path races the handler to finalizeSession. The gate makes exactly one
    // of them finalize (a double run would print the save prompt and summary twice and
    // have two readLine calls fighting over stdin).
    let finalizeGate = FinalizeGate()

    let signalSource = DispatchSource.makeSignalSource(signal: SIGINT, queue: .main)
    signalSource.setEventHandler {
        Task {
            // Stop the audio sources before the save prompt blocks. Otherwise mic
            // and speaker capture keep running for as long as the user is reading
            // the prompt, which is wasted work and a small privacy footgun.
            await pipeline.cancel()
            guard await finalizeGate.claim() else {
                // Finalization is already underway (a repeat Ctrl-C, likely at the
                // save prompt). Abort hard but leave the temp file in place so the
                // captured session is still recoverable.
                Foundation.exit(130)
            }
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

    if await finalizeGate.claim() {
        let count = await renderer.utteranceCount
        await finalizeSession(
            sessionLog: sessionLog,
            isTTY: isTTY,
            count: count,
            duration: Date().timeIntervalSince(startedAt)
        )
    } else {
        // The SIGINT handler claimed finalization and will exit the process.
        // Returning here would tear it down mid-prompt, so park until that exit.
        while true {
            try? await Task.sleep(nanoseconds: 1_000_000_000)
        }
    }
}

/// One-shot gate so the SIGINT handler and the natural completion path cannot both
/// run finalizeSession.
private actor FinalizeGate {
    private var claimed = false

    /// Returns `true` exactly once; every later call returns `false`.
    func claim() -> Bool {
        if claimed { return false }
        claimed = true
        return true
    }
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
