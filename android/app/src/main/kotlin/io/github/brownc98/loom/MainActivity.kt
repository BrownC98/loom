package io.github.brownc98.loom

import android.content.ClipboardManager
import android.content.Context
import android.content.Intent
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
import com.google.mlkit.vision.codescanner.GmsBarcodeScanning
import java.io.File
import java.net.InetSocketAddress
import java.net.Socket
import java.security.MessageDigest
import kotlin.concurrent.thread

/**
 * PC loom이 내는 폰 화면을 연다. 길은 둘이다: 데몬의 폰 문(`https://<IP>:<포트>`, 자체 서명 인증서를 QR의 지문으로 확인)과
 * Tailscale Serve(`https://<PC>.ts.net`). 화면은 PC가 그리고, 앱은 PC 주소를 모를 때(①)와 닿지 못할 때(②)의 카드와
 * 앱 새 판 띠만 그린다. 근거: docs/design.md 「Android 앱」, 목업 docs/mockups/android-app.html, phone-direct.html ④,
 * docs/mockups/android-update.html.
 */
class MainActivity : ComponentActivity() {
    private lateinit var web: WebView
    private lateinit var card: ScrollView
    private lateinit var banner: LinearLayout
    private lateinit var root: LinearLayout
    /** 마지막으로 새 판을 확인한 때(`SystemClock.elapsedRealtime`). 앱을 열 때와 그 뒤 6시간마다 확인한다. */
    private var checked: Long? = null
    /** 설치 허용을 켜러 설정에 간 동안 기다리는 APK. 돌아오면 설치 화면을 연다. */
    private var waiting: Pair<String, File>? = null
    private var upload: ValueCallback<Array<Uri>>? = null
    /** 앱이 연 첫 페이지가 뜨면 뒤로 기록을 비운다. 실패한 페이지로 돌아가지 않게 한다. */
    private var fresh = false
    /** 인증서 지문이 달라 페이지를 끊었다. 뒤따르는 연결 실패가 지문 카드를 덮지 않게 한다. */
    private var refused = false
    /** 마지막 `reach`의 번호. QR을 다시 찍는 등 새 `reach`가 앞선 것의 결과를 버리게 한다. */
    private var attempt = 0
    private var reaching = false
    /** 지금 메인 페이지가 401(짝짓지 않은 기기, src/daemon/origin.rs)이다. 그 쿠키는 다른 문에 퍼뜨리지 않는다. */
    private var unpaired = false
    private val pickFile = registerForActivityResult(ActivityResultContracts.StartActivityForResult()) { result ->
        upload?.onReceiveValue(WebChromeClient.FileChooserParams.parseResult(result.resultCode, result.data))
        upload = null
    }

    private val prefs by lazy { getSharedPreferences("loom", MODE_PRIVATE) }

    /** 지금 연 PC 주소(`https://<IP>:<포트>` 또는 `https://<PC>.ts.net`). 짝짓기 코드는 저장하지 않는다. */
    private var origin: String?
        get() = prefs.getString(ORIGIN, null)
        set(value) = prefs.edit().putString(ORIGIN, value).apply()

    /** QR의 폰 문 주소(`<IP>:<포트>`, 같은 네트워크가 먼저, Tailscale이 뒤). Serve만 쓰면 비어 있다. */
    private var doors: List<String>
        get() = prefs.getString(DOORS, null)?.split(',')?.filter { it.isNotEmpty() } ?: emptyList()
        set(value) = prefs.edit().putString(DOORS, value.joinToString(",")).apply()

