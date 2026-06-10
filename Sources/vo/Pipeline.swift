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
        let transcriber = SpeechTranscriber(
            locale: sourceLocale,
            transcriptionOptions: [],
            reportingOptions: [.volatileResults, .fastResults],
            attributeOptions: [.audioTimeRange]
        )
        let analyzer = SpeechAnalyzer(modules: [transcriber])
        try await ensureSpeechAsset(for: transcriber, locale: sourceLocale)

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
            try cap.start()
            audioStream = cap.stream
            stopper = { cap.stop() }
        case .speaker:
            let cap = SpeakerCapture()
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
                    let timing = ChunkTiming(
                        timestamp: Date(),
                        audioStart: result.range.start.isValid ? result.range.start.seconds : nil,
                        audioEnd:   result.range.end.isValid   ? result.range.end.seconds   : nil
                    )
                    await renderer.handle(.finalized(channel: channel, seq: seq, source: text, timing: timing))
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

    private func ensureSpeechAsset(for transcriber: SpeechTranscriber, locale: Locale) async throws {
        let supportedIDs = (await SpeechTranscriber.supportedLocales).map { $0.identifier(.bcp47) }
        let isSupported = supportedIDs.contains(locale.identifier(.bcp47))
        guard isSupported else {
            throw VoError.unsupportedSpeechLocale(locale, supported: supportedIDs)
        }

        let installed = await SpeechTranscriber.installedLocales
        let isInstalled = installed.contains { $0.identifier(.bcp47) == locale.identifier(.bcp47) }
        if !isInstalled {
            if let req = try await AssetInventory.assetInstallationRequest(supporting: [transcriber]) {
                try await req.downloadAndInstall()
            }
        }
    }
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
    case noDisplayForScreenCapture

    var description: String {
        switch self {
        case .noCompatibleAudioFormat:
            return "No audio format compatible with SpeechTranscriber is available on this device."

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

        case .noDisplayForScreenCapture:
            return "No display available for ScreenCaptureKit. Speaker capture requires at least one display."
        }
    }
}
