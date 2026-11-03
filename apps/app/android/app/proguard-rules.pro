# R8 rules for the release build; wry's own (proguard-wry.pro) and the libraries' consumer rules
# are applied on top.

# readable stack traces with the mapping file CI keeps next to the APK
-keepattributes SourceFile,LineNumberTable
-renamesourcefileattribute SourceFile
-keepattributes *Annotation*,Signature,InnerClasses,EnclosingMethod

# Rust calls these by name over JNI (apps/app/src/android) and registers the native ones
-keep class tech.abhashchakraborty.inyon.app.Native { *; }

# instantiated by Android from the manifest
-keep class tech.abhashchakraborty.inyon.app.InyonApp { <init>(); }
-keep class dev.dioxus.main.MainActivity { <init>(); }
-keep class tech.abhashchakraborty.inyon.device.KeepAliveService { <init>(); }
-keep class tech.abhashchakraborty.inyon.device.AnswerService { <init>(); }
-keep class tech.abhashchakraborty.inyon.device.NotificationActions { <init>(); }

# sshlib registers a JCA provider that names its Ed25519 classes by package at runtime, and
# its algorithm tables are looked up by string; keep the library as it is
-keep class com.trilead.ssh2.** { *; }
-keep class org.connectbot.** { *; }
