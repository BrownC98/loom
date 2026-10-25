package io.github.brownc98.loom

internal class Pace {
    private var failures = 0
    private var beating = false

    val quietMs: Long? get() = if (beating) QUIET_MS else null

    fun opened() {
        failures = 0
    }

    fun heard(heard: Heard) {
        if (heard == Heard.BEAT) beating = true
    }

    fun dropped(): Long {
        beating = false
        return minOf(RETRY_MAX_MS, RETRY_MS shl minOf(failures++, RETRY_DOUBLINGS))
    }

    fun restarted() {
        beating = false
        failures = 0
    }

    companion object {
        const val CONNECT_MS = 5_000L
        const val QUIET_MS = 40_000L
        const val RETRY_MS = 1_000L
        const val RETRY_MAX_MS = 60_000L
        private const val RETRY_DOUBLINGS = 6
    }
}
