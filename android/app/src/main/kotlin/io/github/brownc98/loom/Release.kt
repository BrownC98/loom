package io.github.brownc98.loom

import android.content.pm.PackageManager
import java.io.File
import java.io.InputStream
import java.net.HttpURLConnection
import java.net.URL
import org.json.JSONObject

internal object Release {
    const val CHECK_EVERY_MS = 6 * 60 * 60 * 1000L

    fun newer(): String? {
        if (BuildConfig.RELEASES.isEmpty()) return null
        val version = runCatching {
            JSONObject(get("${BuildConfig.RELEASES}/latest/download/latest.json").use { it.bufferedReader().readText() }).getString("version")
        }.getOrNull() ?: return null
        if (!newer(version, BuildConfig.VERSION_NAME)) return null
        return version.takeIf { runCatching { open(apkUrl(it), "HEAD").disconnect() }.isSuccess }
    }

    fun download(version: String, to: File, progress: (Int) -> Unit) {
        to.parentFile?.mkdirs()
        val connection = open(apkUrl(version))
        val total = connection.contentLengthLong
        connection.inputStream.use { input ->
            to.outputStream().use { output ->
                val buf = ByteArray(64 * 1024)
                var done = 0L
                var shown = -2
                while (true) {
                    val n = input.read(buf)
                    if (n < 0) break
                    output.write(buf, 0, n)
                    done += n
                    val percent = if (total > 0) (done * 100 / total).toInt() else -1
                    if (percent != shown) progress(percent).also { shown = percent }
                }
                if (total > 0 && done != total) error("short download: $done of $total")
            }
        }
    }

    fun verified(packages: PackageManager, apk: File, packageName: String, version: String): Boolean {
        @Suppress("DEPRECATION")
        val info = packages.getPackageArchiveInfo(apk.path, 0) ?: return false
        return info.packageName == packageName && info.versionName == version
    }

    private fun get(url: String): InputStream = open(url).inputStream

    private fun apkUrl(version: String) = "${BuildConfig.RELEASES}/download/v$version/loom_$version.apk"

    private fun open(url: String, method: String = "GET") = (URL(url).openConnection() as HttpURLConnection).apply {
        requestMethod = method
        connectTimeout = 15_000
        readTimeout = 30_000
        if (responseCode != HttpURLConnection.HTTP_OK) error("HTTP $responseCode: $url")
    }

    fun newer(theirs: String, mine: String): Boolean {
        fun parts(version: String) = version.split('.').map { it.toIntOrNull() }.takeIf { it.size == 3 && null !in it }?.map { it!! }
        val (t, m) = (parts(theirs) ?: return false) to (parts(mine) ?: return false)
        return t.zip(m).firstOrNull { (a, b) -> a != b }?.let { (a, b) -> a > b } ?: false
    }
}
