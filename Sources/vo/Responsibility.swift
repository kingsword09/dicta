import Darwin
import Foundation
import MachO

// Private libSystem call. When set on the spawn attributes, the spawned child
// becomes its own TCC responsible process instead of inheriting the launching
// terminal's. There is no exec-based equivalent, which is why claiming vo's own
// identity requires spawning a child rather than re-imaging the current process.
@_silgen_name("responsibility_spawnattrs_setdisclaim")
private func responsibility_spawnattrs_setdisclaim(
    _ attr: UnsafeMutablePointer<posix_spawnattr_t?>,
    _ disclaim: Int32
) -> Int32

// Holds the disclaimed child's pid so the async-signal-safe forwarder can reach
// it. Written once before the handlers are installed, then only read, so the
// unchecked access is safe.
nonisolated(unsafe) private var disclaimedChildPID: pid_t = 0

// Forward a terminating signal from the launcher to the child that owns the
// capture. `kill` is async-signal-safe, so this is safe to call from a handler.
private func forwardSignalToChild(_ sig: Int32) {
    if disclaimedChildPID > 0 {
        kill(disclaimedChildPID, sig)
    }
}

enum Responsibility {
    // Set on the spawned child so it does not spawn yet another copy.
    private static let guardKey = "VO_DISCLAIMED"

    /// Re-launch vo as its own responsible process so the Microphone / Speech /
    /// Audio Recording prompts attach to vo rather than the host terminal.
    ///
    /// On success this never returns: the launcher waits on the child and exits
    /// with its status. It returns (so the caller keeps running in-process under
    /// the terminal's identity) when already running as the disclaimed child, or
    /// when any step fails. Permission isolation is best-effort, never a gate on
    /// startup.
    static func reexecAsResponsibleProcess() {
        guard getenv(guardKey) == nil else { return }
        // A disclaimed process is its own responsible process, so it must carry the
        // usage descriptions itself. Only the release build embeds them (see
        // build.sh); re-launching a plain `swift build` binary would hit the mic
        // with no usage string and get killed, so leave such builds on the
        // terminal's identity instead. This guard is the expected path for dev
        // builds, so it stays silent.
        guard hasEmbeddedInfoPlist() else { return }
        guard let exePath = executablePath() else {
            warn("could not resolve its own path; continuing under the terminal's identity")
            return
        }

        var attr: posix_spawnattr_t?
        guard posix_spawnattr_init(&attr) == 0 else {
            warn("could not initialize spawn attributes; continuing under the terminal's identity")
            return
        }
        defer { posix_spawnattr_destroy(&attr) }
        guard responsibility_spawnattrs_setdisclaim(&attr, 1) == 0 else {
            warn("could not disclaim responsibility; continuing under the terminal's identity")
            return
        }

        let argv: [UnsafeMutablePointer<CChar>?] =
            CommandLine.arguments.map { strdup($0) } + [nil]
        let envp = environmentWithGuard()

        var pid: pid_t = 0
        let rc = posix_spawn(&pid, exePath, nil, &attr, argv, envp)
        // posix_spawn copies argv/envp into the child, so the parent's duplicates
        // are dead after the call on either outcome.
        freeCStringArray(argv)
        freeCStringArray(envp)
        guard rc == 0 else {
            warn("could not claim its own permissions (\(String(cString: strerror(rc)))); continuing under the terminal's identity")
            return
        }

        // Forward terminating signals to the child so the launcher stays a
        // transparent proxy: a SIGTERM/SIGINT/SIGQUIT aimed at this pid (kill, a
        // process manager) reaches the child that owns the capture instead of being
        // dropped while the child keeps recording. The launcher keeps running so it
        // can reap the child and report its exit status.
        disclaimedChildPID = pid
        signal(SIGINT, forwardSignalToChild)
        signal(SIGTERM, forwardSignalToChild)
        signal(SIGQUIT, forwardSignalToChild)

        var wstatus: Int32 = 0
        while waitpid(pid, &wstatus, 0) == -1 && errno == EINTR {}

        if (wstatus & 0x7f) == 0 {
            exit((wstatus >> 8) & 0xff)
        }
        exit(128 + (wstatus & 0x7f))
    }

    private static func hasEmbeddedInfoPlist() -> Bool {
        guard let header = _dyld_get_image_header(0) else { return false }
        var size: UInt = 0
        let section = header.withMemoryRebound(to: mach_header_64.self, capacity: 1) {
            getsectiondata($0, "__TEXT", "__info_plist", &size)
        }
        return section != nil && size > 0
    }

    private static func executablePath() -> String? {
        var size: UInt32 = 0
        _ = _NSGetExecutablePath(nil, &size)
        var buf = [CChar](repeating: 0, count: Int(size))
        guard _NSGetExecutablePath(&buf, &size) == 0 else { return nil }
        // Resolve symlinks (Homebrew links vo into its bin) so the spawned path is
        // the real Mach-O whose embedded Info.plist + signature TCC keys on.
        if let resolved = realpath(buf, nil) {
            defer { free(resolved) }
            return String(cString: resolved)
        }
        return buf.withUnsafeBufferPointer { String(cString: $0.baseAddress!) }
    }

    private static func environmentWithGuard() -> [UnsafeMutablePointer<CChar>?] {
        var env: [UnsafeMutablePointer<CChar>?] = []
        var cursor = environ
        while let entry = cursor.pointee {
            env.append(strdup(entry))
            cursor = cursor.advanced(by: 1)
        }
        env.append(strdup("\(guardKey)=1"))
        env.append(nil)
        return env
    }

    private static func freeCStringArray(_ array: [UnsafeMutablePointer<CChar>?]) {
        for entry in array where entry != nil { free(entry) }
    }

    private static func warn(_ message: String) {
        FileHandle.standardError.write(Data("vo: \(message).\n".utf8))
    }
}
