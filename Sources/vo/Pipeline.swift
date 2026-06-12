import Foundation
@preconcurrency import AVFoundation
import Speech
import Translation

/// Monotonic sequence counter shared across channels.
actor SeqCounter {
    private var n = 0
    func next() -> Int {
        defer { n += 1 }
        return n
    }
}

/// Wrapper that lets us pass MicCapture/SpeakerCapture stop closures across actor
/// boundaries without making the capture classes themselves Sendable. They are owned
/// strictly by their channel Task and the registry is only ever invoked from a
/// serialized actor context (or from `runChannel`'s own defer), so this is safe.
struct AsyncStopper: @unchecked Sendable {
    let action: () async -> Void
}

/// Holds per-channel "stop the audio source" closures so the SIGINT handler can
/// stop capture before blocking on the save prompt. Without this the OS keeps
/// pulling mic/speaker frames while the user is deciding where to save the log.
actor StopRegistry {
    private var stoppers: [AsyncStopper] = []
    private var stopped: Bool = false

    /// Register a stopper. If `stopAll()` has already run, the stopper is invoked
    /// immediately instead of being stored — otherwise a channel that finished
    /// starting its audio source after SIGINT would leave that source running
    /// while the save prompt blocks.
    func register(_ stopper: AsyncStopper) async {
        if stopped {
            await stopper.action()
            return
        }
        stoppers.append(stopper)
    }

    /// Run every registered stopper exactly once, in registration order. Subsequent
    /// `register` calls invoke the stopper inline; subsequent `stopAll` calls are no-ops.
    func stopAll() async {
        stopped = true
        let pending = stoppers
        stoppers.removeAll()
        for s in pending { await s.action() }
    }
}

/// End-to-end pipeline that orchestrates one or more audio channels.
///
/// For each enabled channel:
///   AudioSource -> AVAudioConverter -> SpeechTranscriber -> finalized chunk
///                                                       -> TranslationSession
///                                                       -> Renderer
///
/// Mic and Speaker run concurrently in a TaskGroup but share one renderer + one seq counter,
/// so the output order reflects which channel finalized first.
struct Pipeline {
    let sourceLocale: Locale
    let targetLocale: Locale?  // nil = transcribe only, no translation
    let renderer: any Renderer
    let enableMic: Bool
    let enableSpeaker: Bool
    let voiceProcessing: Bool  // apply AEC + NR + AGC on mic input
    /// Lets callers (notably the SIGINT handler in Listen.swift) stop every active
    /// audio source without having to wait for `run()` to unwind on its own.
    let stops: StopRegistry = StopRegistry()

    func run() async throws {
        let counter = SeqCounter()

        // Resolve models once, before any channel starts. Both channels share the
        // source locale, so doing this here (instead of per-channel) avoids a double
        // download and lets us emit a single progress line. The speech asset can be
        // fetched headlessly; the translation model cannot, so we fail fast with
        // install instructions rather than letting every chunk surface a failure.
        try await ensureSpeechAsset(locale: sourceLocale)
        if let targetLocale {
            try await ensureTranslationModel(source: sourceLocale, target: targetLocale)
        }

        try await withThrowingTaskGroup(of: Void.self) { group in
            if enableMic {
                group.addTask {
                    try await self.runChannel(channel: .mic, counter: counter)
                }
            }
            if enableSpeaker {
                group.addTask {
                    try await self.runChannel(channel: .speaker, counter: counter)
                }
            }
            for try await _ in group {}
        }

        await renderer.handle(.eof)
        await renderer.flush()
    }

    /// Stop every registered audio source. Idempotent. Use this to halt the mic and
    /// speaker captures before the SIGINT handler blocks on an interactive prompt.
    func cancel() async {
        await stops.stopAll()
    }

    /// Build the callback a capture runs when its audio device changes mid-session.
    /// vo does not follow the new device; it announces why on stderr and then runs the
    /// same graceful shutdown as Ctrl-C. stopAll finishes every channel's stream, so
    /// run() returns and the natural completion path saves the transcript and prints
    /// the summary. The user restarts vo to pick up the new device.
    private func deviceLostHandler(for channel: AudioChannel) -> @Sendable () -> Void {
        let stops = self.stops
        let what = channel == .mic ? "microphone input device" : "system audio output device"
        return {
            emitProgress("vo: the \(what) changed or disconnected. Stopping. Restart vo to use the new device.")
            Task { await stops.stopAll() }
        }
    }

