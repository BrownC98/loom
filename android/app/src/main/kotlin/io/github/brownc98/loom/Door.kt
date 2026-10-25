package io.github.brownc98.loom

import android.webkit.CookieManager
import java.security.MessageDigest
import java.util.Base64

internal object Prefs {
    const val FILE = "loom"
    const val ORIGIN = "origin"
    const val DOORS = "doors"
    const val PIN = "pin"
    const val NOTIFY_ASK = "notifyAsk"
    const val NOTIFY_DONE = "notifyDone"
    const val NOTIFY_ASKED = "notifyAsked"
    const val NOTIFY_SEEN = "notifySeen"
}

internal object Door {
    const val DEVICE_COOKIE = "loom_device"

    fun list(text: String?): List<String> = text?.split(',')?.filter { it.isNotEmpty() } ?: emptyList()

    fun fingerprint(der: ByteArray): String =
        Base64.getUrlEncoder().withoutPadding().encodeToString(MessageDigest.getInstance("SHA-256").digest(der))

    fun deviceCookie(door: String): String? =
        CookieManager.getInstance().getCookie("https://$door/")?.split(';')?.map { it.trim() }?.firstOrNull { it.startsWith("$DEVICE_COOKIE=") }
}
