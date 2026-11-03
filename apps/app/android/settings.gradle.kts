// The Android app from 1.0: dx compiles the Rust (apps/app) into libmain.so and writes wry's Kotlin
// glue; this project packages them with the Kotlin the Capacitor app shares (the services,
// notifications, SSH and launcher icons in apps/mobile/android/app/src/shared). It owns the
// version code, signing and R8, which dx's own generated project hardcodes. build.sh runs it.
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
rootProject.name = "inyon"
include(":app")
