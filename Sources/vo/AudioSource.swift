import Foundation
@preconcurrency import AVFoundation
import CoreAudio
import AudioToolbox

enum AudioChannel: String, Sendable, CaseIterable {
    case mic
    case speaker

    var shortLabel: String {
        switch self {
        case .mic:     return "MIC"
        case .speaker: return "SPK"
        }
    }
}

/// Microphone capture using AVAudioEngine.
final class MicCapture: @unchecked Sendable {
    let stream: AsyncStream<AVAudioPCMBuffer>
    private let builder: AsyncStream<AVAudioPCMBuffer>.Continuation
    private let engine = AVAudioEngine()
    private let voiceProcessing: Bool

    init(voiceProcessing: Bool = false) {
        self.voiceProcessing = voiceProcessing
        let (s, b) = AsyncStream<AVAudioPCMBuffer>.makeStream()
        self.stream = s
        self.builder = b
    }

    func start() throws {
        let inputNode = engine.inputNode

        // Optionally enable system voice processing (echo cancellation + noise reduction + AGC).
        // Trade-off: stops the mic from re-capturing audio from the speaker, but switches the
        // OS audio session to a "communication" mode that attenuates system output volume.
        // Off by default; enable with --voice-processing.
        if voiceProcessing {
            do {
                try inputNode.setVoiceProcessingEnabled(true)
            } catch {
                FileHandle.standardError.write(Data(
                    "vo: warning: voice processing unavailable on mic input, continuing without echo cancellation (\(error.localizedDescription))\n".utf8
                ))
            }
        }

        let inputFormat = inputNode.outputFormat(forBus: 0)
        let builder = self.builder
        inputNode.installTap(onBus: 0, bufferSize: 4096, format: inputFormat) { buffer, _ in
            // Copy the buffer because installTap reuses the underlying storage.
            guard let copy = buffer.copy() else { return }
            builder.yield(copy)
        }
        try engine.start()
    }

    func stop() {
        engine.inputNode.removeTap(onBus: 0)
        engine.stop()
        builder.finish()
    }
}

/// System audio (speaker) capture using Core Audio process taps (macOS 14.4+).
/// Requires the Audio Recording TCC permission, not Screen Recording: ScreenCaptureKit
/// would force a screen-capture grant even though vo only ever wanted the audio.
final class SpeakerCapture: @unchecked Sendable {
    let stream: AsyncStream<AVAudioPCMBuffer>
    private let builder: AsyncStream<AVAudioPCMBuffer>.Continuation

    private var tapID = AudioObjectID(kAudioObjectUnknown)
    private var aggregateID = AudioObjectID(kAudioObjectUnknown)
    private var ioProcID: AudioDeviceIOProcID?
    // Runs the IOProc off Core Audio's real-time IO thread: the block deep-copies
    // and yields into an AsyncStream, neither of which is safe on the RT thread.
    private let ioQueue = DispatchQueue(label: "vo.spk.audio")

    init() {
        let (s, b) = AsyncStream<AVAudioPCMBuffer>.makeStream()
        self.stream = s
        self.builder = b
    }

    func start() async throws {
        // Any step past the first allocation can throw, and Pipeline.runChannel only
        // registers stop() once start() returns. So clean up partial allocations here
        // before rethrowing, otherwise the tap / aggregate would dangle.
        do {
            try await startUnchecked()
        } catch {
            await stop()
            throw error
        }
    }

    private func startUnchecked() async throws {
        // Global system-output tap. vo never plays audio, so there is nothing of
        // our own to exclude from the mix.
        let tapDescription = CATapDescription(stereoGlobalTapButExcludeProcesses: [])
        tapDescription.uuid = UUID()
        tapDescription.muteBehavior = .unmuted
        tapDescription.isPrivate = true

        var tap = AudioObjectID(kAudioObjectUnknown)
        var status = AudioHardwareCreateProcessTap(tapDescription, &tap)
        guard status == noErr else { throw CoreAudioError(code: status, op: "CreateProcessTap") }
        self.tapID = tap

        // A process tap carries no clock of its own, so it has to ride an aggregate
        // device anchored to the current default output device.
        let outputUID = try defaultOutputDeviceUID()
        let aggregateUID = UUID().uuidString
        let description: [String: Any] = [
            kAudioAggregateDeviceNameKey: "vo-system-tap",
            kAudioAggregateDeviceUIDKey: aggregateUID,
            kAudioAggregateDeviceMainSubDeviceKey: outputUID,
            kAudioAggregateDeviceIsPrivateKey: true,
            kAudioAggregateDeviceIsStackedKey: false,
            kAudioAggregateDeviceTapAutoStartKey: true,
            kAudioAggregateDeviceSubDeviceListKey: [
                [kAudioSubDeviceUIDKey: outputUID]
            ],
            kAudioAggregateDeviceTapListKey: [
                [
                    kAudioSubTapDriftCompensationKey: true,
                    kAudioSubTapUIDKey: tapDescription.uuid.uuidString,
                ]
            ],
        ]

        var aggregate = AudioObjectID(kAudioObjectUnknown)
        status = AudioHardwareCreateAggregateDevice(description as CFDictionary, &aggregate)
        guard status == noErr else { throw CoreAudioError(code: status, op: "CreateAggregateDevice") }
        self.aggregateID = aggregate

        var asbd = try tapStreamFormat(tap)
        guard let format = AVAudioFormat(streamDescription: &asbd) else {
            throw VoError.noCompatibleAudioFormat
        }

        let builder = self.builder
        var procID: AudioDeviceIOProcID?
        status = AudioDeviceCreateIOProcIDWithBlock(&procID, aggregate, ioQueue) { _, inInputData, _, _, _ in
            // The no-copy buffer aliases Core Audio's IO memory; downstream consumers
            // run async and must own their data, so deep-copy before yielding.
            guard let aliased = AVAudioPCMBuffer(pcmFormat: format, bufferListNoCopy: inInputData),
                  let owned = aliased.copy() else { return }
            builder.yield(owned)
        }
        guard status == noErr, let procID else {
            throw CoreAudioError(code: status, op: "CreateIOProc")
        }
        self.ioProcID = procID

        status = AudioDeviceStart(aggregate, procID)
        guard status == noErr else { throw CoreAudioError(code: status, op: "DeviceStart") }
    }

