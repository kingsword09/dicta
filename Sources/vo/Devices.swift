import Foundation
import CoreAudio
@preconcurrency import AVFoundation

struct InputDeviceInfo: Sendable {
    let id: UInt32
    let uid: String
    let name: String
    let channels: Int
    let isDefault: Bool
}

/// Enumerate all input-capable audio devices via Core Audio.
func collectInputDevices() throws -> [InputDeviceInfo] {
    let defaultID = try queryDefaultInputDevice()
    let allIDs = try queryAllDeviceIDs()
    var result: [InputDeviceInfo] = []
    for id in allIDs {
        guard let inputChannels = try? queryInputChannelCount(id), inputChannels > 0 else { continue }
        let name = (try? queryStringProperty(id, selector: kAudioObjectPropertyName)) ?? "(unknown)"
        let uid = (try? queryStringProperty(id, selector: kAudioDevicePropertyDeviceUID)) ?? ""
        result.append(InputDeviceInfo(
            id: UInt32(id),
            uid: uid,
            name: name,
            channels: inputChannels,
            isDefault: id == defaultID
        ))
    }
    return result
}

// MARK: - Core Audio enumeration helpers

private func queryDefaultInputDevice() throws -> AudioDeviceID {
    var address = AudioObjectPropertyAddress(
        mSelector: kAudioHardwarePropertyDefaultInputDevice,
        mScope: kAudioObjectPropertyScopeGlobal,
        mElement: kAudioObjectPropertyElementMain
    )
    var deviceID: AudioDeviceID = 0
    var size = UInt32(MemoryLayout<AudioDeviceID>.size)
    let status = AudioObjectGetPropertyData(
        AudioObjectID(kAudioObjectSystemObject),
        &address,
        0,
        nil,
        &size,
        &deviceID
    )
    guard status == noErr else { throw CoreAudioError(code: status, op: "DefaultInputDevice") }
    return deviceID
}

private func queryAllDeviceIDs() throws -> [AudioDeviceID] {
    var address = AudioObjectPropertyAddress(
        mSelector: kAudioHardwarePropertyDevices,
        mScope: kAudioObjectPropertyScopeGlobal,
        mElement: kAudioObjectPropertyElementMain
    )
    var size: UInt32 = 0
    var status = AudioObjectGetPropertyDataSize(
        AudioObjectID(kAudioObjectSystemObject),
        &address,
        0,
        nil,
        &size
    )
    guard status == noErr else { throw CoreAudioError(code: status, op: "DevicesSize") }

    let count = Int(size) / MemoryLayout<AudioDeviceID>.size
    var ids = [AudioDeviceID](repeating: 0, count: count)
    status = AudioObjectGetPropertyData(
        AudioObjectID(kAudioObjectSystemObject),
        &address,
        0,
        nil,
        &size,
        &ids
    )
    guard status == noErr else { throw CoreAudioError(code: status, op: "Devices") }
    return ids
}

private func queryInputChannelCount(_ deviceID: AudioDeviceID) throws -> Int {
    var address = AudioObjectPropertyAddress(
        mSelector: kAudioDevicePropertyStreamConfiguration,
        mScope: kAudioDevicePropertyScopeInput,
        mElement: kAudioObjectPropertyElementMain
    )
    var size: UInt32 = 0
    var status = AudioObjectGetPropertyDataSize(deviceID, &address, 0, nil, &size)
    guard status == noErr else { return 0 }

    let buffer = UnsafeMutablePointer<AudioBufferList>.allocate(capacity: 1)
    defer { buffer.deallocate() }
    status = AudioObjectGetPropertyData(deviceID, &address, 0, nil, &size, buffer)
    guard status == noErr else { return 0 }

    let abl = UnsafeMutableAudioBufferListPointer(buffer)
    var channels = 0
    for i in 0..<abl.count {
        channels += Int(abl[i].mNumberChannels)
    }
    return channels
}

private func queryStringProperty(_ deviceID: AudioDeviceID, selector: AudioObjectPropertySelector) throws -> String {
    var address = AudioObjectPropertyAddress(
        mSelector: selector,
        mScope: kAudioObjectPropertyScopeGlobal,
        mElement: kAudioObjectPropertyElementMain
    )
    var cfStr: Unmanaged<CFString>?
    var size = UInt32(MemoryLayout<Unmanaged<CFString>>.size)
    let status = withUnsafeMutablePointer(to: &cfStr) { ptr in
        AudioObjectGetPropertyData(deviceID, &address, 0, nil, &size, ptr)
    }
    guard status == noErr, let cf = cfStr else {
        throw CoreAudioError(code: status, op: "StringProperty")
    }
    return cf.takeRetainedValue() as String
}

private struct CoreAudioError: Error, CustomStringConvertible {
    let code: OSStatus
    let op: String
    var description: String { "CoreAudio error (\(op)): \(code)" }
}
