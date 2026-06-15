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
        #expect(box.setCurrent { await stopped.set() } == true)

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
        #expect(box.setCurrent { await first.set() } == true)
        #expect(box.setCurrent { await second.set() } == true)

        await box.stopCurrent()
        #expect(await first.value == false)
        #expect(await second.value == true)
    }

    @Test func setCurrentIsRejectedAfterStopCurrent() async {
        let box = RebindBox()
        await box.stopCurrent()

        // A rebind that finishes starting a capture after shutdown must be told no,
        // so the caller stops that capture instead of leaving it running.
        let late = Flag()
        #expect(box.setCurrent { await late.set() } == false)

        // The rejected stopper is never owned by the box, so a second stopCurrent
        // does not run it.
        await box.stopCurrent()
        #expect(await late.value == false)
    }
}

/// Minimal async-safe boolean used to observe that a stopper closure ran.
private actor Flag {
    private(set) var value = false
    func set() { value = true }
}
