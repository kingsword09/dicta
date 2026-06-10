import ArgumentParser
import Foundation

@main
struct Vo: AsyncParsableCommand {
    static let configuration = CommandConfiguration(
        commandName: "vo",
        abstract: "On-device live transcription and translation for macOS 26+",
        version: "0.1.0"
    )

    // MARK: - Capture options

    @Option(name: .long, help: "Source locale, BCP-47 (e.g. en-US, ja-JP). Defaults to your system locale.")
    var src: String = Locale.current.identifier(.bcp47)

    @Option(name: .long, help: "Target locale, BCP-47. Omit to skip translation.")
    var dst: String?

    @Flag(name: .long, help: "Disable microphone capture")
    var noMic: Bool = false

    @Flag(name: .long, help: "Disable system audio (speaker) capture via ScreenCaptureKit")
    var noSpeaker: Bool = false

    @Flag(name: .long, help: "Apply system voice processing (echo cancellation + noise reduction) on mic input. Note: this lowers system speaker volume while active.")
    var voiceProcessing: Bool = false

    // MARK: - Diagnostic

    @Flag(name: .long, help: "Print full environment diagnostics (system, models, devices) and exit.")
    var doctor: Bool = false

    // MARK: - Output

    @Flag(name: .long, help: "Force machine-readable JSON output. Without this, auto-detects based on whether STDOUT is a TTY.")
    var json: Bool = false

    @Option(name: .long, help: "Save finalized chunks as JSONL to this path on exit. Skips the interactive save prompt.")
    var log: String?

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
            log: log
        )
    }
}
