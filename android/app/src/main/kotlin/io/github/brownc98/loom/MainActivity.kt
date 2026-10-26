package io.github.brownc98.loom

import android.Manifest
import android.content.ClipboardManager
import android.content.Context
import android.content.Intent
import android.content.res.Configuration
import android.graphics.drawable.GradientDrawable
import android.net.http.SslCertificate
import android.net.http.SslError
import android.net.Uri
import android.os.Build
import android.os.Bundle
import android.os.SystemClock
import android.provider.Settings
import android.text.InputType
import android.view.Gravity
import android.view.HapticFeedbackConstants
import android.view.View
import android.view.ViewGroup.LayoutParams.MATCH_PARENT
import android.view.ViewGroup.LayoutParams.WRAP_CONTENT
import android.view.WindowInsets
import android.view.inputmethod.EditorInfo
import android.view.inputmethod.InputMethodManager
import android.webkit.CookieManager
import android.webkit.JavascriptInterface
import android.webkit.SslErrorHandler
import android.webkit.ValueCallback
import android.webkit.WebChromeClient
import android.webkit.WebResourceError
import android.webkit.WebResourceRequest
import android.webkit.WebResourceResponse
import android.webkit.WebView
import android.webkit.WebViewClient
import android.widget.Button
import android.widget.EditText
import android.widget.FrameLayout
import android.widget.LinearLayout
import android.widget.ProgressBar
import android.widget.ScrollView
import android.widget.TextView
import androidx.activity.ComponentActivity
import androidx.activity.OnBackPressedCallback
import androidx.activity.result.contract.ActivityResultContracts
import androidx.core.content.FileProvider
import androidx.lifecycle.Lifecycle
import com.google.mlkit.vision.codescanner.GmsBarcodeScanning
import java.io.File
import java.net.InetSocketAddress
import java.net.Socket
import kotlin.concurrent.thread
import org.json.JSONObject

class MainActivity : ComponentActivity() {
    private lateinit var web: WebView
    private lateinit var card: ScrollView
    private lateinit var banner: LinearLayout
    private lateinit var root: LinearLayout
    private lateinit var palette: Context
    private var redraw: (() -> Unit)? = null
    private var checked: Long? = null
    private var waiting: Pair<String, File>? = null
    private var upload: ValueCallback<Array<Uri>>? = null
    private var fresh = false
    private var refused = false
    private var attempt = 0
    private var reaching = false
    private var unpaired = false
    private var jump: String? = null
    private val pickFile = registerForActivityResult(ActivityResultContracts.StartActivityForResult()) { result ->
        upload?.onReceiveValue(WebChromeClient.FileChooserParams.parseResult(result.resultCode, result.data))
        upload = null
    }
    private val askNotify = registerForActivityResult(ActivityResultContracts.RequestPermission()) { notifyChanged() }

    private val prefs by lazy { getSharedPreferences(Prefs.FILE, MODE_PRIVATE) }
    private val net by lazy { NetWatch(this, appeared = {}, moved = ::moved) }

    private var origin: String?
        get() = prefs.getString(Prefs.ORIGIN, null)
        set(value) = prefs.edit().putString(Prefs.ORIGIN, value).apply()

    private var doors: List<String>
        get() = Door.list(prefs.getString(Prefs.DOORS, null))
        set(value) = prefs.edit().putString(Prefs.DOORS, value.joinToString(",")).apply()

