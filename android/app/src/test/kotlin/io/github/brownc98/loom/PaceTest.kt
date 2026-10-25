package io.github.brownc98.loom

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

class PaceTest {
    @Test
    fun quietLimitStartsOnlyAfterTheFirstBeat() {
        val pace = Pace()
        pace.opened()
        pace.heard(Heard.WAITS)
        pace.heard(Heard.OTHER)
        assertNull(pace.quietMs)
        pace.heard(Heard.BEAT)
        assertEquals(Pace.QUIET_MS, pace.quietMs)
        pace.heard(Heard.WAITS)
        assertEquals(Pace.QUIET_MS, pace.quietMs)
    }

    @Test
    fun aDropForgetsTheBeatUntilTheNextConnectionBeats() {
        val pace = Pace()
        pace.heard(Heard.BEAT)
        pace.dropped()
        pace.opened()
        assertNull(pace.quietMs)
    }

    @Test
    fun retryWaitsOneSecondAfterACloseAndBacksOffWhileThePcStaysAway() {
        val pace = Pace()
        pace.opened()
        assertEquals(listOf(1_000L, 2_000L, 4_000L, 8_000L, 16_000L, 32_000L, 60_000L, 60_000L), List(8) { pace.dropped() })
        pace.opened()
        assertEquals(1_000L, pace.dropped())
        pace.dropped()
        pace.restarted()
        assertEquals(1_000L, pace.dropped())
    }
}
