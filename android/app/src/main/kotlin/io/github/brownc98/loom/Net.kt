package io.github.brownc98.loom

import android.content.Context
import android.net.ConnectivityManager
import android.net.Network
import android.net.NetworkCapabilities
import android.os.Handler
import android.os.Looper
import android.os.SystemClock

internal data class Route(val validated: Boolean, val transports: Set<Int>)

internal class Moves {
    private var known = false
    private var network: Any? = null
    private var route: Route? = null
    private var movedAt: Long? = null

    fun available(next: Any, now: Long): Boolean {
        if (known && next == network) return false
        val first = !known
        known = true
        network = next
        route = null
        return !first && moved(now)
    }

    fun lost(gone: Any, now: Long): Boolean {
        if (gone != network) return false
        network = null
        route = null
        return moved(now)
    }

    fun changed(at: Any, next: Route, now: Long): Boolean {
        if (at != network) return false
        val before = route
        route = next
        return before != null && before != next && moved(now)
    }

    fun settled(now: Long): Boolean {
        val at = movedAt ?: return false
        if (now - at < SETTLE_MS) return false
        movedAt = null
        return true
    }

    private fun moved(now: Long): Boolean {
        movedAt = now
        return true
    }

    companion object {
        const val SETTLE_MS = 2_000L
    }
}

internal class NetWatch(context: Context, private val moved: () -> Unit) {
    private val main = Handler(Looper.getMainLooper())
    private val moves = Moves()
    private val connectivity = context.getSystemService(ConnectivityManager::class.java)
    private val check = Runnable { if (moves.settled(SystemClock.elapsedRealtime())) moved() }
    private val callback = object : ConnectivityManager.NetworkCallback() {
        override fun onAvailable(network: Network) = on { moves.available(network, it) }

        override fun onLost(network: Network) = on { moves.lost(network, it) }

        override fun onCapabilitiesChanged(network: Network, caps: NetworkCapabilities) = on {
            moves.changed(network, Route(caps.hasCapability(NetworkCapabilities.NET_CAPABILITY_VALIDATED), TRANSPORTS.filter(caps::hasTransport).toSet()), it)
        }
    }

    fun start() = connectivity.registerDefaultNetworkCallback(callback, main)

    fun stop() {
        connectivity.unregisterNetworkCallback(callback)
        main.removeCallbacks(check)
    }

    private fun on(event: (Long) -> Boolean) {
        if (!event(SystemClock.elapsedRealtime())) return
        main.removeCallbacks(check)
        main.postDelayed(check, Moves.SETTLE_MS)
    }

    companion object {
        private val TRANSPORTS = listOf(
            NetworkCapabilities.TRANSPORT_WIFI,
            NetworkCapabilities.TRANSPORT_CELLULAR,
            NetworkCapabilities.TRANSPORT_ETHERNET,
            NetworkCapabilities.TRANSPORT_VPN,
        )
    }
}
