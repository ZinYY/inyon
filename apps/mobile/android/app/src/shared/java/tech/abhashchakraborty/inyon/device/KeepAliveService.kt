package tech.abhashchakraborty.inyon.device

import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.pm.ServiceInfo
import android.os.Build
import android.os.IBinder
import android.os.PowerManager
import android.util.Log
import androidx.core.app.ServiceCompat

/**
 * A foreground service with nothing to do but exist: while it runs, Android keeps the app process
 * (and with it the WebView's renderer) out of the cached state, so sockets stay open and the web
 * app keeps getting live updates in the background.
 */
class KeepAliveService : Service() {

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        val title = intent?.getStringExtra(EXTRA_TITLE) ?: "Inyon"
        val text = intent?.getStringExtra(EXTRA_TEXT) ?: "Connected"
        // specialUse has no daily limit; dataSync before Android 14, where it has none either
        val type = when {
            Build.VERSION.SDK_INT >= 34 -> ServiceInfo.FOREGROUND_SERVICE_TYPE_SPECIAL_USE
            Build.VERSION.SDK_INT >= 29 -> ServiceInfo.FOREGROUND_SERVICE_TYPE_DATA_SYNC
            else -> 0
        }
        try {
            ServiceCompat.startForeground(this, Notifications.KEEP_ALIVE_ID, Notifications.keepAlive(this, title, text), type)
            if (wakeLock == null) {
                val pm = getSystemService(Context.POWER_SERVICE) as PowerManager
                wakeLock = pm.newWakeLock(PowerManager.PARTIAL_WAKE_LOCK, "inyon:incoming").apply { setReferenceCounted(false) }
            }
            running = true
        } catch (e: RuntimeException) {
            // e.g. the daily dataSync allowance (Android 15+) is used up
            Log.w(TAG, "could not start in the foreground: $e")
            stop("error")
        }
        // if the process dies the WebView is gone too, so there's nothing to restart for
        return START_NOT_STICKY
    }

    /** Android 15+, dataSync only (so before Android 14 never): 6 hours a day, reset when the app opens. */
    override fun onTimeout(startId: Int, fgsType: Int) {
        stop("timeout")
    }

    override fun onDestroy() {
        running = false
        synchronized(lockGuard) {
            wakeLock?.let { if (it.isHeld) it.release() }
            wakeLock = null
        }
        super.onDestroy()
    }

    override fun onBind(intent: Intent?): IBinder? = null

    private fun stop(reason: String) {
        running = false
        ServiceCompat.stopForeground(this, ServiceCompat.STOP_FOREGROUND_REMOVE)
        stopSelf()
        DeviceEvents.keepAliveEnded(reason)
    }

    companion object {
        private const val TAG = "InyonKeepAlive"
        private const val EXTRA_TITLE = "title"
        private const val EXTRA_TEXT = "text"

        /** how long the phone stays awake after a machine sends something: enough to read it and post the alert */
        private const val AWAKE_MS = 10_000L

        @Volatile
        var running = false
            private set

        @Volatile
        private var wakeLock: PowerManager.WakeLock? = null

        /** nudge() runs on SSH threads while onDestroy releases the lock on main: both hold this */
        private val lockGuard = Any()

        @Volatile
        private var nudgedAt = 0L

        /**
         * A machine sent something: stay awake a few seconds so the app can act on it (an agent
         * finishing, say) before the phone sleeps again. Each call pushes the deadline back.
         */
        fun nudge() {
            synchronized(lockGuard) {
                // once the service is gone there is nothing to hold awake
                val lock = wakeLock ?: return
                val now = System.currentTimeMillis()
                // a busy stream calls this for every batch of lines; once a second is plenty
                if (now - nudgedAt < 1000) return
                nudgedAt = now
                try {
                    lock.acquire(AWAKE_MS)
                } catch (_: RuntimeException) {
                }
            }
        }

        fun intent(ctx: Context, title: String?, text: String?): Intent =
            Intent(ctx, KeepAliveService::class.java).putExtra(EXTRA_TITLE, title).putExtra(EXTRA_TEXT, text)
    }
}
