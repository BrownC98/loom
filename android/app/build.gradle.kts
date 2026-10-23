plugins {
    id("com.android.application") version "9.3.3"
}

android {
    namespace = "io.github.brownc98.loom"
    compileSdk = 36
    defaultConfig {
        applicationId = "io.github.brownc98.loom"
        minSdk = 26
        targetSdk = 36
        versionName = providers.gradleProperty("loomVersion").getOrElse("0.0.0")
        versionCode = providers.gradleProperty("loomVersionCode").getOrElse("1").toInt()
        buildConfigField("String", "RELEASES", "\"${providers.gradleProperty("loomReleases").getOrElse("")}\"")
    }
    signingConfigs {
        create("release") {
            storeFile = System.getenv("LOOM_KEYSTORE")?.let(::file)
            storePassword = System.getenv("LOOM_KEYSTORE_PASSWORD")
            keyAlias = "loom"
            keyPassword = System.getenv("LOOM_KEYSTORE_PASSWORD")
        }
    }
    buildTypes {
        release {
            signingConfig = signingConfigs.getByName("release")
        }
    }
    buildFeatures { buildConfig = true }
    sourceSets["main"].res.directories.add(layout.buildDirectory.dir("icon").get().asFile.path)
}

val icon = tasks.register<Copy>("icon") {
    from("../../web/public/icon.png")
    into(layout.buildDirectory.dir("icon/mipmap-xxxhdpi"))
    rename { "ic_launcher.png" }
}
tasks.named("preBuild") { dependsOn(icon) }

dependencies {
    implementation("com.google.android.gms:play-services-code-scanner:16.1.0")
    implementation("androidx.activity:activity:1.13.0")
    testImplementation("junit:junit:4.13.2")
    constraints {
        implementation("androidx.fragment:fragment:1.9.0")
    }
}
