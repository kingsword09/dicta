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

/// Full identifiers of all Translation languages available on this device.
func collectTranslationLanguages() async -> [String] {
    let availability = LanguageAvailability()
    let supported = await availability.supportedLanguages
    return Set(supported.map { fullLanguageIdentifier($0) }).sorted()
}

private func fullLanguageIdentifier(_ lang: Locale.Language) -> String {
    var parts: [String] = []
    if let code = lang.languageCode?.identifier { parts.append(code) }
    if let script = lang.script?.identifier { parts.append(script) }
    if let region = lang.region?.identifier { parts.append(region) }
    return parts.isEmpty ? "?" : parts.joined(separator: "-")
}
