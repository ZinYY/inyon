plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
}

// what build.sh copied out of dx's build: libmain.so, wry's Kotlin glue and the app's assets
val dx = layout.buildDirectory.dir("dx").get().asFile
val shared = file("../../../mobile/android/app/src/shared")

android {
    // the Capacitor app's id and namespace, so this one installs over it and the shared Kotlin
    // finds its R and BuildConfig
    namespace = "tech.abhashchakraborty.inyon"
    compileSdk = 36
    defaultConfig {
        applicationId = "tech.abhashchakraborty.inyon"
        // dx links the Rust for Android 9 (Dioxus.toml)
        minSdk = 28
        targetSdk = 36
        // CI's run number, shared with the Capacitor build, so each build installs over the last
        versionCode = (System.getenv("INYON_VERSION_CODE") ?: "1").toInt()
        versionName = System.getenv("INYON_VERSION") ?: "0.1.0"
        // the app has no translations, so the libraries' strings in other languages are dead weight
        androidResources.localeFilters += listOf("en")
        ndk { abiFilters += (System.getenv("INYON_ABIS") ?: "arm64-v8a").split(",") }
    }
    signingConfigs {
        create("release") {
            System.getenv("INYON_KEYSTORE")?.let {
                storeFile = file(it)
                storePassword = System.getenv("INYON_KEYSTORE_PASSWORD")
                keyAlias = System.getenv("INYON_KEY_ALIAS")
                keyPassword = System.getenv("INYON_KEY_PASSWORD")
            }
        }
    }
    buildTypes {
        getByName("release") {
            if (System.getenv("INYON_KEYSTORE") != null) signingConfig = signingConfigs.getByName("release")
            isMinifyEnabled = true
            isShrinkResources = true
            // wry's keep rules come with its Kotlin (proguard-wry.pro)
            proguardFiles(getDefaultProguardFile("proguard-android-optimize.txt"), "proguard-rules.pro", File(dx, "kotlin/dev/dioxus/main/proguard-wry.pro"))
        }
    }
    sourceSets {
        getByName("main") {
            java.srcDirs(shared.resolve("java"), File(dx, "kotlin"))
            res.srcDirs(shared.resolve("res"))
            jniLibs.srcDirs(File(dx, "jniLibs"))
            assets.srcDirs(File(dx, "assets"))
        }
    }
    buildFeatures { buildConfig = true }
    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_21
        targetCompatibility = JavaVersion.VERSION_21
    }
    packaging {
        // wry and the Rust library expect the library on disk, as dx's own project does
        jniLibs.useLegacyPackaging = true
        // gson, error_prone_annotations and jspecify (pulled in by the SSH and secrets libraries) all ship one
        resources.excludes += "META-INF/versions/9/module-info.class"
    }
}

kotlin {
    compilerOptions {
        jvmTarget.set(org.jetbrains.kotlin.gradle.dsl.JvmTarget.JVM_21)
    }
}

// the Capacitor app's versions (apps/mobile/android/variables.gradle), less Capacitor and the camera
dependencies {
    implementation("androidx.appcompat:appcompat:1.7.1")
    implementation("androidx.core:core:1.17.0")
    implementation("androidx.webkit:webkit:1.14.0")
    // sshlib's plain `tink` jar holds the same classes as security-crypto's `tink-android`
    implementation("org.connectbot:sshlib:2.2.50") {
        exclude(group = "com.google.crypto.tink", module = "tink")
    }
    implementation("com.google.crypto.tink:tink-android:1.23.0")
    implementation("androidx.security:security-crypto:1.1.0")
}
