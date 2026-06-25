import ArgumentParser
import Foundation

/// Run the live capture loop: mic + speaker -> SpeechTranscriber -> TranslationSession -> Renderer.
/// When `input` is set, transcribe that file instead. Mic and speaker capture are
/// bypassed and the file feeds the same pipeline with end-to-end backpressure.
func runListen(
    src: String,
    dst: String?,
    json: Bool,
    mic: Bool,
    speaker: Bool,
    voiceProcessing: Bool,
    selectDevice: Bool,
    input: String?,
    transcript: String?
) async throws {
    let inputURL: URL?
    if let input {
        // --input is mutually exclusive with the live-capture switches. Each combination
        // is rejected with its own message so the user knows which flag to drop.
        if !mic { throw ValidationError("--no-mic has no meaning with --input (mic capture is already off).") }
        if !speaker { throw ValidationError("--no-speaker has no meaning with --input (speaker capture is already off).") }
        if voiceProcessing { throw ValidationError("--voice-processing only applies to mic capture; drop it when using --input.") }
        if selectDevice { throw ValidationError("--select-device only applies to mic / speaker capture; drop it when using --input.") }
        // Expand `~` so a quoted or non-shell-supplied path resolves the same way the
        // user's shell would, matching SessionLog's handling of `--transcript`.
        let resolved = (input as NSString).expandingTildeInPath
        inputURL = URL(fileURLWithPath: resolved)
    } else {
        inputURL = nil
        guard mic || speaker else {
            throw ValidationError("Cannot disable both mic and speaker. Drop one of --no-mic / --no-speaker.")
        }
    }

    // Claim vo's own TCC identity before touching audio, so the Microphone / Speech
    // / Audio Recording grants attach to vo rather than the launching terminal. Done
    // after the flag combination is known valid so an invalid run errors without
    // spawning. On a release build this re-execs and never returns here; the
    // disclaimed child resumes below. On a plain `swift build` (no embedded
    // Info.plist) it is a no-op and vo keeps the terminal's identity.
    // File mode still re-execs because the Speech Recognition grant attaches to vo
    // (the file path bypasses mic / audio-recording grants but the Speech framework
    // still checks Speech Recognition).
    Responsibility.reexecAsResponsibleProcess()

    let sourceLocales = try parseLocaleList(src, flag: "--src")
    let targetLocales: [Locale]?
    if let dst {
        let parsed = try parseLocaleList(dst, flag: "--dst")
        guard parsed.count == sourceLocales.count else {
            throw ValidationError("--dst must have the same number of locales as --src (got \(sourceLocales.count) source, \(parsed.count) target).")
        }
        targetLocales = parsed
    } else {
        targetLocales = nil
    }
    // For multi-source mode the renderer carries dummy defaults; every finalized
    // chunk arrives with srcLang / dstLang overrides set by the reconciler. For
    // single-source mode the defaults are what JSONL uses, so pass the actual id.
    let primarySource = sourceLocales[0]
    let primaryTarget = targetLocales?[0]

    // Resolve pinned devices before any capture starts. Done after the re-exec so the
    // prompt runs once in the disclaimed child (which owns stdin), and before the
    // banner so the picker output is not mixed into the live region.
    var selectedDevices = SelectedDevices()
    if selectDevice {
        // stdout may be piped/redirected freely (the picker writes to stderr); we only
        // need stdin to answer on and stderr to show the menu.
        guard canSelectDevicesInteractively() else {
            throw ValidationError("--select-device needs a terminal for stdin (your answer) and stderr (the menu). stdout may be piped.")
        }
        selectedDevices = try selectDevicesInteractively(mic: mic, speaker: speaker)
    }

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

    // File mode is always single-channel, so suppress the [file] label the same way the
    // single-channel live modes (--no-mic / --no-speaker) suppress [mic] / [spk].
    let showChannelLabel = inputURL == nil && mic && speaker

    let renderer = StreamRenderer(
        mode: mode,
        sourceLang: primarySource.identifier(.bcp47),
        targetLang: primaryTarget?.identifier(.bcp47) ?? "",
        translationEnabled: targetLocales != nil,
        showChannelLabel: showChannelLabel,
        logSink: sessionLog
    )

    let pipeline = Pipeline(
        sourceLocales: sourceLocales,
        targetLocales: targetLocales,
        renderer: renderer,
        enableMic: mic,
        enableSpeaker: speaker,
        voiceProcessing: voiceProcessing,
        micDeviceID: selectedDevices.micDeviceID,
        speakerDeviceUID: selectedDevices.speakerDeviceUID,
        inputURL: inputURL
    )

    let startedAt = Date()
    if isTTY {
        if let inputURL {
            printFileBanner(
                inputURL: inputURL,
                sourceLocales: sourceLocales,
                targetLocales: targetLocales
            )
        } else {
            let deviceLabels = resolvedCaptureDeviceLabels(mic: mic, speaker: speaker, selected: selectedDevices)
            printBanner(
                sourceLocales: sourceLocales,
                targetLocales: targetLocales,
                mic: mic,
                speaker: speaker,
                micDevice: deviceLabels.mic,
                speakerDevice: deviceLabels.speaker
            )
        }
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
        // The handler calls Foundation.exit long before one interval elapses, so
        // a long interval (rather than once per second) just avoids needless
        // wakeups while we wait. The loop reparks if the sleep is ever cancelled.
        let parkInterval: UInt64 = 3600 * 1_000_000_000  // 1 hour
        while true {
            try? await Task.sleep(nanoseconds: parkInterval)
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

private func printBanner(
    sourceLocales: [Locale],
    targetLocales: [Locale]?,
    mic: Bool,
    speaker: Bool,
    micDevice: CaptureDeviceLabel?,
    speakerDevice: CaptureDeviceLabel?
) {
    let channels = [mic ? "mic" : nil, speaker ? "speaker" : nil]
        .compactMap { $0 }
        .joined(separator: " + ")
    let langs = bannerLanguagePair(sourceLocales: sourceLocales, targetLocales: targetLocales)
    let version = Vo.configuration.version
    print("vo \(version) — listening on \(channels) (\(langs))")

    // Show which device each active channel is capturing from at startup. The channel
    // label uses the channel's tint (matching the live output); the pinned/default note
    // is dim so it reads as metadata. The label column aligns to "speaker" when both
    // channels are shown.
    let labelWidth = (mic && speaker) ? "speaker".count : 0
    func deviceLine(_ channel: AudioChannel, _ device: CaptureDeviceLabel) {
        let label = (channel == .mic) ? "mic" : "speaker"
        let padded = label.padding(toLength: max(labelWidth, label.count), withPad: " ", startingAt: 0)
        let note = device.pinned ? "[pinned]" : "(default)"
        print("  \(ansi256(channel.tint256, padded))  \(device.name) \(ansi256(244, note))")
    }
    if mic, let micDevice { deviceLine(.mic, micDevice) }
    if speaker, let speakerDevice { deviceLine(.speaker, speakerDevice) }

    print("")
}

private func printFileBanner(
    inputURL: URL,
    sourceLocales: [Locale],
    targetLocales: [Locale]?
) {
    let langs = bannerLanguagePair(sourceLocales: sourceLocales, targetLocales: targetLocales)
    let version = Vo.configuration.version
    let display = inputURL.isFileURL ? inputURL.path : inputURL.absoluteString
    print("vo \(version) — transcribing \(display) (\(langs))")
    print("")
}

/// Render the locale list pair shown in the banner. Single-locale collapses to the
/// familiar "en-US" / "en-US → ja-JP" forms; multi-locale uses a comma-separated
/// list on each side matching what the user typed.
private func bannerLanguagePair(sourceLocales: [Locale], targetLocales: [Locale]?) -> String {
    let srcStr = sourceLocales.map { $0.identifier(.bcp47) }.joined(separator: ",")
    if let targetLocales {
        let dstStr = targetLocales.map { $0.identifier(.bcp47) }.joined(separator: ",")
        return "\(srcStr) → \(dstStr)"
    }
    return srcStr
}

/// Split a comma-separated locale list flag value into trimmed Locale instances.
/// Empty list, empty entries, and duplicate locales each produce a ValidationError
/// naming the flag. Duplicates are rejected because ChunkReconciler keys per-region
/// candidates and TranslationLane keys translator pipes by `identifier(.bcp47)`,
/// so a repeat would either lock the reconciler into always waiting its 300ms
/// timeout (count stays under nLocales because two transcribers share one slot)
/// or silently overwrite a translator lane. Newlines are trimmed alongside spaces
/// so `--src "$(cat list.txt)"`-style usage does not leak a trailing newline into
/// the SpeechTranscriber locale lookup later.
private func parseLocaleList(_ raw: String, flag: String) throws -> [Locale] {
    let parts = raw.split(separator: ",", omittingEmptySubsequences: false).map {
        $0.trimmingCharacters(in: .whitespacesAndNewlines)
    }
    guard !parts.isEmpty, parts.allSatisfy({ !$0.isEmpty }) else {
        throw ValidationError("\(flag) cannot be empty or contain empty entries.")
    }
    let locales = parts.map { Locale(identifier: $0) }
    var seen: Set<String> = []
    for locale in locales {
        let id = locale.identifier(.bcp47)
        if !seen.insert(id).inserted {
            throw ValidationError("\(flag) contains duplicate locale \(id).")
        }
    }
    return locales
}

/// Wrap a string in a 256-color foreground SGR escape. Matches the renderer's palette.
private func ansi256(_ code: Int, _ s: String) -> String {
    "\u{001B}[38;5;\(code)m\(s)\u{001B}[0m"
}

private func printSummary(count: Int, duration: TimeInterval) {
    let mins = Int(duration) / 60
    let secs = Int(duration) % 60
    print("")
    print("\(count) utterances in \(mins)m \(secs)s")
}
