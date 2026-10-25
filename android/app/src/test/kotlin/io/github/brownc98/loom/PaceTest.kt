package io.github.brownc98.loom

import org.junit.Assert.assertEquals
import org.junit.Test

class PaceTest {
    @Test
    fun quietLimitIsTenMinutesUntilTheFirstBeatThenFortySeconds() {
        val pace = Pace()
        pace.opened()
        assertEquals(600_000L, pace.quietMs)
        pace.heard(Heard.WAITS)
        pace.heard(Heard.OTHER)
        assertEquals(600_000L, pace.quietMs)
        pace.heard(Heard.BEAT)
        assertEquals(40_000L, pace.quietMs)
        pace.heard(Heard.WAITS)
        assertEquals(40_000L, pace.quietMs)
    }

    @Test
    fun aDropOrRestartForgetsTheBeatUntilTheNextConnectionBeats() {
        val pace = Pace()
        pace.heard(Heard.BEAT)
        pace.dropped()
        pace.opened()
        assertEquals(600_000L, pace.quietMs)
        pace.heard(Heard.BEAT)
        pace.restarted()
        assertEquals(600_000L, pace.quietMs)
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
