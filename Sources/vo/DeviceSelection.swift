import Foundation
import CoreAudio
#if canImport(Darwin)
import Darwin
#endif

/// Devices the user pinned at startup via `--select-device`. A nil field means
/// "fall back to the system default" (and keep following it).
struct SelectedDevices {
    var micDeviceID: AudioDeviceID?
    var speakerDeviceUID: String?
}

/// `true` when the picker can run. It needs stdin to be a terminal (a human can answer)
/// and stderr to be a terminal (the menu is visible). stdout is intentionally not checked
/// because the picker writes only to stderr, so piping/redirecting stdout (e.g.
/// `vo --select-device --json | jq`) still allows interactive selection.
func canSelectDevicesInteractively() -> Bool {
    return isatty(fileno(stdin)) != 0 && isatty(fileno(stderr)) != 0
}

/// Prompt the user to pick the mic / speaker device for the enabled channels.
/// Caller must ensure `canSelectDevicesInteractively()`. A picked device is pinned,
/// so subsequent system-default changes are ignored for that channel.
func selectDevicesInteractively(mic: Bool, speaker: Bool) throws -> SelectedDevices {
    var result = SelectedDevices()
    if mic {
        let inputs = try collectInputDevices()
        result.micDeviceID = promptForDevice(label: "microphone input", devices: inputs)
            .map { AudioDeviceID($0.id) }
    }
    if speaker {
        let outputs = try collectOutputDevices()
        result.speakerDeviceUID = promptForDevice(label: "speaker (system audio output)", devices: outputs)?.uid
    }
    return result
}

/// Present a numbered list and read a selection. Empty input picks the
/// default-marked device, or the first listed device when none is marked.
/// Returns nil only when there is nothing to pick, in which case the caller
/// falls back to the system default device.
///
/// The prompt is written to stderr, never stdout, so a `--json --select-device`
/// run in a terminal keeps stdout pure JSONL. Input is read from stdin.
private func promptForDevice(label: String, devices: [AudioDeviceInfo]) -> AudioDeviceInfo? {
    guard !devices.isEmpty else {
        promptWrite("vo: no \(label) devices found; using system default.\n")
        return nil
    }

    let defaultIndex = devices.firstIndex(where: { $0.isDefault }) ?? 0
    promptWrite("\nSelect \(label) device:\n")
    for (i, d) in devices.enumerated() {
        let marker = d.isDefault ? " (default)" : ""
        promptWrite("  \(i + 1)) \(d.name)\(marker)\n")
    }

    while true {
        promptWrite("> [\(defaultIndex + 1)]: ")
        guard let line = readLine() else { return devices[defaultIndex] }
        let trimmed = line.trimmingCharacters(in: .whitespacesAndNewlines)
        if trimmed.isEmpty { return devices[defaultIndex] }
        if let n = Int(trimmed), n >= 1, n <= devices.count {
            return devices[n - 1]
        }
        promptWrite("Enter a number between 1 and \(devices.count).\n")
    }
}

/// Write prompt text to stderr (unbuffered), keeping stdout reserved for data.
private func promptWrite(_ s: String) {
    FileHandle.standardError.write(Data(s.utf8))
}
