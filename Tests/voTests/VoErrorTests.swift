import Foundation
import Testing
@testable import vo

@Suite("VoError messages")
struct VoErrorTests {
    /// An unsupported locale whose language has supported regional variants should
    /// suggest exactly those variants and nothing from other languages.
    @Test func unsupportedLocaleSuggestsRegionalVariants() {
        let err = VoError.unsupportedSpeechLocale(
            Locale(identifier: "en"),
            supported: ["en-US", "en-GB", "ja-JP"]
        )
        let msg = err.description

        #expect(msg.contains("regional variants"))
        #expect(msg.contains("en-US"))
        #expect(msg.contains("en-GB"))
        #expect(!msg.contains("ja-JP"))
    }

    /// When no regional variant matches, fall back to pointing at `--doctor`.
    @Test func unsupportedLocaleWithoutNearbyPointsToDoctor() {
        let err = VoError.unsupportedSpeechLocale(
            Locale(identifier: "xx"),
            supported: ["en-US", "ja-JP"]
        )
        let msg = err.description

        #expect(msg.contains("--doctor"))
        #expect(!msg.contains("regional variants"))
    }

    /// A supported-but-not-downloaded translation pair names both locales and the
    /// System Settings path the user must follow, since a CLI can't trigger the download.
    @Test func translationModelNotInstalledNamesPairAndInstallPath() {
        let err = VoError.translationModelNotInstalled(
            source: Locale(identifier: "en-US"),
            target: Locale(identifier: "ja-JP")
        )
        let msg = err.description

        #expect(msg.contains("en-US"))
        #expect(msg.contains("ja-JP"))
        #expect(msg.contains("System Settings"))
    }

    /// An unsupported translation pair names both locales and points at `--doctor`.
    @Test func unsupportedTranslationPairPointsToDoctor() {
        let err = VoError.unsupportedTranslationPair(
            source: Locale(identifier: "en-US"),
            target: Locale(identifier: "xx")
        )
        let msg = err.description

        #expect(msg.contains("en-US"))
        #expect(msg.contains("not supported"))
        #expect(msg.contains("--doctor"))
    }

    /// A failure to open `--input` includes the supplied path and the underlying
    /// error message, so a user can tell from stderr which file was wrong.
    @Test func inputFileOpenFailedIncludesPathAndUnderlying() {
        let url = URL(fileURLWithPath: "/tmp/vo-test-missing.wav")
        let underlying = NSError(
            domain: NSCocoaErrorDomain,
            code: NSFileReadNoSuchFileError,
            userInfo: [NSLocalizedDescriptionKey: "No such file"]
        )
        let err = VoError.inputFileOpenFailed(url: url, underlying: underlying)
        let msg = err.description

        #expect(msg.contains("Could not open"))
        #expect(msg.contains("/tmp/vo-test-missing.wav"))
        #expect(msg.contains("No such file"))
    }

    /// A mid-stream read failure distinguishes itself from the open-time error and
    /// hints at the likely causes (corruption, disconnect) so a user does not
    /// confuse it with a missing-file case.
    @Test func inputFileReadFailedNamesCorruptionAndPath() {
        let url = URL(fileURLWithPath: "/tmp/vo-test-truncated.wav")
        let underlying = NSError(
            domain: NSOSStatusErrorDomain,
            code: -50,
            userInfo: [NSLocalizedDescriptionKey: "I/O error"]
        )
        let err = VoError.inputFileReadFailed(url: url, underlying: underlying)
        let msg = err.description

        #expect(msg.contains("Read failure"))
        #expect(msg.contains("/tmp/vo-test-truncated.wav"))
        #expect(msg.contains("corrupt"))
        #expect(msg.contains("I/O error"))
    }
}
