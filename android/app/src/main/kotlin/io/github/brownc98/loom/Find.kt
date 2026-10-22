package io.github.brownc98.loom

import java.net.DatagramPacket
import java.net.DatagramSocket
import java.net.InetAddress
import java.net.NetworkInterface
import java.net.SocketTimeoutException
import java.util.Base64
import org.json.JSONObject

/**
 * 같은 Wi-Fi에서 짝짓기 코드로 PC를 찾는다(src/daemon/lan.rs `answer_finds`). `pA`를 UDP로 모든 브로드캐스트 주소에 보내고,
 * 확인값이 맞는 답만 받는다. 답은 폰에 바로 오므로(유니캐스트) 멀티캐스트 잠금과 권한이 없어도 된다.
 * 근거: docs/design.md 「Android 앱」(사람 결정 Q426 B).
 */
object Find {
    /** src/daemon/lan.rs `FIND_PORT`. */
    private const val PORT = 7430
    /** UDP는 잃을 수 있다. 같은 `pA`를 이만큼 보낸다. 데몬은 같은 `pA`를 한 번만 센다. */
    private const val ROUNDS = 3
    /** 한 번 보낸 뒤 답을 기다리는 시간. 같은 Wi-Fi의 왕복과 데몬의 큰 수 계산(수십 ms)보다 넉넉하다. */
    private const val WAIT_MS = 1500
    /** 한 번 찾을 때 확인해 보는 답의 수. 가짜 PC가 답을 많이 보내 코드를 맞혀 보는 횟수를 막는다. */
    private const val MAX_REPLIES = 16
    private const val MAX_REPLY_BYTES = 2048

    sealed interface Result
    /** 코드로 확인한 폰 문 지문과 주소. 답이 온 주소가 맨 앞이다. */
    class Found(val fp: String, val doors: List<String>) : Result
    /** PC가 답했지만 확인값이 맞지 않았다: 코드가 틀렸거나 가운데에 다른 기기가 있다. */
    data object Wrong : Result
    /** 아무도 답하지 않았다: 같은 Wi-Fi가 아니거나, 코드가 열려 있지 않거나, 방화벽이 막았다. */
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

    /** QR의 `#` 뒤와 찾기 답의 `door`: `pair=<코드>&fp=<지문>&a=<IP:포트>,…`(src/daemon/lan.rs `Opened::door`). */
    fun params(text: String): Map<String, String> =
        text.split('&').mapNotNull { part -> part.split('=', limit = 2).takeIf { it.size == 2 }?.let { it[0] to it[1] } }.toMap()

    fun doors(params: Map<String, String>): List<String> = params["a"]?.split(',')?.filter { it.isNotEmpty() }.orEmpty()

    /** 모든 브로드캐스트 주소. 어떤 공유기는 255.255.255.255를 넘기지 않아 망마다의 주소(예: 192.168.0.255)에도 보낸다. */
    private fun targets(): List<InetAddress> {
        val nets = runCatching { NetworkInterface.getNetworkInterfaces().toList() }.getOrDefault(emptyList())
            .filter { runCatching { it.isUp && !it.isLoopback }.getOrDefault(false) }
            .flatMap { net -> net.interfaceAddresses.mapNotNull { it.broadcast } }
        return (listOf(InetAddress.getByName("255.255.255.255")) + nets).distinct()
    }

    private fun b64(bytes: ByteArray) = Base64.getUrlEncoder().withoutPadding().encodeToString(bytes)
    private fun unb64(text: String) = runCatching { Base64.getUrlDecoder().decode(text) }.getOrNull()
}
