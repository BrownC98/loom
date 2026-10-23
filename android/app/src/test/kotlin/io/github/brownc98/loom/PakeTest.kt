package io.github.brownc98.loom

import java.math.BigInteger
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class PakeTest {
    private val vector = javaClass.getResource("/pake-vector.txt")!!.readText().lines()
        .mapNotNull { line -> line.split('=', limit = 2).takeIf { it.size == 2 }?.let { it[0] to it[1] } }.toMap()

    private fun hex(name: String) = vector.getValue(name).chunked(2).map { it.toInt(16).toByte() }.toByteArray()

    @Test
    fun sharedVectorMatches() {
        val pake = Pake(vector.getValue("code"), BigInteger(1, hex("x")))
        assertArrayEquals(hex("pa"), pake.pa)
        val door = vector.getValue("door").toByteArray()
        assertTrue(pake.confirms(hex("pb"), door, hex("c")))
        assertFalse(pake.confirms(hex("pb"), "fp=other".toByteArray(), hex("c")))
        assertFalse(Pake("ABCD2346", BigInteger(1, hex("x"))).confirms(hex("pb"), door, hex("c")))
        assertFalse(pake.confirms(ByteArray(Pake.ELEMENT_LEN) { if (it == Pake.ELEMENT_LEN - 1) 1 else 0 }, door, hex("c")))
    }
}
