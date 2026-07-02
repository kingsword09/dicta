import Testing
@testable import dicta

@Suite("detectRenderMode")
struct RenderModeTests {
    @Test func jsonForcedAlwaysSelectsJSONL() {
        #expect(detectRenderMode(jsonForced: true) == .jsonl)
    }
}
