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
import java.security.SecureRandom
import java.security.cert.CertificateException
import java.security.cert.X509Certificate
import java.util.concurrent.TimeUnit
import javax.net.ssl.SSLContext
import javax.net.ssl.X509TrustManager
import okhttp3.OkHttpClient
import okhttp3.Request
import okhttp3.Response
import okhttp3.WebSocket
import okhttp3.WebSocketListener

class Watch : Service() {
    private val main = Handler(Looper.getMainLooper())
    private val board = Board()
    private val prefs by lazy { getSharedPreferences(Prefs.FILE, MODE_PRIVATE) }
    private val manager by lazy { getSystemService(NotificationManager::class.java) }
    private val pace = Pace()
    private val again = Runnable { connect() }
    private val quiet = Runnable { dropped() }
    private var socket: WebSocket? = null
    private var turn = 0
    private var at = 0
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
        turn++
        net.stop()
        main.removeCallbacksAndMessages(null)
        socket?.cancel()
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
        val request = Request.Builder().url("wss://$host/api/events").apply { cookie?.let { header("Cookie", it) } }.build()
        val mine = ++turn
        socket = client(pin).newWebSocket(request, object : WebSocketListener() {
            override fun onOpen(webSocket: WebSocket, response: Response) {
                webSocket.send(Pace.BACK_FRAME)
                main.post {
                    if (mine != turn) return@post
                    pace.opened()
                    linked(true)
                    awaitFrame()
                }
            }

            override fun onMessage(webSocket: WebSocket, text: String) {
                main.post { if (mine == turn) heard(board.apply(text)) }
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

    private fun heard(heard: Heard) {
        pace.heard(heard)
        awaitFrame()
        if (heard == Heard.WAITS) show()
    }

    private fun awaitFrame() {
        main.removeCallbacks(quiet)
        main.postDelayed(quiet, pace.quietMs)
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
    }

    private fun dropped() {
        drop()
        if (!linked) at++
        linked(false)
        main.postDelayed(again, pace.dropped())
    }

    private fun restart() {
        drop()
        pace.restarted()
        connect()
    }

    private fun show() {
        if (!board.ready) return
        val shown = manager.activeNotifications.filter { it.tag == WAIT_TAG }.map { it.id.toLong() }.toSet()
        val seen = prefs.getString(Prefs.NOTIFY_SEEN, null)?.let { text -> text.split(',').mapNotNull { it.toLongOrNull() }.toSet() }
        val plan = Waits.plan(board.waits, seen, shown, switches(prefs), front)
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
        private val plain by lazy { OkHttpClient.Builder().callTimeout(Pace.CONNECT_MS, TimeUnit.MILLISECONDS).build() }

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
