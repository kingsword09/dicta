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
}
