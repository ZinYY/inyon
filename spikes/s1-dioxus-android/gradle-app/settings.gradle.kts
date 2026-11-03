// The app's own Gradle project: dx builds the Rust library and wry's Kotlin glue, this packages
// them (ci/own-gradle.sh copies them in). It owns versionCode, signing and R8, which dx's
// generated project hardcodes.
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
rootProject.name = "inyon-spike"
include(":app")
