// AGP 9는 Kotlin을 따로 붙이지 않고 빌드한다.
plugins {
    id("com.android.application") version "9.3.3"
}

android {
    namespace = "io.github.brownc98.loom"
    compileSdk = 36
    defaultConfig {
        applicationId = "io.github.brownc98.loom"
        // Android 8: 2017년 판. 그보다 옛 폰은 Tailscale도 지원하지 않는다.
        minSdk = 26
        targetSdk = 36
        // release.yml이 판(scripts/version.sh)과 전체 커밋 수를 넘긴다. 커밋 수는 판을 올려도 줄지 않아 업데이트 설치가 된다.
        versionName = providers.gradleProperty("loomVersion").getOrElse("0.0.0")
        versionCode = providers.gradleProperty("loomVersionCode").getOrElse("1").toInt()
        // 새 판을 찾을 Release 주소. release.yml이 데몬의 LOOM_RELEASES와 같은 값을 넘긴다. 비면(개발 빌드) 찾지 않는다.
        buildConfigField("String", "RELEASES", "\"${providers.gradleProperty("loomReleases").getOrElse("")}\"")
    }
    signingConfigs {
        // 키 저장소는 저장소에 두지 않는다. release.yml이 GitHub secret에서 풀어 넘긴다.
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

// 앱 아이콘은 화면 아이콘(web/public/icon.png, 256px)과 같다. 사본을 두지 않고 빌드 때 복사한다.
val icon = tasks.register<Copy>("icon") {
    from("../../web/public/icon.png")
    into(layout.buildDirectory.dir("icon/mipmap-xxxhdpi"))
    rename { "ic_launcher.png" }
}
tasks.named("preBuild") { dependsOn(icon) }

dependencies {
    // Google 코드 스캐너: 스캔 화면을 Play 서비스가 그려 앱이 카메라 권한을 갖지 않는다.
    implementation("com.google.android.gms:play-services-code-scanner:16.1.0")
    // 뒤로 제스처(Android 16은 onBackPressed를 부르지 않는다)와 파일 고르기 결과.
    implementation("androidx.activity:activity:1.13.0")
    // Pake.kt가 src/pake.rs와 같은 값을 내는지 본다(src/test/resources/pake-vector.txt).
    testImplementation("junit:junit:4.13.2")
    constraints {
        // 코드 스캐너가 옛 fragment(1.3 아래)를 끌어온다. 그 판은 파일 고르기 결과(registerForActivityResult)를 잃는다(lint InvalidFragmentVersionForActivityResult).
        implementation("androidx.fragment:fragment:1.9.0")
    }
}
