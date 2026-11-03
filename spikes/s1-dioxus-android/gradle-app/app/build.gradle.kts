plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
}

android {
    namespace = "tech.abhashchakraborty.inyon.spike"
    compileSdk = 35
    defaultConfig {
        applicationId = "tech.abhashchakraborty.inyon.spike"
        minSdk = 26
        targetSdk = 35
        // set from CI like the real app (INYON_VERSION_CODE = the run number); it has to be above
        // the Capacitor build's for the Dioxus APK to install as an upgrade
        versionCode = (System.getenv("INYON_VERSION_CODE") ?: "1").toInt()
        versionName = System.getenv("INYON_VERSION") ?: "0.0.0-spike"
        ndk { abiFilters += listOf(System.getenv("INYON_ABI") ?: "arm64-v8a") }
    }
    signingConfigs {
        create("release") {
            storeFile = System.getenv("INYON_KEYSTORE")?.let { file(it) }
            storePassword = System.getenv("INYON_KEYSTORE_PASSWORD")
            keyAlias = System.getenv("INYON_KEY_ALIAS")
            keyPassword = System.getenv("INYON_KEY_PASSWORD")
        }
    }
    buildTypes {
        getByName("release") {
            isMinifyEnabled = true
            isShrinkResources = true
            signingConfig = signingConfigs.getByName("release")
            // wry's keep rules come with its Kotlin files (proguard-wry.pro)
            proguardFiles(*fileTree("src") { include("**/*.pro") }.plus(getDefaultProguardFile("proguard-android-optimize.txt")).toList().toTypedArray())
        }
    }
    buildFeatures { buildConfig = true }
    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
    kotlinOptions { jvmTarget = "17" }
    packaging { jniLibs.useLegacyPackaging = true }
}

dependencies {
    implementation("androidx.webkit:webkit:1.13.0")
    implementation("androidx.appcompat:appcompat:1.7.1")
    implementation("com.google.android.material:material:1.13.0")
}
