# R8 rules for the release build. The libraries' own consumer rules (Capacitor, Play services'
# ML Kit, AndroidX, Tink) are applied on top of these.

# readable stack traces with the mapping file CI keeps next to the APK
-keepattributes SourceFile,LineNumberTable
-renamesourcefileattribute SourceFile
-keepattributes *Annotation*,Signature,InnerClasses,EnclosingMethod

# Capacitor finds plugins and their methods by reflection (annotations, getDeclaredMethods,
# permission callbacks by name) and loads Cordova classes by name; both are small, keep them whole
-keep class com.getcapacitor.** { *; }
-keep class org.apache.cordova.** { *; }
-keep @com.getcapacitor.annotation.CapacitorPlugin class * { *; }

# our plugins: every @PluginMethod and @PermissionCallback is called by name from the bridge
-keep class tech.abhashchakraborty.inyon.device.InyonDevicePlugin { *; }
-keep class tech.abhashchakraborty.inyon.ssh.InyonSshPlugin { *; }
-keep class tech.abhashchakraborty.inyon.migrate.InyonMigratePlugin { *; }
# the app that installs over this one reads and discards the upgrade file through it, by name
-keep class tech.abhashchakraborty.inyon.migrate.MigrationExport { *; }

# instantiated by Android from the manifest and the bridge layout
-keep class tech.abhashchakraborty.inyon.MainActivity { <init>(); }
-keep class tech.abhashchakraborty.inyon.device.KeepAliveService { <init>(); }
-keep class tech.abhashchakraborty.inyon.device.AnswerService { <init>(); }
-keep class tech.abhashchakraborty.inyon.device.InyonWebView { <init>(android.content.Context, android.util.AttributeSet); }
-keep class tech.abhashchakraborty.inyon.device.ScannerActivity { <init>(); }
-keep class tech.abhashchakraborty.inyon.device.ScannerOverlay { <init>(android.content.Context, android.util.AttributeSet); }
-keep class tech.abhashchakraborty.inyon.device.NotificationActions { <init>(); }

# sshlib registers a JCA provider that names its Ed25519 classes by package at runtime, and
# its algorithm tables are looked up by string; keep the library as it is
-keep class com.trilead.ssh2.** { *; }
-keep class org.connectbot.** { *; }
