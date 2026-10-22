package io.github.brownc98.loom

import android.content.Intent
import android.graphics.drawable.GradientDrawable
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
import kotlin.concurrent.thread

/**
 * PC loom이 Tailscale Serve로 내는 폰 화면을 연다. 화면은 PC가 그리고, 앱은 PC 주소를 모를 때(①)와
 * 닿지 못할 때(②)의 카드와 앱 새 판 띠만 그린다. 근거: docs/design.md 「Android 앱」, 목업 docs/mockups/android-app.html,
 * docs/mockups/android-update.html.
 */
class MainActivity : ComponentActivity() {
    private lateinit var web: WebView
    private lateinit var card: ScrollView
    private lateinit var banner: LinearLayout
    /** 마지막으로 새 판을 확인한 때(`SystemClock.elapsedRealtime`). 앱을 열 때와 그 뒤 6시간마다 확인한다. */
    private var checked: Long? = null
    /** 설치 허용을 켜러 설정에 간 동안 기다리는 APK. 돌아오면 설치 화면을 연다. */
    private var waiting: Pair<String, File>? = null
    private var upload: ValueCallback<Array<Uri>>? = null
    /** 앱이 연 첫 페이지가 뜨면 뒤로 기록을 비운다. 실패한 페이지로 돌아가지 않게 한다. */
    private var fresh = false
    private val pickFile = registerForActivityResult(ActivityResultContracts.StartActivityForResult()) { result ->
        upload?.onReceiveValue(WebChromeClient.FileChooserParams.parseResult(result.resultCode, result.data))
        upload = null
    }

    private val prefs by lazy { getSharedPreferences("loom", MODE_PRIVATE) }

