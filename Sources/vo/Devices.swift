import Foundation
import CoreAudio
@preconcurrency import AVFoundation

struct AudioDeviceInfo: Sendable {
    let id: UInt32
    let uid: String
    let name: String
    let channels: Int
    let isDefault: Bool
}

/// Direction used to read a device's channel count and resolve the system default.
enum AudioDirection {
    case input
    case output

    var scope: AudioObjectPropertyScope {
        self == .input ? kAudioDevicePropertyScopeInput : kAudioDevicePropertyScopeOutput
    }

    var defaultSelector: AudioObjectPropertySelector {
        self == .input ? kAudioHardwarePropertyDefaultInputDevice : kAudioHardwarePropertyDefaultOutputDevice
    }
}

/// Enumerate all input-capable audio devices via Core Audio.
func collectInputDevices() throws -> [AudioDeviceInfo] {
    try collectDevices(.input)
}

/// Enumerate all output-capable audio devices via Core Audio.
func collectOutputDevices() throws -> [AudioDeviceInfo] {
    try collectDevices(.output)
}

private func collectDevices(_ direction: AudioDirection) throws -> [AudioDeviceInfo] {
    let defaultID = try queryDefaultDevice(selector: direction.defaultSelector)
    let allIDs = try queryAllDeviceIDs()
    var result: [AudioDeviceInfo] = []
    for id in allIDs {
        guard let channels = try? queryChannelCount(id, scope: direction.scope), channels > 0 else { continue }
        let name = (try? queryStringProperty(id, selector: kAudioObjectPropertyName)) ?? "(unknown)"
        let uid = (try? queryStringProperty(id, selector: kAudioDevicePropertyDeviceUID)) ?? ""
        result.append(AudioDeviceInfo(
            id: UInt32(id),
            uid: uid,
            name: name,
            channels: channels,
            isDefault: id == defaultID
        ))
    }
    return result
}

// MARK: - Core Audio enumeration helpers

private func queryDefaultDevice(selector: AudioObjectPropertySelector) throws -> AudioDeviceID {
    var address = AudioObjectPropertyAddress(
        mSelector: selector,
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
    guard status == noErr else { throw CoreAudioError(code: status, op: "DefaultDevice") }
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

private func queryChannelCount(_ deviceID: AudioDeviceID, scope: AudioObjectPropertyScope) throws -> Int {
    var address = AudioObjectPropertyAddress(
        mSelector: kAudioDevicePropertyStreamConfiguration,
        mScope: scope,
        mElement: kAudioObjectPropertyElementMain
    )
    var size: UInt32 = 0
    var status = AudioObjectGetPropertyDataSize(deviceID, &address, 0, nil, &size)
    guard status == noErr, size > 0 else { return 0 }

    // AudioBufferList is variable-length; devices with multiple streams report a
    // size larger than a single AudioBufferList, so allocate the reported size
    // instead of one fixed-size element.
    let raw = UnsafeMutableRawPointer.allocate(
        byteCount: Int(size),
        alignment: MemoryLayout<AudioBufferList>.alignment
    )
    defer { raw.deallocate() }
    let listPtr = raw.bindMemory(to: AudioBufferList.self, capacity: 1)
    status = AudioObjectGetPropertyData(deviceID, &address, 0, nil, &size, listPtr)
    guard status == noErr else { return 0 }

    let abl = UnsafeMutableAudioBufferListPointer(listPtr)
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
