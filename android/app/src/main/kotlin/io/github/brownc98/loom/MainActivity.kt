package io.github.brownc98.loom

import android.content.Intent
import android.graphics.drawable.GradientDrawable
import android.net.Uri
import android.os.Build
import android.os.Bundle
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
import android.widget.ScrollView
import android.widget.TextView
import androidx.activity.ComponentActivity
import androidx.activity.OnBackPressedCallback
import androidx.activity.result.contract.ActivityResultContracts
import com.google.mlkit.vision.codescanner.GmsBarcodeScanning

/**
 * PC loom이 Tailscale Serve로 내는 폰 화면을 연다. 화면은 PC가 그리고, 앱은 PC 주소를 모를 때(①)와
 * 닿지 못할 때(②)의 카드만 그린다. 근거: docs/design.md 「Android 앱」, 목업 docs/mockups/android-app.html.
 */
class MainActivity : ComponentActivity() {
    private lateinit var web: WebView
    private lateinit var card: ScrollView
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
        val root = FrameLayout(this).apply {
            setBackgroundColor(getColor(R.color.bg))
            addView(web, MATCH_PARENT, MATCH_PARENT)
            addView(card, MATCH_PARENT, MATCH_PARENT)
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

    /** ① 처음 실행: QR을 찍거나 PC 주소를 적는다. */
    private fun showStart(error: Int?, typed: String) {
        val address = EditText(this).apply {
            hint = getString(R.string.address_hint)
            setText(typed)
            inputType = InputType.TYPE_CLASS_TEXT or InputType.TYPE_TEXT_VARIATION_URI
            imeOptions = EditorInfo.IME_ACTION_GO
            typeface = android.graphics.Typeface.MONOSPACE
            background = box(R.color.surface, R.color.line_strong)
            setPadding(dp(12), 0, dp(12), 0)
            minHeight = dp(48)
        }
        val submit = {
            val text = address.text.toString().trim()
            pick(text.removePrefix("https://").substringBefore('/'), R.string.address_error, typed = text)
        }
        address.setOnEditorActionListener { _, _, _ -> submit(); true }
        show(
            title = R.string.start_title,
            body = getString(R.string.start_body),
            error = error,
            button(R.string.scan, primary = true) { scan() },
            text(getString(R.string.or), R.color.ink_3, 12f).apply { gravity = Gravity.CENTER },
            address,
            button(R.string.open, primary = false) { submit() },
        )
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

    private fun scan() {
        GmsBarcodeScanning.getClient(this).startScan().addOnSuccessListener { code ->
            val url = Uri.parse(code.rawValue ?: "")
            if (url.scheme == "https") pick(url.host ?: "", R.string.qr_error, url = url.toString()) else showStart(R.string.qr_error, "")
        }.addOnFailureListener { showStart(R.string.scan_error, "") }
    }

    /** PC 주소가 tailnet 이름이면 저장하고 연다. 데몬도 `.ts.net`으로 온 요청만 폰으로 받는다(src/daemon/origin.rs). */
    private fun pick(host: String, error: Int, url: String = "https://$host/", typed: String = "") {
        if (!host.endsWith(TAILNET)) return showStart(error, typed)
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
    }
}