    /** 폰 문 인증서 DER의 SHA-256(base64url, 덧붙임 없음). 이 지문이 아닌 인증서의 페이지는 열지 않는다. */
    private var pin: String?
        get() = prefs.getString(PIN, null)
        set(value) = prefs.edit().putString(PIN, value).apply()

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        web = WebView(this).apply {
            settings.javaScriptEnabled = true
            settings.domStorageEnabled = true
            webViewClient = Client()
            webChromeClient = Chooser()
            addJavascriptInterface(Clipboard(), "loomClipboard")
            addJavascriptInterface(Chrome(), "loomChrome")
            addJavascriptInterface(Browser(), "loomBrowser")
        }
        CookieManager.getInstance().setAcceptCookie(true)
        card = ScrollView(this).apply {
            isFillViewport = true
            setBackgroundColor(getColor(R.color.bg))
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
        // 띠는 PC 화면을 아래로 민다. 위에 겹치면 PC 화면의 머리 줄을 가린다(목업 ①).
        root = LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            setBackgroundColor(getColor(R.color.bg))
            addView(banner, MATCH_PARENT, WRAP_CONTENT)
            addView(page, LinearLayout.LayoutParams(MATCH_PARENT, 0, 1f))
        }
        // targetSdk 35부터 화면이 상태 줄·키보드 밑까지 그린다. 그 자리만큼 안으로 민다.
        // 민 자리를 WebView에 다시 넘기면 페이지의 env(safe-area-inset-*)가 한 번 더 비워 머리 위가 두 번 비었다(2026-10-22 사람 "헤더 상단 공백이 너무 큼"). 그래서 여기서 다 쓴다.
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
        // 폰 문은 `onResume`이 두드린다.
        if (doors.isEmpty()) origin?.let(::open) ?: showStart(null)
    }

    /**
     * 폰 문 주소를 차례로 TCP로 두드려 먼저 답한 곳을 연다(같은 Wi-Fi가 먼저, 밖이면 Tailscale 주소).
     * 아무도 답하지 않으면 `REACH_MS` 동안 다시 두드린다. `fragment`는 처음 짝지을 때의 `#pair=<코드>`다.
     */
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

    /**
     * 기기 쿠키는 호스트마다 따로다. 쿠키가 없는 문에만 다른 문의 쿠키를 넣는다. 있는 쿠키는 `spreadCookie`가 맞춘다.
     * 있는 쿠키를 덮으면 다시 짝지은 뒤 남은 옛 토큰(지운 기기)이 새 토큰을 덮을 수 있다.
     */
    private fun copyCookie(to: String) {
        if (deviceCookie(to) != null) return
        doors.firstNotNullOfOrNull(::deviceCookie)?.let { putCookies(listOf(to), it) }
    }

    /** 짝지은 기기로 열린 문의 쿠키를 다른 문에 넣는다. 같은 Wi-Fi에서 짝지은 뒤 밖에서 Tailscale 주소로 열 때 다시 짝짓지 않는다. */
    private fun spreadCookie(from: String) {
        val token = deviceCookie(from) ?: return
        putCookies(doors.filter { it != from && deviceCookie(it) != token }, token)
    }

    private fun deviceCookie(door: String): String? =
        CookieManager.getInstance().getCookie("https://$door/")?.split(';')?.map { it.trim() }?.firstOrNull { it.startsWith("$DEVICE_COOKIE=") }

    /** 쿠키 속성은 데몬의 `device_cookie`(src/daemon/phone.rs)와 같다. 앱이 곧 꺼져도 남게 바로 디스크에 쓴다. */
    private fun putCookies(to: List<String>, token: String) {
        if (to.isEmpty()) return
        val cookies = CookieManager.getInstance()
        to.forEach { cookies.setCookie("https://$it/", "$token; Path=/; Max-Age=$COOKIE_MAX_AGE_SECS; Secure; HttpOnly; SameSite=Strict") }
        cookies.flush()
    }

    override fun onResume() {
        super.onResume()
        waiting?.takeIf { packageManager.canRequestPackageInstalls() }?.let { (version, apk) -> install(version, apk) }
        val now = SystemClock.elapsedRealtime()
        if (checked.let { it == null || now - it >= Release.CHECK_EVERY_MS }) {
            checked = now
            thread { Release.newer()?.let { runOnUiThread { showAvailable(it) } } }
        }
        // 앱을 다시 앞으로 가져오면 연 문이 아직 닿는지 본다. Wi-Fi를 벗어나면 같은 Wi-Fi 주소는 닿지 않는다.
        // 웹소켓은 같은 주소에 다시 붙기만 한다(web/src/api.ts `RECONNECT_MS`). 그래서 앱이 닿는 다른 문을 연다.
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
        CookieManager.getInstance().flush()
    }

    private fun open(url: String) {
        getSystemService(InputMethodManager::class.java).hideSoftInputFromWindow(card.windowToken, 0)
        card.visibility = View.GONE
        fresh = true
        refused = false
        web.loadUrl(url)
    }

    /** ① 처음 실행: QR을 찍거나 PC에 나온 코드를 적는다. 코드로는 같은 Wi-Fi에서 PC를 찾는다(`find`). */
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

    /**
     * 같은 Wi-Fi에서 코드로 PC를 찾는다(`Find`). PC의 답은 코드로 확인한 폰 문 지문과 주소다. 그 뒤는 QR로 찍은 것과 같다.
     * 사람 결정 Q426 B: 주소 칸 없이 코드만 적는다.
     */
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

    /** 확인한 폰 문 지문과 주소를 저장하고 닿는 주소에서 `#pair=<코드>`를 연다. */
    private fun pinDoor(fp: String, found: List<String>, code: String) {
        pin = fp
        doors = found
        reach(found, "#pair=$code")
    }

    /**
     * 짝짓기 코드 칸: PC 화면과 같은 네모 8칸(`□□□□-□□□□`, src/daemon/phone.rs `UNPAIRED_PAGE`). 보이지 않는 입력 칸 하나가
     * 글을 받고 네모가 그린다. 그래서 다음 칸으로 가기, 지우기, 붙여 넣기(`-`가 있든 없든)를 입력 칸이 맡는다.
     * 8자를 다 적으면 `full`을 부른다. 돌려주는 입력 칸의 부모가 화면에 넣을 줄이다.
     */
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
            // 대문자 자판, 맞춤법 고치기 끔.
            inputType = InputType.TYPE_CLASS_TEXT or InputType.TYPE_TEXT_FLAG_CAP_CHARACTERS or InputType.TYPE_TEXT_FLAG_NO_SUGGESTIONS or InputType.TYPE_TEXT_VARIATION_VISIBLE_PASSWORD
            imeOptions = EditorInfo.IME_ACTION_GO
            contentDescription = getString(R.string.code_hint)
        }
        fun draw() {
            val value = input.text.toString()
            boxes.forEachIndexed { i, b ->
                b.text = value.getOrNull(i)?.toString() ?: ""
                val at = input.hasFocus() && i == minOf(value.length, CODE_LENGTH - 1)
                b.background = box(R.color.surface, if (at) R.color.ink else R.color.line_strong, radius = 8).apply { if (at) setStroke(dp(2), getColor(R.color.ink)) }
            }
        }
        // 받은 글을 듣기 전에 넣는다. 다 찬 코드로 `full`을 부르면 아직 화면이 없다.
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

    /** ② PC에 닿지 못함. */
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

    /** 앱 새 판 띠 ①: 새 판이 있음. */
    private fun showAvailable(version: String) {
        showBanner(getString(R.string.update_title, version), getString(R.string.update_current, BuildConfig.VERSION_NAME), R.color.ink_3, R.string.update) {
            download(version)
        }
    }

    private fun download(version: String) {
        val meter = ProgressBar(this, null, android.R.attr.progressBarStyleHorizontal).apply {
            max = 100
            isIndeterminate = true
            progressTintList = android.content.res.ColorStateList.valueOf(getColor(R.color.accent))
        }
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

    /** ②: Android 설치 화면을 연다. 설치 허용이 꺼져 있으면 그 설정부터 연다. */
    private fun install(version: String, apk: File) {
        waiting = null
        if (!packageManager.canRequestPackageInstalls()) {
            showBanner(getString(R.string.update_permission_title), getString(R.string.update_permission_body), R.color.ink_3, R.string.update_settings) {
                waiting = version to apk
                startActivity(Intent(Settings.ACTION_MANAGE_UNKNOWN_APP_SOURCES, Uri.parse("package:$packageName")))
            }
            return
        }
        // 설치 화면에서 취소하면 다시 받지 않고 이 띠로 다시 연다.
        showBanner(getString(R.string.update_title, version), getString(R.string.update_ready), R.color.ink_3, R.string.update_install) { install(version, apk) }
        val uri = FileProvider.getUriForFile(this, "$packageName.update", apk)
        startActivity(Intent(Intent.ACTION_VIEW).setDataAndType(uri, "application/vnd.android.package-archive").addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION))
    }

    /** 띠 한 줄: 제목, 작은 글, 단추, 닫기. 닫으면 다음 확인(앱을 다시 열거나 6시간 뒤)까지 숨는다. */
    private fun showBanner(title: String, body: String, bodyColor: Int, action: Int, onClick: () -> Unit) {
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

    /** QR: `https://<호스트>/#pair=<코드>&fp=<지문>&a=<IP:포트>,…`(src/daemon/phone.rs `invite_url`). `fp`·`a`가 있으면 폰 문으로 붙는다. */
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
                    // Serve만 켠 PC의 QR. 데몬도 Serve 길에서는 `.ts.net`으로 온 요청만 폰으로 받는다(src/daemon/origin.rs).
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
        setTextColor(getColor(color))
    }

    private fun button(label: Int, primary: Boolean, onClick: () -> Unit) = Button(this).apply {
        setText(label)
        isAllCaps = false
        textSize = 15f
        stateListAnimator = null
        minHeight = dp(44)
        setTextColor(getColor(if (primary) R.color.accent_ink else R.color.ink))
        background = if (primary) box(R.color.accent, R.color.accent) else box(R.color.surface, R.color.line_strong)
        setOnClickListener { onClick() }
    }

    private fun box(fill: Int, line: Int, radius: Int = 10) = GradientDrawable().apply {
        setColor(getColor(fill))
        setStroke(dp(1), getColor(line))
        cornerRadius = dp(radius).toFloat()
    }

    private fun dp(value: Int) = (value * resources.displayMetrics.density).toInt()

    private inner class Client : WebViewClient() {
        /** 같은 PC의 페이지는 쿠키가 있는 앱 안에서, 다른 사이트는 폰 브라우저로 연다. */
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
        }

        /**
         * 앱이 연 첫 페이지가 실패하면 실패 카드를 보인다. 그 뒤의 실패(데몬 새 판으로 바뀐 뒤 화면이 다시 읽기 등)는
         * 폰 문을 다시 두드린다. 데몬이 다시 뜨는 몇 초를 기다린다.
         */
        override fun onReceivedError(view: WebView, request: WebResourceRequest, error: WebResourceError) {
            if (!request.isForMainFrame || refused) return
            if (doors.isNotEmpty() && !fresh) reach(doors) else showFailed(request.url.toString())
        }

        /**
         * 폰 문의 자체 서명 인증서: QR의 지문과 같고 폰 문 주소일 때만 연다. 다르면 가운데에 낀 기기거나 인증서를 바꾼 PC다.
         * WebView는 같은 호스트·인증서에 내린 `proceed`를 기억해 하위 요청과 웹소켓에도 쓴다.
         */
        override fun onReceivedSslError(view: WebView, handler: SslErrorHandler, error: SslError) {
            val place = Uri.parse(error.url).let { "${it.host}:${it.port}" }
            if (pin != null && place in doors && fingerprint(error.certificate) == pin) return handler.proceed()
            handler.cancel()
            if (Uri.parse(error.url).host == Uri.parse(origin ?: "").host) {
                refused = true
                showFailed(error.url, R.string.failed_pin)
            }
        }

        /** Serve는 데몬이 꺼져 있으면 502를 낸다. */
        override fun onReceivedHttpError(view: WebView, request: WebResourceRequest, response: WebResourceResponse) {
            if (!request.isForMainFrame) return
            if (response.statusCode == 401) unpaired = true
            if (response.statusCode >= 500) showFailed(request.url.toString())
        }
    }

    /**
     * 폰 화면의 [붙여넣기]가 읽는 클립보드. WebView는 `navigator.clipboard.readText`를 허락 없이 막는다.
     * 다른 주소로 나가는 길은 `Client`가 막고 PC 화면만 열므로 이 다리는 PC 화면만 쓴다.
     */
    private inner class Clipboard {
        @JavascriptInterface
        fun read(): String {
            val clip = (getSystemService(Context.CLIPBOARD_SERVICE) as ClipboardManager).primaryClip
            return clip?.takeIf { it.itemCount > 0 }?.getItemAt(0)?.coerceToText(this@MainActivity)?.toString() ?: ""
        }
    }

    /**
     * 폰 화면이 고른 테마(`light`·`dark`)에 상태 줄을 맞춘다. 시스템 모드와 다른 테마를 고를 수 있어서
     * 상태 줄 글자색과 바탕을 이 값으로 정한다(2026-10-23 Q464 A).
     */
    private inner class Chrome {
        @JavascriptInterface
        fun setScheme(scheme: String) {
            val light = scheme == "light"
            runOnUiThread {
                root.setBackgroundColor(getColor(if (light) R.color.bg_light else R.color.bg_dark))
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

    /**
     * 파일·질문 문서의 통행증 주소(`/api/files/pass/`)를 폰 기본 브라우저(Chrome)로 연다. 앱 WebView는 줌이 되지 않는다
     * (2026-10-23 사람 "헤드리스 브라우저고 줌인아웃도 안됨", "여전히 헤드리스 브라우저임"). `window.open`은 `shouldOverrideUrlLoading`을
     * 거치지 않고 이 WebView에 열릴 수 있어서 화면이 이 다리를 부른다. 통행증 주소만 연다.
     */
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

    /** 받은편지함의 그림 붙이기(`<input type="file">`). */
    private inner class Chooser : WebChromeClient() {
        override fun onShowFileChooser(view: WebView, callback: ValueCallback<Array<Uri>>, params: FileChooserParams): Boolean {
            upload?.onReceiveValue(null)
            upload = callback
            return runCatching { pickFile.launch(params.createIntent()) }
                .onFailure { upload = null }
                .isSuccess
        }
    }

    /** 인증서 DER의 SHA-256. `SslCertificate.x509Certificate`는 API 29부터라 저장 묶음의 DER을 읽는다. */
    private fun fingerprint(cert: SslCertificate): String? {
        val der = SslCertificate.saveState(cert).getByteArray("x509-certificate") ?: return null
        val hash = MessageDigest.getInstance("SHA-256").digest(der)
        return android.util.Base64.encodeToString(hash, android.util.Base64.URL_SAFE or android.util.Base64.NO_PADDING or android.util.Base64.NO_WRAP)
    }

    private companion object {
        const val ORIGIN = "origin"
        const val DOORS = "doors"
        const val PIN = "pin"
        const val DEVICE_COOKIE = "loom_device"
        /** 데몬 쿠키 수명(src/daemon/phone.rs `COOKIE_MAX_AGE_SECS`). */
        const val COOKIE_MAX_AGE_SECS = 400 * 24 * 60 * 60
        /** 폰 문 주소 하나를 두드리는 시간. 닿지 않는 같은 Wi-Fi 주소에서 Tailscale 주소로 넘어가는 시간이다. */
        const val PROBE_MS = 1500
        /**
         * 아무 문도 답하지 않을 때 다시 두드리는 동안. 앱을 켠 직후 폰의 Tailscale 터널이 PC와 악수하는 데, 그리고
         * `scripts/host.sh`가 데몬을 바꾸는 동안(멈춤 뒤 시작) 몇 초가 걸린다. 한 번만 두드리면 실패 카드가 떴다(2026-10-22 사람 보고).
         */
        const val REACH_MS = 10_000L
        const val RETRY_MS = 500L
        const val TAILNET = ".ts.net"
        /** 짝짓기 코드 길이(src/daemon/phone.rs와 같다). */
        const val CODE_LENGTH = 8
    }
}
