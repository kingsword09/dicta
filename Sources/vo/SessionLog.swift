import Foundation
#if canImport(Darwin)
import Darwin
#endif

/// Streaming JSONL log file used to persist a session without buffering events in memory.
///
/// Two modes:
///   - **Explicit**: `--log <path>` was given. We open that path directly and stream into it.
///     No temp file, no rename, no discard.
///   - **Temp**: opens a file under TMPDIR. At shutdown, the temp file is either moved to a
///     user-chosen path (via prompt) or removed.
final class SessionLog: @unchecked Sendable {
    /// Current path of the open file. For temp mode this is the temp location; for explicit
    /// mode it is the user-supplied destination.
    let path: String
    /// Path used as the default in the interactive prompt. Same as `path` for explicit mode.
    let suggestedPath: String
    /// `true` when `--log` was given. Explicit mode skips the prompt and bypasses the temp
    /// machinery entirely (no temp file ever exists).
    let isExplicit: Bool
    /// Set when `resolveSessionLog` has intentionally kept the temp file in place (e.g. the
    /// chosen move target failed and we surfaced its temp path to the user). The cleanup
    /// `defer` in `Listen.swift` checks this so the safety-net discard does not destroy the
    /// very recovery file we just promised to preserve.
    fileprivate(set) var preservedForRecovery: Bool = false

    private let fileHandle: FileHandle
    private var hasContent: Bool = false
    private var isClosed: Bool = false
    private let lock = NSLock()

    private init(path: String, suggestedPath: String, isExplicit: Bool, fileHandle: FileHandle) {
        self.path = path
        self.suggestedPath = suggestedPath
        self.isExplicit = isExplicit
        self.fileHandle = fileHandle
    }

    /// Open the session log. If `explicitPath` is non-nil we write directly there; otherwise
    /// a temp file is created and cleaned up by `resolveSessionLog` on exit.
    ///
    /// If `explicitPath` already exists and the user (in TTY mode) declines to overwrite,
    /// this prints a message and calls `Foundation.exit(0)` since there's nothing left to do.
    static func open(explicitPath: String?) throws -> SessionLog {
        let stamp = filenameStamp(Date())

        if let explicitPath {
            let resolved = (explicitPath as NSString).expandingTildeInPath
            if !confirmOverwriteIfNeeded(path: resolved) {
                print("Aborted: \(resolved) already exists.")
                Foundation.exit(0)
            }
            FileManager.default.createFile(atPath: resolved, contents: nil)
            let handle = try FileHandle(forWritingTo: URL(fileURLWithPath: resolved))
            // createFile already truncates an existing file to 0 bytes, but make the
            // overwrite-semantics explicit at the handle level so no future reader has
            // to reason about NSFileManager's documented behavior.
            try? handle.truncate(atOffset: 0)
            return SessionLog(
                path: resolved,
                suggestedPath: resolved,
                isExplicit: true,
                fileHandle: handle
            )
        }

        let tmpDir = NSTemporaryDirectory()
        let pid = ProcessInfo.processInfo.processIdentifier
        let tempPath = (tmpDir as NSString).appendingPathComponent("vo-\(stamp)-\(pid).jsonl")
        FileManager.default.createFile(atPath: tempPath, contents: nil)
        let handle = try FileHandle(forWritingTo: URL(fileURLWithPath: tempPath))
        try? handle.truncate(atOffset: 0)
        return SessionLog(
            path: tempPath,
            suggestedPath: "./vo-\(stamp).jsonl",
            isExplicit: false,
            fileHandle: handle
        )
    }

    /// Append one JSONL line (we add the trailing newline).
    func append(_ line: String) {
        lock.lock()
        defer { lock.unlock() }
        guard !isClosed else { return }
        guard let data = (line + "\n").data(using: .utf8) else { return }
        do {
            try fileHandle.write(contentsOf: data)
            hasContent = true
        } catch {
            // Swallow but do not flip hasContent so the save/preserve path won't
            // be triggered for a file that never received any bytes.
        }
    }

    func close() {
        lock.lock()
        defer { lock.unlock() }
        guard !isClosed else { return }
        try? fileHandle.synchronize()
        try? fileHandle.close()
        isClosed = true
    }

