import Testing
@testable import vo

@Suite("RebindBox")
struct RebindBoxTests {
    @Test func requestThenShouldRebindIsConsumedOnce() {
        let box = RebindBox()
        #expect(box.shouldRebind() == false)
        box.request()
        #expect(box.shouldRebind() == true)
        // Consumed: a second check without a new request must not rebind again.
        #expect(box.shouldRebind() == false)
    }

    @Test func stopCurrentRunsTheActiveStopperAndBlocksFurtherRebinds() async {
        let box = RebindBox()
        let stopped = Flag()
        box.setCurrent { await stopped.set() }

        await box.stopCurrent()
        #expect(await stopped.value == true)

        // After shutdown a late device-change event must not resurrect the channel.
        box.request()
        #expect(box.shouldRebind() == false)
    }

    @Test func setCurrentReplacesTheStopperAcrossRebinds() async {
        let box = RebindBox()
        let first = Flag()
        let second = Flag()
        box.setCurrent { await first.set() }
        box.setCurrent { await second.set() }

        await box.stopCurrent()
        #expect(await first.value == false)
        #expect(await second.value == true)
    }
}

/// Minimal async-safe boolean used to observe that a stopper closure ran.
private actor Flag {
    private(set) var value = false
    func set() { value = true }
}
