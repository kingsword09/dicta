import ArgumentParser
import Foundation

@main
struct Vo: AsyncParsableCommand {
    static let configuration = CommandConfiguration(
        commandName: "vo",
        abstract: "On-device live transcription and translation for macOS 26+",
        version: "0.8.0"
    )

    // MARK: - Capture options

    @Option(name: .long, help: "Source locale, BCP-47 (e.g. en-US, ja-JP). Defaults to your system locale.")
    var src: String = Locale.current.identifier(.bcp47)

    @Option(name: .long, help: "Target locale, BCP-47. Omit to skip translation.")
    var dst: String?

    @Flag(name: .long, help: "Disable microphone capture")
    var noMic: Bool = false

    @Flag(name: .long, help: "Disable system audio (speaker) capture via Core Audio process tap")
    var noSpeaker: Bool = false

    @Flag(name: .long, help: "Apply system voice processing (echo cancellation + noise reduction) on mic input. Note: this lowers system speaker volume while active.")
    var voiceProcessing: Bool = false

    @Flag(name: .long, help: "Interactively pick the mic / speaker device at startup. The picked device is pinned, so later system-default changes are ignored. Needs an interactive terminal.")
    var selectDevice: Bool = false

    @Option(name: .long, help: "Transcribe an on-disk audio file instead of live mic / speaker. Any format AVAudioFile can read (wav, m4a, mp3, caf, …). Runs as fast as the analyzer allows. Mutually exclusive with --no-mic / --no-speaker / --voice-processing / --select-device.")
    var input: String?

    // MARK: - Diagnostic

    @Flag(name: .long, help: "Print full environment diagnostics (system, models, devices) and exit.")
    var doctor: Bool = false

    // MARK: - Output

    @Flag(name: .long, help: "Force machine-readable JSON output. Without this, auto-detects based on whether STDOUT is a TTY.")
    var json: Bool = false

    @Option(name: .long, help: "Stream finalized chunks as JSONL to this path (written incrementally so you can `tail -f` it; memory stays bounded for long sessions). Skips the interactive save prompt.")
    var transcript: String?

    // MARK: - Run

    func run() async throws {
        if doctor {
            try await runDoctor(json: json)
            return
        }

        try await runListen(
            src: src,
            dst: dst,
            json: json,
            mic: !noMic,
            speaker: !noSpeaker,
            voiceProcessing: voiceProcessing,
            selectDevice: selectDevice,
            input: input,
            transcript: transcript
        )
    }
}
