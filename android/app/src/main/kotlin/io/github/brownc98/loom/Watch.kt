package io.github.brownc98.loom

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.SharedPreferences
import android.content.pm.ServiceInfo
import android.os.Build
import android.os.Handler
import android.os.IBinder
import android.os.Looper
import android.os.SystemClock
import java.io.IOException
import java.security.SecureRandom
import java.security.cert.CertificateException
import java.security.cert.X509Certificate
import java.util.concurrent.TimeUnit
import javax.net.ssl.SSLContext
import javax.net.ssl.X509TrustManager
import okhttp3.Call
import okhttp3.Callback
import okhttp3.OkHttpClient
import okhttp3.Request
import okhttp3.Response
import okhttp3.WebSocket
import okhttp3.WebSocketListener
import org.json.JSONObject

class Watch : Service() {
    private val main = Handler(Looper.getMainLooper())
    private val prefs by lazy { getSharedPreferences(Prefs.FILE, MODE_PRIVATE) }
    private val manager by lazy { getSystemService(NotificationManager::class.java) }
    private val again = Runnable { connect() }
    private val quiet = Runnable { dropped() }
    private var socket: WebSocket? = null
    private var nodes: (() -> Call)? = null
    private var reading: Call? = null
    private var stale = false
    private var waits: List<Wait>? = null
    private var limit: Long? = CONNECT_MS
    private var turn = 0
    private var at = 0
    private var retryUntil: Long? = null
    private var linked = false
    private var stopped = false
    private val net by lazy { NetWatch(this, appeared = { if (!stopped && !linked && socket == null) restart() }, moved = { if (!stopped) restart() }) }

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onCreate() {
        super.onCreate()
        channels(this)
        foreground()
        net.start()
    }

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        foreground()
        show()
        if (!linked) restart()
        return START_STICKY
    }

    override fun onDestroy() {
        stopped = true
        net.stop()
        drop()
        main.removeCallbacksAndMessages(null)
        manager.activeNotifications.filter { it.tag == WAIT_TAG }.forEach { manager.cancel(WAIT_TAG, it.id) }
        super.onDestroy()
    }

    private fun foreground() {
        val text = getString(if (linked) R.string.notify_linked else R.string.notify_unlinked)
        val note = Notification.Builder(this, LINK_CHANNEL)
            .setSmallIcon(R.drawable.ic_notify)
            .setContentTitle(text)
            .setOnlyAlertOnce(true)
            .setShowWhen(false)
            .setContentIntent(opener(null))
            .build()
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.UPSIDE_DOWN_CAKE) {
            startForeground(LINK_ID, note, ServiceInfo.FOREGROUND_SERVICE_TYPE_REMOTE_MESSAGING)
        } else {
            startForeground(LINK_ID, note)
        }
    }

    private fun connect() {
        if (stopped) return
        val origin = prefs.getString(Prefs.ORIGIN, null)?.removePrefix("https://") ?: return stopSelf()
        val doors = Door.list(prefs.getString(Prefs.DOORS, null))
        val hosts = (listOf(origin) + doors).distinct()
        val host = hosts[at % hosts.size]
        val pin = prefs.getString(Prefs.PIN, null)?.takeIf { host in doors }
        val cookie = Door.deviceCookie(host) ?: doors.firstNotNullOfOrNull(Door::deviceCookie)
        fun at(path: String) = Request.Builder().url(path).apply { cookie?.let { header("Cookie", it) } }.build()
        val http = client(pin)
        nodes = { http.newCall(at("https://$host/api/nodes")) }
        val mine = ++turn
        socket = http.newWebSocket(at("wss://$host/api/events"), object : WebSocketListener() {
            override fun onOpen(webSocket: WebSocket, response: Response) {
                webSocket.send(BACK_FRAME)
                main.post {
                    if (mine != turn) return@post
                    linked(true)
                    awaitFrame()
                }
            }

            override fun onMessage(webSocket: WebSocket, text: String) {
                main.post { if (mine == turn) heard(Waits.heard(text), text) }
            }

            override fun onClosing(webSocket: WebSocket, code: Int, reason: String) {
                webSocket.close(code, null)
                main.post { if (mine == turn) dropped() }
            }

            override fun onFailure(webSocket: WebSocket, t: Throwable, response: Response?) {
                main.post { if (mine == turn) dropped() }
            }
        })
    }

    private fun heard(heard: Heard, text: String) {
        if (heard == Heard.BEAT) limit = runCatching { JSONObject(text).getLong("quietMs") }.getOrNull()
        awaitFrame()
        if (heard == Heard.NODES) read()
    }

    private fun read() {
        if (reading != null) return run { stale = true }
        val call = nodes?.invoke() ?: return
        reading = call
        call.enqueue(object : Callback {
            override fun onResponse(call: Call, response: Response) {
                val read = runCatching { response.use { if (it.isSuccessful) Waits.read(it.body.string()) else null } }.getOrNull()
                main.post { answered(call, read) }
            }

            override fun onFailure(call: Call, e: IOException) {
                main.post { answered(call, null) }
            }
        })
    }

    private fun answered(call: Call, read: List<Wait>?) {
        if (call !== reading) return
        reading = null
        if (stale) {
            stale = false
            return read()
        }
        if (read == null) return dropped()
        retryUntil = null
        waits = read
        show()
    }

    private fun awaitFrame() {
        main.removeCallbacks(quiet)
        limit?.let { main.postDelayed(quiet, it) }
    }

    private fun linked(on: Boolean) {
        if (linked == on) return
        linked = on
        foreground()
    }

    private fun drop() {
        turn++
        main.removeCallbacks(again)
        main.removeCallbacks(quiet)
        socket?.cancel()
        socket = null
        reading?.cancel()
        reading = null
        stale = false
        limit = CONNECT_MS
    }

    private fun dropped() {
        drop()
        if (!linked) at++
        linked(false)
        val now = SystemClock.elapsedRealtime()
        val until = retryUntil ?: (now + RETRY_FOR_MS).also { retryUntil = it }
        if (now < until) main.postDelayed(again, RETRY_MS)
    }

    private fun restart() {
        drop()
        retryUntil = SystemClock.elapsedRealtime() + RETRY_FOR_MS
        connect()
    }

    private fun show() {
        val waits = waits ?: return
        val shown = manager.activeNotifications.filter { it.tag == WAIT_TAG }.map { it.id.toLong() }.toSet()
        val seen = prefs.getString(Prefs.NOTIFY_SEEN, null)?.let { text -> text.split(',').mapNotNull { it.toLongOrNull() }.toSet() }
        val plan = Waits.plan(waits, seen, shown, switches(prefs), front)
        plan.cancel.forEach { manager.cancel(WAIT_TAG, it.toInt()) }
        plan.post.forEach(::post)
        prefs.edit().putString(Prefs.NOTIFY_SEEN, plan.seen.joinToString(",")).apply()
    }

    private fun post(wait: Wait) {
        val note = Notification.Builder(this, if (wait.tone == Tone.DONE) DONE_CHANNEL else ASK_CHANNEL)
            .setSmallIcon(R.drawable.ic_notify)
            .setContentTitle(wait.title(getString(LABELS[wait.kind] ?: R.string.wait_question)))
            .setContentText(wait.summary)
            .setStyle(Notification.BigTextStyle().bigText(wait.summary))
            .setAutoCancel(true)
            .setContentIntent(opener(wait.node))
            .build()
        manager.notify(WAIT_TAG, wait.id.toInt(), note)
    }

    private fun opener(node: Long?): PendingIntent {
        val intent = Intent(this, MainActivity::class.java)
            .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_SINGLE_TOP or Intent.FLAG_ACTIVITY_CLEAR_TOP)
        node?.let { intent.putExtra(EXTRA_NODE, it) }
        return PendingIntent.getActivity(this, node?.toInt() ?: -1, intent, PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT)
    }

    private fun client(pin: String?): OkHttpClient {
        if (pin == null) return plain
        val trust = PinTrust(pin)
        val tls = SSLContext.getInstance("TLS").apply { init(null, arrayOf(trust), SecureRandom()) }
        return plain.newBuilder()
            .sslSocketFactory(tls.socketFactory, trust)
            .hostnameVerifier { _, session -> runCatching { Door.fingerprint(session.peerCertificates[0].encoded) == pin }.getOrDefault(false) }
            .build()
    }

    private class PinTrust(private val pin: String) : X509TrustManager {
        override fun checkClientTrusted(chain: Array<X509Certificate>, authType: String) = throw CertificateException("client certificates are not used")

        override fun checkServerTrusted(chain: Array<X509Certificate>, authType: String) {
            if (chain.isEmpty() || Door.fingerprint(chain[0].encoded) != pin) throw CertificateException("certificate does not match the paired PC")
        }

        override fun getAcceptedIssuers(): Array<X509Certificate> = emptyArray()
    }

    class Boot : BroadcastReceiver() {
        override fun onReceive(context: Context, intent: Intent) {
            if (intent.action == Intent.ACTION_BOOT_COMPLETED || intent.action == Intent.ACTION_MY_PACKAGE_REPLACED) sync(context)
        }
    }

    companion object {
        const val EXTRA_NODE = "node"
        private const val WAIT_TAG = "wait"
        private const val BACK_FRAME = "{\"pace\":\"back\"}"
        private const val CONNECT_MS = 5_000L
        private const val RETRY_MS = 1_000L
        private const val RETRY_FOR_MS = 60_000L
        private const val LINK_ID = 1
        private const val ASK_CHANNEL = "ask"
        private const val DONE_CHANNEL = "done"
        const val LINK_CHANNEL = "link"
        private val LABELS = mapOf(
            "question" to R.string.wait_question,
            "approval" to R.string.wait_approval,
            "trust" to R.string.wait_trust,
            "auth" to R.string.wait_auth,
            "input" to R.string.wait_input,
            "done" to R.string.wait_done,
        )
        private val plain by lazy { OkHttpClient.Builder().callTimeout(CONNECT_MS, TimeUnit.MILLISECONDS).build() }

        @Volatile
        var front = false

        fun allowed(context: Context) = context.getSystemService(NotificationManager::class.java).areNotificationsEnabled()

        internal fun switches(prefs: SharedPreferences) = Switches(prefs.getBoolean(Prefs.NOTIFY_ASK, true), prefs.getBoolean(Prefs.NOTIFY_DONE, true))

        internal fun store(prefs: SharedPreferences, switches: Switches) {
            prefs.edit().putBoolean(Prefs.NOTIFY_ASK, switches.ask).putBoolean(Prefs.NOTIFY_DONE, switches.done).apply()
        }

        fun sync(context: Context) {
            val prefs = context.getSharedPreferences(Prefs.FILE, MODE_PRIVATE)
            val intent = Intent(context, Watch::class.java)
            if (switches(prefs).any && allowed(context) && prefs.getString(Prefs.ORIGIN, null) != null) {
                runCatching { context.startForegroundService(intent) }
            } else {
                context.stopService(intent)
            }
        }

        private fun channels(context: Context) {
            val manager = context.getSystemService(NotificationManager::class.java)
            manager.createNotificationChannels(listOf(
                NotificationChannel(ASK_CHANNEL, context.getString(R.string.notify_channel_ask), NotificationManager.IMPORTANCE_HIGH),
                NotificationChannel(DONE_CHANNEL, context.getString(R.string.notify_channel_done), NotificationManager.IMPORTANCE_DEFAULT),
                NotificationChannel(LINK_CHANNEL, context.getString(R.string.notify_channel_link), NotificationManager.IMPORTANCE_MIN).apply {
                    setShowBadge(false)
                    setSound(null, null)
                    enableVibration(false)
                },
            ))
        }
    }
}
