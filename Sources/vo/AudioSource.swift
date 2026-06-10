import Foundation
@preconcurrency import AVFoundation
@preconcurrency import ScreenCaptureKit
import CoreMedia

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

/// System audio (speaker) capture using ScreenCaptureKit.
/// Requires Screen Recording TCC permission.
final class SpeakerCapture: NSObject, @unchecked Sendable {
    let stream: AsyncStream<AVAudioPCMBuffer>
    private let builder: AsyncStream<AVAudioPCMBuffer>.Continuation
    private var scStream: SCStream?

    override init() {
        let (s, b) = AsyncStream<AVAudioPCMBuffer>.makeStream()
        self.stream = s
        self.builder = b
        super.init()
    }

    func start() async throws {
        let content = try await SCShareableContent.excludingDesktopWindows(false, onScreenWindowsOnly: true)
        guard let display = content.displays.first else {
            throw VoError.noDisplayForScreenCapture
        }
        let filter = SCContentFilter(display: display, excludingApplications: [], exceptingWindows: [])

        let config = SCStreamConfiguration()
        config.capturesAudio = true
        config.excludesCurrentProcessAudio = true
        config.sampleRate = 48000
        config.channelCount = 2
        // Video is mandatory but we make it as cheap as possible.
        config.width = 2
        config.height = 2
        config.minimumFrameInterval = CMTime(value: 1, timescale: 1)
        config.queueDepth = 6

        let s = SCStream(filter: filter, configuration: config, delegate: nil)
        try s.addStreamOutput(self, type: .audio, sampleHandlerQueue: DispatchQueue(label: "vo.spk.audio"))
        try await s.startCapture()
        self.scStream = s
    }

    func stop() async {
        if let s = scStream {
            try? await s.stopCapture()
            scStream = nil
        }
        builder.finish()
    }
}

extension SpeakerCapture: SCStreamOutput {
    func stream(_ stream: SCStream, didOutputSampleBuffer sampleBuffer: CMSampleBuffer, of type: SCStreamOutputType) {
        guard type == .audio else { return }
        guard let pcm = sampleBuffer.asPCMBuffer() else { return }
        builder.yield(pcm)
    }
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

extension CMSampleBuffer {
    /// Materialize the sample buffer as an AVAudioPCMBuffer by copying the audio data.
    func asPCMBuffer() -> AVAudioPCMBuffer? {
        guard let formatDesc = CMSampleBufferGetFormatDescription(self),
              let asbdPtr = CMAudioFormatDescriptionGetStreamBasicDescription(formatDesc) else {
            return nil
        }
        var asbd = asbdPtr.pointee
        guard let format = AVAudioFormat(streamDescription: &asbd) else { return nil }

        let frameCount = AVAudioFrameCount(CMSampleBufferGetNumSamples(self))
        guard let buffer = AVAudioPCMBuffer(pcmFormat: format, frameCapacity: frameCount) else { return nil }
        buffer.frameLength = frameCount

        let ablSize = MemoryLayout<AudioBufferList>.size
            + MemoryLayout<AudioBuffer>.size * (Int(format.channelCount) - 1)
        let rawPtr = UnsafeMutableRawPointer.allocate(
            byteCount: ablSize,
            alignment: MemoryLayout<AudioBufferList>.alignment
        )
        defer { rawPtr.deallocate() }
        let ablPtr = rawPtr.bindMemory(to: AudioBufferList.self, capacity: 1)

        var blockBuffer: CMBlockBuffer?
        let status = CMSampleBufferGetAudioBufferListWithRetainedBlockBuffer(
            self,
            bufferListSizeNeededOut: nil,
            bufferListOut: ablPtr,
            bufferListSize: ablSize,
            blockBufferAllocator: kCFAllocatorDefault,
            blockBufferMemoryAllocator: kCFAllocatorDefault,
            flags: kCMSampleBufferFlag_AudioBufferList_Assure16ByteAlignment,
            blockBufferOut: &blockBuffer
        )
        guard status == noErr else { return nil }

        // Copy from the source ABL into the freshly allocated PCM buffer. The ABL's
        // mData pointers point into blockBuffer, and ARC is free to release it after
        // its last use above, so pin it for the duration of the copy.
        withExtendedLifetime(blockBuffer) {
            let srcList = UnsafeMutableAudioBufferListPointer(ablPtr)
            let dstList = UnsafeMutableAudioBufferListPointer(buffer.mutableAudioBufferList)
            for i in 0..<min(srcList.count, dstList.count) {
                let copyBytes = Int(min(srcList[i].mDataByteSize, dstList[i].mDataByteSize))
                if let s = srcList[i].mData, let d = dstList[i].mData {
                    memcpy(d, s, copyBytes)
                }
            }
        }
        return buffer
    }
}
