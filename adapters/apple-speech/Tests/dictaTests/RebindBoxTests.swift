import Testing
@testable import dicta

@Suite("RebindBox")
struct RebindBoxTests {
    @Test func requestRebindThenShouldRebindIsConsumedOnce() {
        let box = RebindBox()
        #expect(box.shouldRebind() == false)
        // No current stopper registered, so it returns nil but still records the request.
        #expect(box.requestRebindAndTakeStopper() == nil)
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
        #expect(box.requestRebindAndTakeStopper() == nil)
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

    @Test func setCurrentRejectedWhenRebindRequestedDuringStartup() {
        let box = RebindBox()
        // Device-change fires before the capture registered: nothing to stop yet, so it
        // returns nil but records the request.
        #expect(box.requestRebindAndTakeStopper() == nil)
        // setCurrent must reject so the caller stops the just-started capture; otherwise
        // the capture would keep running on the old device and the request would be lost.
        #expect(box.setCurrent { } == false)
        // The request is still pending for the feeder to consume and rebuild.
        #expect(box.shouldRebind() == true)
    }

    @Test func deviceLossTakesStopperSoShutdownDoesNotDoubleStop() async {
        let box = RebindBox()
        let runs = Counter()
        #expect(box.setCurrent { await runs.bump() } == true)

        // Device-change takes the stopper and runs it...
        let stop = box.requestRebindAndTakeStopper()
        #expect(stop != nil)
        await stop?()

        // ...so a racing shutdown finds nothing to stop. The capture is stopped once,
        // which matters because the capture classes have no internal locking.
        await box.stopCurrent()
        #expect(await runs.value == 1)
    }
}

/// Minimal async-safe boolean used to observe that a stopper closure ran.
private actor Flag {
    private(set) var value = false
    func set() { value = true }
}

/// Minimal async-safe counter to assert a stopper runs exactly once.
private actor Counter {
    private(set) var value = 0
    func bump() { value += 1 }
}
