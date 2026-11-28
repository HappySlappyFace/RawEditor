plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
    id("org.jetbrains.kotlin.plugin.compose")
}

// The desktop's icon font (assets/fonts/icons.ttf) as a font resource,
// copied at build time so the repository keeps one copy.
val iconFont = layout.buildDirectory.dir("generated/iconFont")
val copyIconFont by tasks.registering(Copy::class) {
    from(rootProject.file("../assets/fonts/icons.ttf"))
    into(iconFont.map { it.dir("font") })
}
tasks.named("preBuild") { dependsOn(copyIconFont) }

android {
    sourceSets["main"].res.srcDir(iconFont)
    namespace = "dev.raweditor"
    compileSdk = 35
    buildToolsVersion = "35.0.0"

    defaultConfig {
        applicationId = "dev.raweditor"
        // Android 9: Vulkan 1.1 everywhere, and what the Rust library links against.
        minSdk = 28
        targetSdk = 35
        versionCode = 1
        versionName = "0.1"
        // The Rust library is built for 64-bit ARM only (tools/android-app.sh).
        ndk { abiFilters += "arm64-v8a" }
    }

    buildTypes {
        release {
            isMinifyEnabled = false
        }
    }
    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
    kotlinOptions {
        jvmTarget = "17"
    }
    buildFeatures {
        compose = true
    }
    packaging {
        // Keep the Rust library's symbols as built; nothing to strip twice.
        jniLibs { keepDebugSymbols += "**/libraweditor.so" }
    }
}

dependencies {
    val composeBom = platform("androidx.compose:compose-bom:2024.12.01")
    implementation(composeBom)
    implementation("androidx.activity:activity-compose:1.9.3")
    implementation("androidx.compose.material3:material3")
    implementation("androidx.compose.ui:ui")
}
