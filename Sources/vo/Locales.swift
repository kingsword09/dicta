import Foundation
import Speech
import Translation

struct SpeechLocaleInfo: Sendable {
    let identifier: String   // BCP-47, e.g. "en-US"
    let installed: Bool
}

/// All SpeechTranscriber locales (sorted), annotated with installation status.
func collectSpeechLocales() async -> [SpeechLocaleInfo] {
    let supported = await SpeechTranscriber.supportedLocales
    let installed = Set(
        (await SpeechTranscriber.installedLocales).map { $0.identifier(.bcp47) }
    )
    return supported
        .map { l -> SpeechLocaleInfo in
            let id = l.identifier(.bcp47)
            return SpeechLocaleInfo(identifier: id, installed: installed.contains(id))
        }
        .sorted { $0.identifier < $1.identifier }
}

/// BCP-47 identifiers of all Translation languages available on this device,
/// e.g. "ja-JP". A language code is the minimum BCP-47 unit, so entries
/// without one are dropped rather than emitted as a placeholder.
func collectTranslationLanguages() async -> [String] {
    let availability = LanguageAvailability()
    let supported = await availability.supportedLanguages
    return Set(supported.compactMap { languageRegionIdentifier($0) }).sorted()
}

// Script subtag is dropped so the output matches the `--src` / `--dst` option
// format and the Speech locale list. Returns nil without a language code, since
// a region alone is not a usable identifier.
private func languageRegionIdentifier(_ lang: Locale.Language) -> String? {
    guard let code = lang.languageCode?.identifier else { return nil }
    if let region = lang.region?.identifier { return "\(code)-\(region)" }
    return code
}
