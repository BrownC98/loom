// 폰 앱(APK): PC loom이 Tailscale Serve로 내는 폰 화면을 여는 WebView 껍데기다. 근거: docs/design.md 「원격」 「Android 앱」.
pluginManagement {
    repositories {
        google()
        mavenCentral()
        gradlePluginPortal()
    }
}
dependencyResolutionManagement {
    repositories {
        google()
        mavenCentral()
    }
}
rootProject.name = "loom"
include(":app")