    private var pin: String?
        get() = prefs.getString(Prefs.PIN, null)
        set(value) = prefs.edit().putString(Prefs.PIN, value).apply()

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        palette = this
        web = WebView(this).apply {
            settings.javaScriptEnabled = true
            settings.domStorageEnabled = true
            webViewClient = Client()
            webChromeClient = Chooser()
            addJavascriptInterface(Clipboard(), "loomClipboard")
            addJavascriptInterface(Chrome(), "loomChrome")
            addJavascriptInterface(Browser(), "loomBrowser")
            addJavascriptInterface(Notify(), "loomNotify")
        }
        CookieManager.getInstance().setAcceptCookie(true)
        card = ScrollView(this).apply {
            isFillViewport = true
            setBackgroundColor(themed(R.color.bg))
        }
        banner = LinearLayout(this).apply {
            gravity = Gravity.CENTER_VERTICAL
            minimumHeight = dp(52)
            setPadding(dp(14), 0, dp(4), 0)
            background = box(R.color.surface, R.color.line_strong, radius = 0)
            visibility = View.GONE
        }
        val page = FrameLayout(this).apply {
            addView(web, MATCH_PARENT, MATCH_PARENT)
            addView(card, MATCH_PARENT, MATCH_PARENT)
        }
        root = LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            setBackgroundColor(themed(R.color.bg))
            addView(banner, MATCH_PARENT, WRAP_CONTENT)
            addView(page, LinearLayout.LayoutParams(MATCH_PARENT, 0, 1f))
        }
        root.setOnApplyWindowInsetsListener { view, insets ->
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
                val bars = insets.getInsets(WindowInsets.Type.systemBars() or WindowInsets.Type.ime())
                view.setPadding(bars.left, bars.top, bars.right, bars.bottom)
                WindowInsets.CONSUMED
            } else {
                insets
            }
        }
        setContentView(root)
        onBackPressedDispatcher.addCallback(object : OnBackPressedCallback(true) {
            override fun handleOnBackPressed() {
                if (card.visibility != View.VISIBLE && web.canGoBack()) web.goBack() else finish()
            }
        })
        if (savedInstanceState == null) jumpTo(intent)
        if (doors.isEmpty()) origin?.let(::open) ?: showStart(null)
        net.start()
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU && !prefs.getBoolean(Prefs.NOTIFY_ASKED, false)) {
            prefs.edit().putBoolean(Prefs.NOTIFY_ASKED, true).apply()
            askNotify.launch(Manifest.permission.POST_NOTIFICATIONS)
        }
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        jumpTo(intent)
    }

    private fun jumpTo(intent: Intent?) {
        val node = intent?.getLongExtra(Watch.EXTRA_NODE, -1)?.takeIf { it >= 0 } ?: return
        intent.removeExtra(Watch.EXTRA_NODE)
        val hash = "#/node/$node/ask"
        if (web.url != null && card.visibility != View.VISIBLE) web.evaluateJavascript("location.replace('$hash')", null) else jump = hash
    }

    private fun notifyChanged() {
        Watch.sync(this)
        web.evaluateJavascript("dispatchEvent(new Event('$NOTIFY_EVENT'))", null)
    }

    private fun reach(candidates: List<String>, fragment: String = "") {
        val mine = ++attempt
        reaching = true
        thread {
            val deadline = SystemClock.elapsedRealtime() + REACH_MS
            var found: String? = null
            while (found == null && SystemClock.elapsedRealtime() < deadline) {
                found = candidates.firstOrNull(::answers)
                if (found == null) Thread.sleep(RETRY_MS)
            }
            runOnUiThread {
                if (mine != attempt) return@runOnUiThread
                reaching = false
                when (found) {
                    null -> showFailed("https://${candidates.first()}/")
                    else -> {
                        copyCookie(found)
                        origin = "https://$found"
                        open("https://$found/$fragment")
                    }
                }
            }
        }
    }

    private fun answers(door: String): Boolean {
        val host = door.substringBeforeLast(':')
        val port = door.substringAfterLast(':').toIntOrNull() ?: return false
        return runCatching { Socket().use { it.connect(InetSocketAddress(host, port), PROBE_MS) } }.isSuccess
    }

    private fun copyCookie(to: String) {
        if (Door.deviceCookie(to) != null) return
        doors.firstNotNullOfOrNull(Door::deviceCookie)?.let { putCookies(listOf(to), it) }
    }

    private fun spreadCookie(from: String) {
        val token = Door.deviceCookie(from) ?: return
        putCookies(doors.filter { it != from && Door.deviceCookie(it) != token }, token)
    }

    private fun putCookies(to: List<String>, token: String) {
        if (to.isEmpty()) return
        val cookies = CookieManager.getInstance()
        to.forEach { cookies.setCookie("https://$it/", "$token; Path=/; Max-Age=$COOKIE_MAX_AGE_SECS; Secure; HttpOnly; SameSite=Strict") }
        cookies.flush()
    }

    override fun onResume() {
        super.onResume()
        Watch.front = true
        notifyChanged()
        waiting?.takeIf { packageManager.canRequestPackageInstalls() }?.let { (version, apk) -> install(version, apk) }
        val now = SystemClock.elapsedRealtime()
        if (checked.let { it == null || now - it >= Release.CHECK_EVERY_MS }) {
            checked = now
            thread { Release.newer()?.let { runOnUiThread { showAvailable(it) } } }
        }
        recheck()
    }

    override fun onDestroy() {
        net.stop()
        super.onDestroy()
    }

    private fun moved() {
        if (web.url != null && card.visibility != View.VISIBLE) web.evaluateJavascript("dispatchEvent(new Event('online'))", null)
        if (lifecycle.currentState.isAtLeast(Lifecycle.State.RESUMED)) recheck()
    }

    private fun recheck() {
        val at = origin?.removePrefix("https://")
        if (doors.isNotEmpty() && !reaching) {
            when {
                web.url == null || at !in doors -> reach(doors)
                card.visibility != View.VISIBLE -> thread { if (!answers(at!!)) runOnUiThread { if (!reaching) reach(doors) } }
            }
        }
    }

    override fun onPause() {
        super.onPause()
        Watch.front = false
        Watch.sync(this)
        CookieManager.getInstance().flush()
    }

    private fun open(url: String) {
        getSystemService(InputMethodManager::class.java).hideSoftInputFromWindow(card.windowToken, 0)
        card.visibility = View.GONE
        fresh = true
        refused = false
        web.loadUrl(jump?.takeIf { '#' !in url }?.let { url.removeSuffix("/") + "/" + it } ?: url)
        jump = null
    }

    private fun showStart(error: Int?, typedCode: String = "") {
        lateinit var code: EditText
        val submit = {
            val pair = code.text.toString()
            if (pair.length == CODE_LENGTH) find(pair) else showStart(R.string.code_error, pair)
        }
        code = codeBoxes(typedCode) { submit() }
        show(
            title = R.string.start_title,
            body = getString(R.string.start_body),
            error = error,
            button(R.string.scan, primary = true) { scan() },
            text(getString(R.string.or), R.color.ink_3, 12f).apply { gravity = Gravity.CENTER },
            code.parent as View,
            button(R.string.open, primary = false) { submit() },
        )
    }

    private fun find(code: String) {
        show(R.string.finding_title, getString(R.string.finding_body), null, ProgressBar(this).apply { isIndeterminate = true })
        thread {
            val found = runCatching { Find.run(code) }.getOrDefault(Find.Missing)
            runOnUiThread {
                when (found) {
                    is Find.Found -> pinDoor(found.fp, found.doors, code)
                    Find.Wrong -> showStart(R.string.code_wrong, code)
                    Find.Missing -> showStart(R.string.not_found, code)
                }
            }
        }
    }

    private fun pinDoor(fp: String, found: List<String>, code: String) {
        pin = fp
        doors = found
        reach(found, "#pair=$code")
    }

    private fun codeBoxes(typed: String, full: () -> Unit): EditText {
        val boxes = List(CODE_LENGTH) {
            text("", R.color.ink, 20f).apply {
                gravity = Gravity.CENTER
                typeface = android.graphics.Typeface.create(android.graphics.Typeface.MONOSPACE, android.graphics.Typeface.BOLD)
            }
        }
        val row = LinearLayout(this).apply {
            gravity = Gravity.CENTER_VERTICAL
            boxes.forEachIndexed { i, b ->
                if (i == CODE_LENGTH / 2) addView(text("-", R.color.ink_3, 20f).apply { setPadding(dp(2), 0, dp(2), 0) })
                addView(b, LinearLayout.LayoutParams(0, dp(48), 1f).apply { if (i % (CODE_LENGTH / 2) != 0) leftMargin = dp(6) })
            }
        }
        val input = EditText(this).apply {
            alpha = 0f
            isCursorVisible = false
            background = null
            inputType = InputType.TYPE_CLASS_TEXT or InputType.TYPE_TEXT_FLAG_CAP_CHARACTERS or InputType.TYPE_TEXT_FLAG_NO_SUGGESTIONS or InputType.TYPE_TEXT_VARIATION_VISIBLE_PASSWORD
            imeOptions = EditorInfo.IME_ACTION_GO
            contentDescription = getString(R.string.code_hint)
        }
        fun draw() {
            val value = input.text.toString()
            boxes.forEachIndexed { i, b ->
                b.text = value.getOrNull(i)?.toString() ?: ""
                val at = input.hasFocus() && i == minOf(value.length, CODE_LENGTH - 1)
                b.background = box(R.color.surface, if (at) R.color.ink else R.color.line_strong, radius = 8).apply { if (at) setStroke(dp(2), themed(R.color.ink)) }
            }
        }
        input.setText(typed)
        input.addTextChangedListener(object : android.text.TextWatcher {
            override fun beforeTextChanged(s: CharSequence, start: Int, count: Int, after: Int) {}
            override fun onTextChanged(s: CharSequence, start: Int, before: Int, count: Int) {}
            override fun afterTextChanged(s: android.text.Editable) {
                val clean = s.toString().uppercase().filter { it in 'A'..'Z' || it in '0'..'9' }.take(CODE_LENGTH)
                if (clean != s.toString()) {
                    s.replace(0, s.length, clean)
                    return
                }
                input.setSelection(clean.length)
                draw()
                if (clean.length == CODE_LENGTH) full()
            }
        })
        input.setOnFocusChangeListener { _, _ -> draw() }
        input.setOnClickListener { input.setSelection(input.text.length) }
        draw()
        FrameLayout(this).apply {
            addView(row, MATCH_PARENT, dp(48))
            addView(input, MATCH_PARENT, dp(48))
        }
        return input
    }

    private fun showFailed(failed: String, body: Int = if (pin != null) R.string.failed_body_door else R.string.failed_body) {
        val actions = LinearLayout(this).apply {
            addView(button(R.string.retry, primary = true) { if (doors.isNotEmpty()) reach(doors) else open(failed) }, LinearLayout.LayoutParams(0, WRAP_CONTENT, 1f))
            addView(View(this@MainActivity), dp(8), 1)
            addView(button(if (pin != null) R.string.rescan else R.string.change, primary = false) {
                origin = null
                if (pin != null) scan() else showStart(null)
            }, LinearLayout.LayoutParams(0, WRAP_CONTENT, 1f))
        }
        val place = Uri.parse(failed).let { if (it.port > 0) "${it.host}:${it.port}" else it.host }
        show(R.string.failed_title, "$place\n${getString(body)}", null, actions)
    }

    private fun showAvailable(version: String) {
        showBanner(getString(R.string.update_title, version), getString(R.string.update_current, BuildConfig.VERSION_NAME), R.color.ink_3, R.string.update) {
            download(version)
        }
    }

    private fun download(version: String) {
        val meter = ProgressBar(this, null, android.R.attr.progressBarStyleHorizontal).apply {
            max = 100
            isIndeterminate = true
            progressTintList = android.content.res.ColorStateList.valueOf(themed(R.color.accent))
        }
        redraw = null
        val head = text(getString(R.string.update_downloading, version, 0), R.color.ink, 14f).apply { setTypeface(typeface, android.graphics.Typeface.BOLD) }
        banner.removeAllViews()
        banner.addView(LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            addView(head)
            addView(meter, MATCH_PARENT, dp(8))
        }, LinearLayout.LayoutParams(0, dp(52), 1f).apply { rightMargin = dp(10) })
        banner.visibility = View.VISIBLE
        val apk = File(cacheDir, "update/loom.apk")
        thread {
            val ok = runCatching {
                Release.download(version, apk) { percent ->
                    runOnUiThread {
                        meter.isIndeterminate = percent < 0
                        if (percent >= 0) {
                            meter.progress = percent
                            head.text = getString(R.string.update_downloading, version, percent)
                        }
                    }
                }
            }.isSuccess && Release.verified(packageManager, apk, packageName, version)
            runOnUiThread {
                if (ok) install(version, apk) else {
                    apk.delete()
                    showBanner(getString(R.string.update_title, version), getString(R.string.update_failed), R.color.err, R.string.retry) { download(version) }
                }
            }
        }
    }

    private fun install(version: String, apk: File) {
        waiting = null
        if (!packageManager.canRequestPackageInstalls()) {
            showBanner(getString(R.string.update_permission_title), getString(R.string.update_permission_body), R.color.ink_3, R.string.update_settings) {
                waiting = version to apk
                startActivity(Intent(Settings.ACTION_MANAGE_UNKNOWN_APP_SOURCES, Uri.parse("package:$packageName")))
            }
            return
        }
        showBanner(getString(R.string.update_title, version), getString(R.string.update_ready), R.color.ink_3, R.string.update_install) { install(version, apk) }
        val uri = FileProvider.getUriForFile(this, "$packageName.update", apk)
        startActivity(Intent(Intent.ACTION_VIEW).setDataAndType(uri, "application/vnd.android.package-archive").addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION))
    }

    private fun showBanner(title: String, body: String, bodyColor: Int, action: Int, onClick: () -> Unit) {
        redraw = { showBanner(title, body, bodyColor, action, onClick) }
        banner.removeAllViews()
        banner.addView(LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            addView(text(title, R.color.ink, 14f).apply { setTypeface(typeface, android.graphics.Typeface.BOLD) })
            addView(text(body, bodyColor, 12f))
        }, LinearLayout.LayoutParams(0, WRAP_CONTENT, 1f))
        banner.addView(button(action, primary = true, onClick).apply {
            minHeight = dp(36)
            minimumHeight = dp(36)
            textSize = 13f
            setPadding(dp(12), 0, dp(12), 0)
        }, LinearLayout.LayoutParams(WRAP_CONTENT, dp(36)))
        banner.addView(text("✕", R.color.ink_2, 18f).apply {
            gravity = Gravity.CENTER
            contentDescription = getString(R.string.close)
            setOnClickListener {
                waiting = null
                banner.visibility = View.GONE
            }
        }, LinearLayout.LayoutParams(dp(44), dp(44)))
        banner.visibility = View.VISIBLE
    }

    private fun scan() {
        GmsBarcodeScanning.getClient(this).startScan().addOnSuccessListener { code ->
            val url = Uri.parse(code.rawValue ?: "")
            val params = Find.params(url.fragment ?: "")
            val found = Find.doors(params)
            val pair = params["pair"]
            val fp = params["fp"]
            when {
                url.scheme != "https" || pair == null -> showStart(R.string.qr_error)
                fp != null && found.isNotEmpty() -> pinDoor(fp, found, pair)
                url.host?.endsWith(TAILNET) == true -> {
                    pin = null
                    doors = emptyList()
                    origin = "https://${url.host}"
                    open("https://${url.host}/#pair=$pair")
                }
                else -> showStart(R.string.qr_error)
            }
        }.addOnFailureListener { showStart(R.string.scan_error) }
    }

    private fun show(title: Int, body: String, error: Int?, vararg rows: View) {
        val column = LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            background = box(R.color.surface, R.color.line, radius = 12)
            setPadding(dp(18), dp(18), dp(18), dp(18))
            val mark = text("L", R.color.accent_ink, 16f).apply {
                gravity = Gravity.CENTER
                typeface = android.graphics.Typeface.create(android.graphics.Typeface.MONOSPACE, android.graphics.Typeface.BOLD)
                background = box(R.color.accent, R.color.accent, radius = 8)
            }
            addView(mark, LinearLayout.LayoutParams(dp(32), dp(32)))
            val head = text(getString(title), R.color.ink, 17f).apply { setTypeface(typeface, android.graphics.Typeface.BOLD) }
            val all = listOfNotNull(head, text(body, R.color.ink_2, 14f), error?.let { text(getString(it), R.color.err, 13f) }) + rows
            for (row in all) {
                addView(row, LinearLayout.LayoutParams(MATCH_PARENT, WRAP_CONTENT).apply { topMargin = dp(12) })
            }
        }
        card.removeAllViews()
        card.addView(FrameLayout(this).apply {
            addView(column, FrameLayout.LayoutParams(MATCH_PARENT, WRAP_CONTENT).apply {
                setMargins(dp(20), dp(120), dp(20), dp(20))
            })
        })
        card.visibility = View.VISIBLE
    }

    private fun text(value: String, color: Int, size: Float) = TextView(this).apply {
        text = value
        textSize = size
        setTextColor(themed(color))
    }

    private fun button(label: Int, primary: Boolean, onClick: () -> Unit) = Button(this).apply {
        setText(label)
        isAllCaps = false
        textSize = 15f
        stateListAnimator = null
        minHeight = dp(44)
        setTextColor(themed(if (primary) R.color.accent_ink else R.color.ink))
        background = if (primary) box(R.color.accent, R.color.accent) else box(R.color.surface, R.color.line_strong)
        setOnClickListener { onClick() }
    }

    private fun box(fill: Int, line: Int, radius: Int = 10) = GradientDrawable().apply {
        setColor(themed(fill))
        setStroke(dp(1), themed(line))
        cornerRadius = dp(radius).toFloat()
    }

    private fun themed(id: Int) = palette.getColor(id)

    private fun dp(value: Int) = (value * resources.displayMetrics.density).toInt()

    private inner class Client : WebViewClient() {
        override fun shouldOverrideUrlLoading(view: WebView, request: WebResourceRequest): Boolean {
            if (request.url.host == Uri.parse(origin ?: "").host) return false
            runCatching { startActivity(Intent(Intent.ACTION_VIEW, request.url)) }
            return true
        }

        override fun onPageStarted(view: WebView, url: String, favicon: android.graphics.Bitmap?) {
            unpaired = false
        }

        override fun onPageFinished(view: WebView, url: String) {
            if (fresh && card.visibility != View.VISIBLE) view.clearHistory()
            fresh = false
            val door = Uri.parse(url).let { "${it.host}:${it.port}" }
            if (!unpaired && card.visibility != View.VISIBLE && door in doors) spreadCookie(door)
            if (!unpaired && card.visibility != View.VISIBLE) Watch.sync(this@MainActivity)
        }

        override fun onReceivedError(view: WebView, request: WebResourceRequest, error: WebResourceError) {
            if (!request.isForMainFrame || refused) return
            if (doors.isNotEmpty() && !fresh) reach(doors) else showFailed(request.url.toString())
        }

        override fun onReceivedSslError(view: WebView, handler: SslErrorHandler, error: SslError) {
            val place = Uri.parse(error.url).let { "${it.host}:${it.port}" }
            if (pin != null && place in doors && fingerprint(error.certificate) == pin) return handler.proceed()
            handler.cancel()
            if (Uri.parse(error.url).host == Uri.parse(origin ?: "").host) {
                refused = true
                showFailed(error.url, R.string.failed_pin)
            }
        }

        override fun onReceivedHttpError(view: WebView, request: WebResourceRequest, response: WebResourceResponse) {
            if (!request.isForMainFrame) return
            if (response.statusCode == 401) unpaired = true
            if (response.statusCode >= 500) showFailed(request.url.toString())
        }
    }

    private inner class Clipboard {
        @JavascriptInterface
        fun read(): String {
            val clip = (getSystemService(Context.CLIPBOARD_SERVICE) as ClipboardManager).primaryClip
            return clip?.takeIf { it.itemCount > 0 }?.getItemAt(0)?.coerceToText(this@MainActivity)?.toString() ?: ""
        }
    }

    private inner class Chrome {
        @JavascriptInterface
        fun haptic() {
            runOnUiThread { web.performHapticFeedback(HapticFeedbackConstants.LONG_PRESS) }
        }

        @JavascriptInterface
        fun setScheme(scheme: String) {
            val light = scheme == "light"
            runOnUiThread {
                val config = Configuration(resources.configuration).apply {
                    uiMode = (uiMode and Configuration.UI_MODE_NIGHT_MASK.inv()) or (if (light) Configuration.UI_MODE_NIGHT_NO else Configuration.UI_MODE_NIGHT_YES)
                }
                palette = createConfigurationContext(config)
                root.setBackgroundColor(themed(R.color.bg))
                card.setBackgroundColor(themed(R.color.bg))
                banner.background = box(R.color.surface, R.color.line_strong, radius = 0)
                if (banner.visibility == View.VISIBLE) redraw?.invoke()
                val flag = if (light) View.SYSTEM_UI_FLAG_LIGHT_STATUS_BAR else 0
                if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
                    val mask = android.view.WindowInsetsController.APPEARANCE_LIGHT_STATUS_BARS
                    window.insetsController?.setSystemBarsAppearance(if (light) mask else 0, mask)
                } else {
                    @Suppress("DEPRECATION")
                    window.decorView.systemUiVisibility = flag
                }
            }
        }
    }

    private inner class Browser {
        @JavascriptInterface
        fun open(url: String) {
            val uri = Uri.parse(url)
            if (uri.scheme != "https" && uri.scheme != "http") return
            if (uri.path?.startsWith("/api/files/pass/") != true) return
            runOnUiThread {
                runCatching { startActivity(Intent(Intent.ACTION_VIEW, uri).addCategory(Intent.CATEGORY_BROWSABLE)) }
            }
        }
    }

    private inner class Notify {
        @JavascriptInterface
        fun state(): String {
            val allowed = Watch.allowed(this@MainActivity)
            val on = Watch.switches(prefs)
            return JSONObject().put("ask", allowed && on.ask).put("done", allowed && on.done).put("allowed", allowed).toString()
        }

        @JavascriptInterface
        fun ask(on: Boolean) {
            runOnUiThread { choose(on) { it.copy(ask = on) } }
        }

        @JavascriptInterface
        fun done(on: Boolean) {
            runOnUiThread { choose(on) { it.copy(done = on) } }
        }

        @JavascriptInterface
        fun link() {
            val intent = Intent(Settings.ACTION_CHANNEL_NOTIFICATION_SETTINGS)
                .putExtra(Settings.EXTRA_APP_PACKAGE, packageName)
                .putExtra(Settings.EXTRA_CHANNEL_ID, Watch.LINK_CHANNEL)
            runOnUiThread { runCatching { startActivity(intent) } }
        }
    }

    private fun choose(on: Boolean, change: (Switches) -> Switches) {
        Watch.store(prefs, change(Watch.switches(prefs)))
        if (on && !Watch.allowed(this)) {
            runCatching { startActivity(Intent(Settings.ACTION_APP_NOTIFICATION_SETTINGS).putExtra(Settings.EXTRA_APP_PACKAGE, packageName)) }
        }
        notifyChanged()
    }

    private inner class Chooser : WebChromeClient() {
        override fun onShowFileChooser(view: WebView, callback: ValueCallback<Array<Uri>>, params: FileChooserParams): Boolean {
            upload?.onReceiveValue(null)
            upload = callback
            return runCatching { pickFile.launch(params.createIntent()) }
                .onFailure { upload = null }
                .isSuccess
        }
    }

    private fun fingerprint(cert: SslCertificate): String? =
        SslCertificate.saveState(cert).getByteArray("x509-certificate")?.let(Door::fingerprint)

    private companion object {
        const val NOTIFY_EVENT = "loomnotify"
        const val COOKIE_MAX_AGE_SECS = 400 * 24 * 60 * 60
        const val PROBE_MS = 1500
        const val REACH_MS = 10_000L
        const val RETRY_MS = 500L
        const val TAILNET = ".ts.net"
        const val CODE_LENGTH = 8
    }
}
