import CoreAudio

/// Read a fixed-size Core Audio property into `value`. The caller owns the `op` label so the
/// surfaced `CoreAudioError` keeps the same wording each existing call site produced.
func audioObjectProperty<T>(
    _ objectID: AudioObjectID,
    _ address: AudioObjectPropertyAddress,
    into value: inout T,
    op: String
) throws {
    var address = address
    var size = UInt32(MemoryLayout<T>.size)
    let status = withUnsafeMutablePointer(to: &value) { ptr in
        AudioObjectGetPropertyData(objectID, &address, 0, nil, &size, ptr)
    }
    guard status == noErr else { throw CoreAudioError(code: status, op: op) }
}

/// Read a CFString-typed Core Audio property. The framework hands back a +1 retained CFString,
/// so `takeRetainedValue()` (not `takeUnretainedValue()`) balances the reference.
func audioObjectString(
    _ objectID: AudioObjectID,
    _ address: AudioObjectPropertyAddress,
    op: String
) throws -> String {
    var address = address
    var cfStr: Unmanaged<CFString>?
    var size = UInt32(MemoryLayout<Unmanaged<CFString>>.size)
    let status = withUnsafeMutablePointer(to: &cfStr) { ptr in
        AudioObjectGetPropertyData(objectID, &address, 0, nil, &size, ptr)
    }
    guard status == noErr, let cf = cfStr else {
        throw CoreAudioError(code: status, op: op)
    }
    return cf.takeRetainedValue() as String
}

/// Build a global-scope, main-element property address for `selector`.
func globalPropertyAddress(_ selector: AudioObjectPropertySelector) -> AudioObjectPropertyAddress {
    AudioObjectPropertyAddress(
        mSelector: selector,
        mScope: kAudioObjectPropertyScopeGlobal,
        mElement: kAudioObjectPropertyElementMain
    )
}
