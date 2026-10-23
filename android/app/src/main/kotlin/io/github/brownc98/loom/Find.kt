package io.github.brownc98.loom

import java.net.DatagramPacket
import java.net.DatagramSocket
import java.net.InetAddress
import java.net.NetworkInterface
import java.net.SocketTimeoutException
import java.util.Base64
import org.json.JSONObject

object Find {
    private const val PORT = 7430
    private const val ROUNDS = 3
    private const val WAIT_MS = 1500
    private const val MAX_REPLIES = 16
    private const val MAX_REPLY_BYTES = 2048

    sealed interface Result
    class Found(val fp: String, val doors: List<String>) : Result
    data object Wrong : Result
    data object Missing : Result

    fun run(code: String): Result {
        val pake = Pake(code)
        val ask = JSONObject().put("pa", b64(pake.pa)).toString().toByteArray()
        var heard = 0
        DatagramSocket().use { socket ->
            socket.broadcast = true
            socket.soTimeout = WAIT_MS
            repeat(ROUNDS) {
                for (to in targets()) runCatching { socket.send(DatagramPacket(ask, ask.size, to, PORT)) }
                while (heard < MAX_REPLIES) {
                    val packet = DatagramPacket(ByteArray(MAX_REPLY_BYTES), MAX_REPLY_BYTES)
                    try {
                        socket.receive(packet)
                    } catch (_: SocketTimeoutException) {
                        break
                    }
                    heard++
                    found(pake, packet)?.let { return it }
                }
            }
        }
        return if (heard > 0) Wrong else Missing
    }

    private fun found(pake: Pake, packet: DatagramPacket): Found? {
        val reply = runCatching { JSONObject(String(packet.data, 0, packet.length)) }.getOrNull() ?: return null
        val door = reply.optString("door")
        val pb = unb64(reply.optString("pb")) ?: return null
        val c = unb64(reply.optString("c")) ?: return null
        if (!pake.confirms(pb, door.toByteArray(), c)) return null
        val params = params(door)
        val fp = params["fp"] ?: return null
        val listed = doors(params)
        val port = listed.firstOrNull()?.substringAfterLast(':') ?: return null
        val from = "${packet.address.hostAddress}:$port"
        return Found(fp, (listOf(from) + listed).distinct())
    }

    fun params(text: String): Map<String, String> =
        text.split('&').mapNotNull { part -> part.split('=', limit = 2).takeIf { it.size == 2 }?.let { it[0] to it[1] } }.toMap()

    fun doors(params: Map<String, String>): List<String> = params["a"]?.split(',')?.filter { it.isNotEmpty() }.orEmpty()

    private fun targets(): List<InetAddress> {
        val nets = runCatching { NetworkInterface.getNetworkInterfaces().toList() }.getOrDefault(emptyList())
            .filter { runCatching { it.isUp && !it.isLoopback }.getOrDefault(false) }
            .flatMap { net -> net.interfaceAddresses.mapNotNull { it.broadcast } }
        return (listOf(InetAddress.getByName("255.255.255.255")) + nets).distinct()
    }

    private fun b64(bytes: ByteArray) = Base64.getUrlEncoder().withoutPadding().encodeToString(bytes)
    private fun unb64(text: String) = runCatching { Base64.getUrlDecoder().decode(text) }.getOrNull()
}
