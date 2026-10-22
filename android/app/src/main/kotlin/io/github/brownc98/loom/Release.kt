package io.github.brownc98.loom

import android.content.pm.PackageManager
import java.io.File
import java.io.InputStream
import java.net.HttpURLConnection
import java.net.URL
import org.json.JSONObject

/**
 * 앱 새 판. 데몬(src/update.rs)과 같은 Release의 `latest.json`을 보고, 새 판이면 그 Release의 `loom_<판>.apk`를 받는다.
 * 네트워크를 쓰므로 UI 스레드 밖에서 부른다. 근거: docs/design.md 「Android 앱」, 목업 docs/mockups/android-update.html.
 */
internal object Release {
    /** 데몬과 같은 간격(src/update.rs `CHECK_EVERY`). */
    const val CHECK_EVERY_MS = 6 * 60 * 60 * 1000L

    /**
     * 새 판이고 그 APK가 올라와 있으면 그 판. 같거나 옛 판, 확인할 수 없음, 개발 빌드(`loomReleases` 없음)는 null.
     * release.yml은 `latest.json`을 올린 몇 분 뒤에 APK를 올린다. 그 사이에 알리면 받기가 404로 실패했다(2026-10-23 사람 "업데이트 시도 시 받지 못했다라는 경우가 뜬다").
     */
    fun newer(): String? {
        if (BuildConfig.RELEASES.isEmpty()) return null
        val version = runCatching {
            JSONObject(get("${BuildConfig.RELEASES}/latest/download/latest.json").use { it.bufferedReader().readText() }).getString("version")
        }.getOrNull() ?: return null
        if (!newer(version, BuildConfig.VERSION_NAME)) return null
        return version.takeIf { runCatching { open(apkUrl(it), "HEAD").disconnect() }.isSuccess }
    }

    /** `to`에 받는다. `progress`는 0~100, 크기를 모르면 -1이다. 실패하면 예외를 던진다. */
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

    /** 받은 파일이 이 앱의 그 판인지 본다. 다른 서명은 Android 설치 화면이 거절한다. */
    fun verified(packages: PackageManager, apk: File, packageName: String, version: String): Boolean {
        @Suppress("DEPRECATION") // 새 오버로드(PackageInfoFlags)는 Android 13부터다.
        val info = packages.getPackageArchiveInfo(apk.path, 0) ?: return false
        return info.packageName == packageName && info.versionName == version
    }

    private fun get(url: String): InputStream = open(url).inputStream

    private fun apkUrl(version: String) = "${BuildConfig.RELEASES}/download/v$version/loom_$version.apk"

    private fun open(url: String, method: String = "GET") = (URL(url).openConnection() as HttpURLConnection).apply {
        requestMethod = method
        // 폰 망이 느려도 기다리되, 끊긴 연결에 멈추지 않는다.
        connectTimeout = 15_000
        readTimeout = 30_000
        if (responseCode != HttpURLConnection.HTTP_OK) error("HTTP $responseCode: $url")
    }

    /** `MAJOR.MINOR.PATCH`를 수로 비교한다(src/launch.rs `newer`와 같다). */
    fun newer(theirs: String, mine: String): Boolean {
        fun parts(version: String) = version.split('.').map { it.toIntOrNull() }.takeIf { it.size == 3 && null !in it }?.map { it!! }
        val (t, m) = (parts(theirs) ?: return false) to (parts(mine) ?: return false)
        return t.zip(m).firstOrNull { (a, b) -> a != b }?.let { (a, b) -> a > b } ?: false
    }
}
