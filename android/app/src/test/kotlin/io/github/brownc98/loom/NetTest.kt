package io.github.brownc98.loom

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class NetTest {
    private val wifi = "wifi"
    private val cell = "cell"
    private val online = Route(true, setOf(1))
    private val unchecked = Route(false, setOf(1))
    private val settle = Moves.SETTLE_MS

    private fun joined(network: Any = wifi, route: Route = online) = Moves().apply {
        available(network, 0)
        changed(network, route, 0)
    }

    @Test
    fun theFirstNetworkAtRegistrationIsNoMove() {
        val moves = Moves()
        assertEquals(Arrival.FIRST, moves.available(wifi, 0))
        assertFalse(moves.changed(wifi, online, 0))
        assertFalse(moves.settled(settle * 10))
    }

    @Test
    fun aNetworkThatAppearsAfterAStartWithoutNetworkIsTheFirstArrival() {
        val moves = Moves()
        assertEquals(Arrival.FIRST, moves.available(wifi, 5_000))
        assertEquals(Arrival.SAME, moves.available(wifi, 6_000))
        assertFalse(moves.settled(5_000 + settle))
    }

    @Test
    fun aNewDefaultNetworkMoves() {
        val moves = joined()
        assertEquals(Arrival.NEW, moves.available(cell, 100))
        assertFalse(moves.changed(cell, online, 100))
        assertTrue(moves.settled(100 + settle))
    }

    @Test
    fun losingTheDefaultNetworkMoves() {
        val moves = joined()
        assertTrue(moves.lost(wifi, 100))
        assertTrue(moves.settled(100 + settle))
        assertEquals(Arrival.NEW, moves.available(wifi, 200))
    }

    @Test
    fun aChangeOfValidationOrTransportMoves() {
        val moves = joined(route = unchecked)
        assertTrue(moves.changed(wifi, online, 100))
        assertTrue(moves.changed(wifi, Route(true, setOf(1, 4)), 200))
    }

    @Test
    fun anUnchangedNetworkDoesNotMove() {
        val moves = joined()
        assertEquals(Arrival.SAME, moves.available(wifi, 100))
        assertFalse(moves.changed(wifi, online, 100))
        assertFalse(moves.lost(cell, 100))
        assertFalse(moves.changed(cell, unchecked, 100))
        assertFalse(moves.settled(100 + settle))
    }

    @Test
    fun flapsSettleIntoOneReconnectAfterTheLastChange() {
        val moves = joined()
        assertTrue(moves.lost(wifi, 100))
        assertEquals(Arrival.NEW, moves.available(cell, 600))
        assertEquals(Arrival.NEW, moves.available(wifi, 1_500))
        assertFalse(moves.settled(100 + settle))
        assertFalse(moves.settled(1_500 + settle - 1))
        assertTrue(moves.settled(1_500 + settle))
        assertFalse(moves.settled(1_500 + settle * 2))
    }

    @Test
    fun theSettleTimeIsTwoSeconds() {
        assertEquals(2_000L, Moves.SETTLE_MS)
    }
}
