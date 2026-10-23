package io.github.brownc98.loom

import java.math.BigInteger
import java.nio.ByteBuffer
import java.nio.ByteOrder
import java.security.MessageDigest
import java.security.SecureRandom
import javax.crypto.Mac
import javax.crypto.spec.SecretKeySpec

class Pake(private val code: String, private val x: BigInteger = random()) {
    val pa: ByteArray = bytes(G.modPow(x, P).multiply(M.modPow(w(code), P)).mod(P))

    fun confirms(pb: ByteArray, door: ByteArray, confirm: ByteArray): Boolean {
        val b = parse(pb) ?: return false
        val k = b.multiply(N.modPow(w(code), P).modInverse(P)).mod(P).modPow(x, P)
        return MessageDigest.isEqual(confirmation(listOf(LABEL, pa, pb, bytes(k), wBytes(code), door)), confirm)
    }

    companion object {
        private val P = BigInteger(
            "FFFFFFFFFFFFFFFFC90FDAA22168C234C4C6628B80DC1CD129024E088A67CC74020BBEA63B139B22514A08798E3404DDEF9519B3CD3A431B302B0A6DF25F14374FE1356D6D51C245E485B576625E7EC6F44C42E9A637ED6B0BFF5CB6F406B7EDEE386BFB5A899FA5AE9F24117C4B1FE649286651ECE45B3DC2007CB8A163BF0598DA48361C55D39A69163FA8FD24CF5F83655D23DCA3AD961C62F356208552BB9ED529077096966D670C354E4ABC9804F1746C08CA18217C32905E462E36CE3BE39E772C180E86039B2783A2EC07A28FB5C55DF06F4C52C9DE2BCBF6955817183995497CEA956AE515D2261898FA051015728E5A8AACAA68FFFFFFFFFFFFFFFF",
            16,
        )
        private val Q = P.shiftRight(1)
        private val G = BigInteger.valueOf(2)
        const val ELEMENT_LEN = 256
        private val LABEL = "loom pake v1".toByteArray()
        private val M = toGroup("loom pake v1 M")
        private val N = toGroup("loom pake v1 N")

        private fun random(): BigInteger = BigInteger(1, ByteArray(ELEMENT_LEN).also { SecureRandom().nextBytes(it) }).mod(Q)

        private fun sha256(vararg parts: ByteArray): ByteArray =
            MessageDigest.getInstance("SHA-256").apply { parts.forEach(::update) }.digest()

        private fun toGroup(label: String): BigInteger {
            val wide = (0 until 9).flatMap { sha256(label.toByteArray(), byteArrayOf(it.toByte())).toList() }.toByteArray()
            return BigInteger(1, wide).mod(P).modPow(G, P)
        }

        private fun wBytes(code: String) = sha256("loom pake v1 w".toByteArray(), code.toByteArray())
        private fun w(code: String) = BigInteger(1, wBytes(code)).mod(Q)

        private fun parse(bytes: ByteArray): BigInteger? {
            if (bytes.size != ELEMENT_LEN) return null
            val e = BigInteger(1, bytes)
            return e.takeIf { it > BigInteger.ONE && it < P.subtract(BigInteger.ONE) && it.modPow(Q, P) == BigInteger.ONE }
        }

        private fun bytes(e: BigInteger): ByteArray {
            val raw = e.toByteArray().let { if (it.size > ELEMENT_LEN) it.copyOfRange(it.size - ELEMENT_LEN, it.size) else it }
            return ByteArray(ELEMENT_LEN - raw.size) + raw
        }

        private fun confirmation(parts: List<ByteArray>): ByteArray {
            val tt = MessageDigest.getInstance("SHA-256")
            for (part in parts) {
                tt.update(ByteBuffer.allocate(8).order(ByteOrder.LITTLE_ENDIAN).putLong(part.size.toLong()).array())
                tt.update(part)
            }
            return Mac.getInstance("HmacSHA256").apply { init(SecretKeySpec(tt.digest(), "HmacSHA256")) }
                .doFinal("loom pake v1 pc confirm".toByteArray())
        }
    }
}