    /** 저장한 PC 주소(`https://<PC>.ts.net`). 짝짓기 코드는 저장하지 않는다. */
    private var origin: String?
        get() = prefs.getString(ORIGIN, null)
        set(value) = prefs.edit().putString(ORIGIN, value).apply()

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        web = WebView(this).apply {
            settings.javaScriptEnabled = true
            settings.domStorageEnabled = true
            webViewClient = Client()
            webChromeClient = Chooser()
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
        val root = LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            setBackgroundColor(getColor(R.color.bg))
            addView(banner, MATCH_PARENT, WRAP_CONTENT)
            addView(page, LinearLayout.LayoutParams(MATCH_PARENT, 0, 1f))
        }
        // targetSdk 35부터 화면이 상태 줄·키보드 밑까지 그린다. 그 자리만큼 안으로 민다.
        root.setOnApplyWindowInsetsListener { view, insets ->
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
                val bars = insets.getInsets(WindowInsets.Type.systemBars() or WindowInsets.Type.ime())
                view.setPadding(bars.left, bars.top, bars.right, bars.bottom)
            }
            insets
        }
        setContentView(root)
        onBackPressedDispatcher.addCallback(object : OnBackPressedCallback(true) {
            override fun handleOnBackPressed() {
                if (card.visibility != View.VISIBLE && web.canGoBack()) web.goBack() else finish()
            }
        })
        origin?.let(::open) ?: showStart(null, "")
    }

    override fun onResume() {
        super.onResume()
        waiting?.takeIf { packageManager.canRequestPackageInstalls() }?.let { (version, apk) -> install(version, apk) }
        val now = SystemClock.elapsedRealtime()
        if (checked.let { it == null || now - it >= Release.CHECK_EVERY_MS }) {
            checked = now
            thread { Release.newer()?.let { runOnUiThread { showAvailable(it) } } }
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
        web.loadUrl(url)
    }

    /** ① 처음 실행: QR을 찍거나 PC 주소와 코드를 적는다. */
    private fun showStart(error: Int?, typed: String, typedCode: String = "") {
        val address = EditText(this).apply {
            hint = getString(R.string.address_hint)
            setText(typed)
            inputType = InputType.TYPE_CLASS_TEXT or InputType.TYPE_TEXT_VARIATION_URI
            imeOptions = EditorInfo.IME_ACTION_NEXT
            typeface = android.graphics.Typeface.MONOSPACE
            background = box(R.color.surface, R.color.line_strong)
            setPadding(dp(12), 0, dp(12), 0)
            minHeight = dp(48)
        }
        lateinit var code: EditText
        val submit = {
            val text = address.text.toString().trim()
            val pair = code.text.toString()
            val host = text.removePrefix("https://").substringBefore('/')
            when (pair.length) {
                0 -> pick(host, R.string.address_error, typed = text)
                CODE_LENGTH -> pick(host, R.string.address_error, url = "https://$host/#pair=$pair", typed = text, code = pair)
                else -> showStart(R.string.code_error, text, pair)
            }
        }
        code = codeBoxes(typedCode) { if (address.text.isNotBlank()) submit() }
        address.setOnEditorActionListener { _, _, _ -> code.requestFocus(); true }
        show(
            title = R.string.start_title,
            body = getString(R.string.start_body),
            error = error,
            button(R.string.scan, primary = true) { scan() },
            text(getString(R.string.or), R.color.ink_3, 12f).apply { gravity = Gravity.CENTER },
            address,
            code.parent as View,
            button(R.string.open, primary = false) { submit() },
        )
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
    private fun showFailed(failed: String) {
        val actions = LinearLayout(this).apply {
            addView(button(R.string.retry, primary = true) { open(failed) }, LinearLayout.LayoutParams(0, WRAP_CONTENT, 1f))
            addView(View(this@MainActivity), dp(8), 1)
            addView(button(R.string.change, primary = false) {
                origin = null
                showStart(null, "")
            }, LinearLayout.LayoutParams(0, WRAP_CONTENT, 1f))
        }
        show(R.string.failed_title, "${Uri.parse(failed).host}\n${getString(R.string.failed_body)}", null, actions)
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

    private fun scan() {
        GmsBarcodeScanning.getClient(this).startScan().addOnSuccessListener { code ->
            val url = Uri.parse(code.rawValue ?: "")
            if (url.scheme == "https") pick(url.host ?: "", R.string.qr_error, url = url.toString()) else showStart(R.string.qr_error, "")
        }.addOnFailureListener { showStart(R.string.scan_error, "") }
    }

    /** PC 주소가 tailnet 이름이면 저장하고 연다. 데몬도 `.ts.net`으로 온 요청만 폰으로 받는다(src/daemon/origin.rs). */
    private fun pick(host: String, error: Int, url: String = "https://$host/", typed: String = "", code: String = "") {
        if (!host.endsWith(TAILNET)) return showStart(error, typed, code)
        origin = "https://$host"
        open(url)
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
        /** 같은 PC의 페이지(질문 문서 등)는 쿠키가 있는 앱 안에서, 다른 사이트는 폰 브라우저로 연다. */
        override fun shouldOverrideUrlLoading(view: WebView, request: WebResourceRequest): Boolean {
            if (request.url.host == Uri.parse(origin ?: "").host) return false
            runCatching { startActivity(Intent(Intent.ACTION_VIEW, request.url)) }
            return true
        }

        override fun onPageFinished(view: WebView, url: String) {
            if (fresh && card.visibility != View.VISIBLE) view.clearHistory()
            fresh = false
        }

        override fun onReceivedError(view: WebView, request: WebResourceRequest, error: WebResourceError) {
            if (request.isForMainFrame) showFailed(request.url.toString())
        }

        /** Serve는 데몬이 꺼져 있으면 502를 낸다. */
        override fun onReceivedHttpError(view: WebView, request: WebResourceRequest, response: WebResourceResponse) {
            if (request.isForMainFrame && response.statusCode >= 500) showFailed(request.url.toString())
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

    private companion object {
        const val ORIGIN = "origin"
        const val TAILNET = ".ts.net"
        /** 짝짓기 코드 길이(src/daemon/phone.rs와 같다). */
        const val CODE_LENGTH = 8
    }
}