    var isEmpty: Bool {
        lock.lock(); defer { lock.unlock() }
        return !hasContent
    }

    /// Move the temp file to `destination`. Only valid in temp mode.
    func move(to destination: String) throws {
        precondition(!isExplicit, "move() is for temp mode only")
        let resolvedDst = (destination as NSString).expandingTildeInPath
        let dst = URL(fileURLWithPath: resolvedDst)
        let src = URL(fileURLWithPath: path)
        try? FileManager.default.removeItem(at: dst)
        try FileManager.default.moveItem(at: src, to: dst)
    }

    /// Delete the underlying file. Used in temp mode when the user declined to save, and
    /// also belt-and-suspenders to make sure no temp file is ever left behind.
    func discard() {
        try? FileManager.default.removeItem(atPath: path)
    }

    // MARK: - Helpers

    private static func filenameStamp(_ date: Date) -> String {
        let f = DateFormatter()
        f.dateFormat = "yyyy-MM-dd-HHmmss"
        f.locale = Locale(identifier: "en_US_POSIX")
        f.timeZone = .current
        return f.string(from: date)
    }
}

/// Decide what to do with the session log on exit and execute it.
/// Returns a short human-readable status message (or nil for silent).
func resolveSessionLog(sessionLog: SessionLog, canPrompt: Bool) -> String? {
    sessionLog.close()

    // Explicit mode: file was already written to the user's path. Nothing to move,
    // nothing to discard.
    if sessionLog.isExplicit {
        if sessionLog.isEmpty {
            // No utterances captured. Leave an empty file rather than silently delete
            // something the user explicitly named.
            return "Saved log: \(sessionLog.path) (no utterances)"
        }
        return "Saved log: \(sessionLog.path)"
    }

    // Temp mode: nothing captured, just clean up.
    if sessionLog.isEmpty {
        sessionLog.discard()
        return nil
    }

    guard canPrompt else {
        sessionLog.discard()
        return nil
    }

    // Loop so that declining an overwrite returns the user to the save prompt
    // for a different path rather than throwing the captured session away.
    while true {
        guard let target = promptForLogPath(defaultPath: sessionLog.suggestedPath) else {
            sessionLog.discard()
            return nil
        }

        if !confirmOverwriteIfNeeded(path: (target as NSString).expandingTildeInPath) {
            continue
        }

        do {
            try sessionLog.move(to: target)
            return "Saved log: \(target)"
        } catch {
            // Move failed (bad path, permission, etc.). Keep the temp file so the user
            // can recover it manually, and mark it so the Listen.swift safety-net defer
            // will not discard the file we just told the user we preserved.
            sessionLog.preservedForRecovery = true
            return "Failed to save log to \(target): \(error.localizedDescription)\n  Log preserved at: \(sessionLog.path)"
        }
    }
}

/// Prompt for overwrite confirmation if `path` already exists. Returns `true` to proceed,
/// `false` to abort. In non-TTY contexts we proceed silently (matching existing semantics
/// of file creation), since there is no human to ask.
private func confirmOverwriteIfNeeded(path: String) -> Bool {
    guard FileManager.default.fileExists(atPath: path) else { return true }
    guard canPromptForLog() else { return true }
    print("\(path) already exists. Overwrite? [y/N]: ", terminator: "")
    guard let line = readLine() else { return false }
    let trimmed = line.trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
    return trimmed == "y" || trimmed == "yes"
}

/// Interactive prompt. Returns the chosen path, or nil if the user declined.
private func promptForLogPath(defaultPath: String) -> String? {
    print("")
    print("Save log to \(defaultPath)? [Y/n/<path>]: ", terminator: "")
    guard let line = readLine() else { return nil }
    let trimmed = line.trimmingCharacters(in: .whitespacesAndNewlines)
    if trimmed.isEmpty { return defaultPath }
    switch trimmed.lowercased() {
    case "y", "yes": return defaultPath
    case "n", "no":  return nil
    default:         return trimmed
    }
}

/// `true` when both STDIN and STDOUT are attached to a terminal (so we can both prompt and read).
func canPromptForLog() -> Bool {
    return isatty(fileno(stdin)) != 0 && isatty(fileno(stdout)) != 0
}