    func stop() async {
        if let procID = ioProcID, aggregateID != AudioObjectID(kAudioObjectUnknown) {
            AudioDeviceStop(aggregateID, procID)
            AudioDeviceDestroyIOProcID(aggregateID, procID)
            ioProcID = nil
        }
        if aggregateID != AudioObjectID(kAudioObjectUnknown) {
            AudioHardwareDestroyAggregateDevice(aggregateID)
            aggregateID = AudioObjectID(kAudioObjectUnknown)
        }
        if tapID != AudioObjectID(kAudioObjectUnknown) {
            AudioHardwareDestroyProcessTap(tapID)
            tapID = AudioObjectID(kAudioObjectUnknown)
        }
        builder.finish()
    }

    /// UID of the current default output device, used as the aggregate's main sub-device.
    private func defaultOutputDeviceUID() throws -> String {
        var address = AudioObjectPropertyAddress(
            mSelector: kAudioHardwarePropertyDefaultOutputDevice,
            mScope: kAudioObjectPropertyScopeGlobal,
            mElement: kAudioObjectPropertyElementMain
        )
        var deviceID = AudioObjectID(kAudioObjectUnknown)
        var size = UInt32(MemoryLayout<AudioObjectID>.size)
        var status = AudioObjectGetPropertyData(
            AudioObjectID(kAudioObjectSystemObject), &address, 0, nil, &size, &deviceID
        )
        guard status == noErr else { throw CoreAudioError(code: status, op: "DefaultOutputDevice") }

        address.mSelector = kAudioDevicePropertyDeviceUID
        var cfStr: Unmanaged<CFString>?
        size = UInt32(MemoryLayout<Unmanaged<CFString>>.size)
        status = withUnsafeMutablePointer(to: &cfStr) {
            AudioObjectGetPropertyData(deviceID, &address, 0, nil, &size, $0)
        }
        guard status == noErr, let cf = cfStr else {
            throw CoreAudioError(code: status, op: "OutputDeviceUID")
        }
        return cf.takeRetainedValue() as String
    }

    /// Stream format the tap delivers, read from the tap object itself.
    private func tapStreamFormat(_ tap: AudioObjectID) throws -> AudioStreamBasicDescription {
        var address = AudioObjectPropertyAddress(
            mSelector: kAudioTapPropertyFormat,
            mScope: kAudioObjectPropertyScopeGlobal,
            mElement: kAudioObjectPropertyElementMain
        )
        var asbd = AudioStreamBasicDescription()
        var size = UInt32(MemoryLayout<AudioStreamBasicDescription>.size)
        let status = AudioObjectGetPropertyData(tap, &address, 0, nil, &size, &asbd)
        guard status == noErr else { throw CoreAudioError(code: status, op: "TapFormat") }
        return asbd
    }
}

struct CoreAudioError: Error, CustomStringConvertible {
    let code: OSStatus
    let op: String
    var description: String { "CoreAudio error (\(op)): \(code)" }
}

extension AVAudioPCMBuffer {
    /// Allocate a deep copy of the buffer. AVAudioEngine reuses its tap buffer; downstream
    /// consumers running async must own their copy.
    func copy() -> AVAudioPCMBuffer? {
        guard let out = AVAudioPCMBuffer(pcmFormat: format, frameCapacity: frameCapacity) else { return nil }
        out.frameLength = frameLength
        let src = UnsafeMutableAudioBufferListPointer(mutableAudioBufferList)
        let dst = UnsafeMutableAudioBufferListPointer(out.mutableAudioBufferList)
        for i in 0..<min(src.count, dst.count) {
            let bytes = Int(min(src[i].mDataByteSize, dst[i].mDataByteSize))
            if let s = src[i].mData, let d = dst[i].mData {
                memcpy(d, s, bytes)
            }
        }
        return out
    }
}
