package dev.dioxus.main

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.pm.ServiceInfo
import android.os.Build
import android.os.IBinder
import android.util.Log
import android.webkit.WebView
import androidx.core.app.NotificationCompat
import androidx.core.app.ServiceCompat

typealias BuildConfig = tech.abhashchakraborty.inyon.spike.BuildConfig

class MainActivity : WryActivity() {
    override fun onWebViewCreate(webView: WebView) {
        // the CI job reads the page over the DevTools protocol; a spike only
        WebView.setWebContentsDebuggingEnabled(true)
    }
}

/**
 * Inyon's keep-alive service (apps/mobile .../device/KeepAliveService.kt), cut down: a foreground
 * service with nothing to do but keep the process, and with it the WebView and the Rust runtime,
 * out of the cached state. Rust starts it over JNI (src/android.rs); there is no Kotlin caller.
 */
class KeepAliveService : Service() {
    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        val text = intent?.getStringExtra("text") ?: "Connected"
        val type = when {
            Build.VERSION.SDK_INT >= 34 -> ServiceInfo.FOREGROUND_SERVICE_TYPE_SPECIAL_USE
            Build.VERSION.SDK_INT >= 29 -> ServiceInfo.FOREGROUND_SERVICE_TYPE_DATA_SYNC
            else -> 0
        }
        try {
            ServiceCompat.startForeground(this, 1, notification(this, text), type)
            Log.i("InyonSpike", "keep-alive service is in the foreground")
        } catch (e: RuntimeException) {
            Log.w("InyonSpike", "keep-alive could not start in the foreground: $e")
            stopSelf()
        }
        return START_NOT_STICKY
    }

    override fun onBind(intent: Intent?): IBinder? = null

    private fun notification(ctx: Context, text: String): Notification {
        val nm = ctx.getSystemService(NotificationManager::class.java)
        if (Build.VERSION.SDK_INT >= 26 && nm.getNotificationChannel("connection") == null) {
            nm.createNotificationChannel(NotificationChannel("connection", "Connection", NotificationManager.IMPORTANCE_LOW))
        }
        return NotificationCompat.Builder(ctx, "connection")
            .setSmallIcon(android.R.drawable.stat_notify_sync)
            .setContentTitle("Inyon spike")
            .setContentText(text)
            .setOngoing(true)
            .setPriority(NotificationCompat.PRIORITY_LOW)
            .setCategory(NotificationCompat.CATEGORY_SERVICE)
            .setForegroundServiceBehavior(NotificationCompat.FOREGROUND_SERVICE_IMMEDIATE)
            .build()
    }
}