    // MARK: - Per-channel

    private func runChannel(
        channel: AudioChannel,
        counter: SeqCounter
    ) async throws {
        let willTranslate = targetLocale != nil
        // `.fastResults` biases the transcriber toward a shorter context window so
        // both volatile and finalized chunks arrive with lower latency. Apple
        // documents an accuracy trade-off but it is acceptable here: the live
        // region is a preview that gets replaced by the finalized line, and the
        // finalized line still benefits from the lower latency. This pair of
        // options (volatileResults + fastResults) is what
        // `SpeechTranscriber.Preset.timeIndexedProgressiveTranscription` would
        // give us; we spell it out explicitly to keep the trade-off readable.
        // Request per-run transcription confidence alongside the time range. It is
        // aggregated per chunk (mean + min) and emitted under `src.confidence`. Note
        // the value is acoustic per-character confidence, not word correctness.
        let transcriber = SpeechTranscriber(
            locale: sourceLocale,
            transcriptionOptions: [],
            reportingOptions: [.volatileResults, .fastResults],
            attributeOptions: [.audioTimeRange, .transcriptionConfidence]
        )
        let analyzer = SpeechAnalyzer(modules: [transcriber])

        guard let analyzerFormat = await SpeechAnalyzer.bestAvailableAudioFormat(compatibleWith: [transcriber]) else {
            throw VoError.noCompatibleAudioFormat
        }

        // Start audio capture and the resampler BEFORE warming the analyzer so
        // any speech the user produces during the ANE compile window lands in
        // the inputBuilder buffer instead of being lost. Without this ordering
        // the ~1.5 s `prepareToAnalyze` would silently drop the first words of
        // a session.
        let audioStream: AsyncStream<AVAudioPCMBuffer>
        let stopper: () async -> Void

        switch channel {
        case .mic:
            let cap = MicCapture(voiceProcessing: voiceProcessing)
            cap.onDeviceLost = deviceLostHandler(for: .mic)
            try cap.start()
            audioStream = cap.stream
            stopper = { cap.stop() }
        case .speaker:
            let cap = SpeakerCapture()
            cap.onDeviceLost = deviceLostHandler(for: .speaker)
            try await cap.start()
            audioStream = cap.stream
            stopper = { await cap.stop() }
        }

        // Register the per-channel stopper so a SIGINT-triggered cancel() halts
        // mic and speaker capture before any save prompt blocks the main task.
        await stops.register(AsyncStopper(action: stopper))

        let (inputSeq, inputBuilder) = AsyncStream<AnalyzerInput>.makeStream()

        // Audio resampling task: pull from the source, convert to analyzer format,
        // and push into the SpeechAnalyzer input stream. Starting it now means
        // buffers accumulate in `inputBuilder` while the analyzer warms up; the
        // analyzer drains them once `start(inputSequence:)` connects below.
        let resampler = Task.detached {
            for await buffer in audioStream {
                if let converted = convertBuffer(buffer, to: analyzerFormat) {
                    inputBuilder.yield(AnalyzerInput(buffer: converted))
                }
            }
            inputBuilder.finish()
        }

        // Warm the model / ANE in parallel with the now-flowing capture so the
        // first finalized chunk comes back in ~1.45 s instead of ~2.2 s. If
        // either prepare/start throws, the resampler we just spawned would be
        // orphaned, so cancel it explicitly before propagating.
        do {
            try await analyzer.prepareToAnalyze(in: analyzerFormat)
            try await analyzer.start(inputSequence: inputSeq)
        } catch {
            resampler.cancel()
            inputBuilder.finish()
            await stopper()
            throw error
        }

        // Translation worker for this channel (only when targetLocale is set).
        // TranslationSession is non-Sendable so we create it inside the Task closure.
        let (chunkSeq, chunkBuilder) = AsyncStream<(Int, String)>.makeStream()
        let renderer = renderer
        let sourceLang = sourceLocale.language
        let targetLang = targetLocale?.language
        let translator: Task<Void, Never>? = willTranslate ? Task {
            guard let targetLang else { return }
            let session = TranslationSession(installedSource: sourceLang, target: targetLang)
            // run() already verified the pair via ensureTranslationModel (it throws
            // otherwise), so warm the model now to keep the first chunk's translation
            // off the lazy on-demand loading path. A warm-up failure is non-fatal (the
            // per-chunk translate path still surfaces real errors), but a cancellation
            // means shutdown started, so bail instead of entering the chunk loop.
            do {
                try await session.prepareTranslation()
            } catch is CancellationError {
                return
            } catch {}
            for await (seq, text) in chunkSeq {
                do {
                    let response = try await session.translate(text)
                    await renderer.handle(.translated(seq: seq, target: response.targetText))
                } catch {
                    await renderer.handle(.translated(seq: seq, target: "[translation failed: \(error.localizedDescription)]"))
                }
            }
        } : nil

        // Drain transcriber results.
        do {
            for try await result in transcriber.results {
                // SpeechTranscriber emits every chunk after the first with a leading
                // space (stream-concatenation artifact). Trim so TTY columns stay
                // aligned and the space doesn't leak into JSONL or translation input.
                let text = String(result.text.characters).trimmingCharacters(in: .whitespacesAndNewlines)
                if result.isFinal {
                    // Whitespace-only finals carry no content; skip them so they
                    // don't burn a seq or emit blank lines.
                    guard !text.isEmpty else { continue }
                    let seq = await counter.next()
                    // A CMTime can be valid yet infinite/indefinite (open-ended
                    // ranges); its `.seconds` is then non-finite, which would trap the
                    // downstream Int conversion in the renderer. Treat those as "no offset".
                    func finiteSeconds(_ t: CMTime) -> Double? {
                        guard t.isValid else { return nil }
                        let s = t.seconds
                        return s.isFinite ? s : nil
                    }
                    let timing = ChunkTiming(
                        timestamp: Date(),
                        audioStart: finiteSeconds(result.range.start),
                        audioEnd:   finiteSeconds(result.range.end)
                    )
                    let confidence = aggregateConfidence(result.text)
                    await renderer.handle(.finalized(channel: channel, seq: seq, source: text, timing: timing, confidence: confidence))
                    if willTranslate {
                        chunkBuilder.yield((seq, text))
                    }
                } else {
                    await renderer.handle(.volatile(channel: channel, text: text))
                }
            }
        } catch {
            resampler.cancel()
            chunkBuilder.finish()
            translator?.cancel()
            await stopper()
            throw error
        }

        resampler.cancel()
        await stopper()
        chunkBuilder.finish()
        await translator?.value
    }

