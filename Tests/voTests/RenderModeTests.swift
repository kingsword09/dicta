import Testing
@testable import vo

@Suite("detectRenderMode")
struct RenderModeTests {
    @Test func jsonForcedAlwaysSelectsJSONL() {
        #expect(detectRenderMode(jsonForced: true) == .jsonl)
    }
}