    // MARK: - Helpers

    private func ensureSpeechAsset(locale: Locale) async throws {
        let supportedIDs = (await SpeechTranscriber.supportedLocales).map { $0.identifier(.bcp47) }
        let isSupported = supportedIDs.contains(locale.identifier(.bcp47))
        guard isSupported else {
            throw VoError.unsupportedSpeechLocale(locale, supported: supportedIDs)
        }

        let installed = await SpeechTranscriber.installedLocales
        let isInstalled = installed.contains { $0.identifier(.bcp47) == locale.identifier(.bcp47) }
        guard !isInstalled else { return }

        let transcriber = SpeechTranscriber(
            locale: locale,
            transcriptionOptions: [],
            reportingOptions: [.volatileResults, .fastResults],
            attributeOptions: [.audioTimeRange]
        )
        guard let req = try await AssetInventory.assetInstallationRequest(supporting: [transcriber]) else { return }
        // Unlike the Translation framework, the speech asset downloads headlessly.
        // Announce it only once a real request exists, so the first run's blocking
        // wait doesn't look like a hang without claiming a download that isn't happening.
        emitProgress("Downloading speech model for \(locale.identifier(.bcp47))… (first run only)")
        try await req.downloadAndInstall()
    }

    /// Verify the translation model for this pair is installed. The Translation
    /// framework only downloads via a UI confirmation sheet, which a CLI cannot
    /// present, so an uninstalled pair would otherwise fail silently on every
    /// chunk. Fail fast with install instructions instead.
    private func ensureTranslationModel(source: Locale, target: Locale) async throws {
        let status = await LanguageAvailability().status(from: source.language, to: target.language)
        switch status {
        case .installed:
            return
        case .supported:
            throw VoError.translationModelNotInstalled(source: source, target: target)
        case .unsupported:
            throw VoError.unsupportedTranslationPair(source: source, target: target)
        @unknown default:
            // A status we don't recognize is not a confirmed install, so fail fast
            // rather than letting an unverified pair reach the per-chunk translate path.
            throw VoError.translationModelNotInstalled(source: source, target: target)
        }
    }
}

/// Write a one-line status to stderr. Stays off stdout so it never corrupts the
/// JSONL stream a downstream reader may be consuming.
private func emitProgress(_ message: String) {
    FileHandle.standardError.write(Data((message + "\n").utf8))
}

/// Aggregate a finalized chunk's per-run `transcriptionConfidence` into a
/// length-weighted mean and the minimum run value, rounded to three decimals.
/// Returns nil when no run carried a confidence value. Runs split per character for
/// CJK text, so weighting by run length keeps multi-character (e.g. English word)
/// runs from being under-counted relative to single-character ones.
private func aggregateConfidence(_ text: AttributedString) -> ChunkConfidence? {
    var weightedSum = 0.0
    var weight = 0
    var lowest = Double.greatestFiniteMagnitude
    for run in text.runs {
        guard let c = run.transcriptionConfidence else { continue }
        let n = text[run.range].characters.count
        weightedSum += c * Double(n)
        weight += n
        lowest = Swift.min(lowest, c)
    }
    guard weight > 0 else { return nil }
    let round3 = { (x: Double) in (x * 1000).rounded() / 1000 }
    return ChunkConfidence(mean: round3(weightedSum / Double(weight)), min: round3(lowest))
}

/// Convert one PCM buffer to the analyzer's preferred format. Handles sample-rate conversion.
private func convertBuffer(_ source: AVAudioPCMBuffer, to dstFormat: AVAudioFormat) -> AVAudioPCMBuffer? {
    let srcFormat = source.format
    if srcFormat == dstFormat {
        return source
    }
    guard let converter = AVAudioConverter(from: srcFormat, to: dstFormat) else { return nil }

    let ratio = dstFormat.sampleRate / srcFormat.sampleRate
    let outCapacity = AVAudioFrameCount(Double(source.frameLength) * ratio + 1024)
    guard let dst = AVAudioPCMBuffer(pcmFormat: dstFormat, frameCapacity: outCapacity) else { return nil }

    final class State: @unchecked Sendable { var consumed = false }
    let state = State()
    var error: NSError?
    converter.convert(to: dst, error: &error) { _, status in
        if state.consumed {
            status.pointee = .endOfStream
            return nil
        }
        state.consumed = true
        status.pointee = .haveData
        return source
    }
    return error == nil ? dst : nil
}

enum VoError: Error, CustomStringConvertible {
    case noCompatibleAudioFormat
    case unsupportedSpeechLocale(Locale, supported: [String])
    case translationModelNotInstalled(source: Locale, target: Locale)
    case unsupportedTranslationPair(source: Locale, target: Locale)

    var description: String {
        switch self {
        case .noCompatibleAudioFormat:
            return "No audio format compatible with SpeechTranscriber is available on this device."

        case .translationModelNotInstalled(let s, let t):
            return """
            Translation model for \(s.identifier(.bcp47)) → \(t.identifier(.bcp47)) is not installed.
            macOS does not allow downloading it from a CLI, so install it once via
            System Settings > General > Language & Region > Translation Languages, then re-run.
            (Run `vo --doctor` to see translation languages available on this device.)
            """

        case .unsupportedTranslationPair(let s, let t):
            return """
            Translation from \(s.identifier(.bcp47)) to \(t.identifier(.bcp47)) is not supported on this device.
            Run `vo --doctor` to see available translation languages.
            """

        case .unsupportedSpeechLocale(let l, let supported):
            let id = l.identifier(.bcp47)
            let langCode = l.language.languageCode?.identifier ?? ""
            let nearby = supported
                .filter { $0.hasPrefix(langCode + "-") }
                .sorted()
            if !nearby.isEmpty {
                return """
                SpeechTranscriber does not support locale \(id). Try one of these regional variants:
                  \(nearby.joined(separator: ", "))
                """
            }
            return """
            SpeechTranscriber does not support locale \(id). Run `vo --doctor` to see all supported locales.
            """
        }
    }
}
