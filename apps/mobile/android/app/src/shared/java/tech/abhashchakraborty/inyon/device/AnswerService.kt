package tech.abhashchakraborty.inyon.device

import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.pm.ServiceInfo
import android.os.Build
import android.os.Handler
import android.os.IBinder
import android.os.Looper
import android.util.Log
import androidx.core.app.ServiceCompat
import androidx.core.content.ContextCompat

/** Answers tapped on notifications that the web app hasn't confirmed yet, by session tag. */
internal class PendingAnswers {
    data class Pending(val tag: String, val label: String, val href: String, val at: Long)

    private val byTag = LinkedHashMap<String, Pending>()

    /** A newer tap on the same session replaces the older one. */
    fun add(p: Pending) {
        byTag.remove(p.tag)
        byTag[p.tag] = p
    }

    /** The web app posted or cleared this session's notification: its answer went, or it said why not. */
    fun settle(tag: String): Boolean = byTag.remove(tag) != null

    /** Takes out the answers older than `ttl`, which the web app will now drop if they reach it. */
    fun expire(now: Long, ttl: Long): List<Pending> {
        val old = byTag.values.filter { now - it.at >= ttl }
        old.forEach { byTag.remove(it.tag) }
        return old
    }

    fun latest(): Pending? = byTag.values.lastOrNull()

    fun isEmpty() = byTag.isEmpty()
}

/**
 * Wakes the app while an answer tapped on a notification is sent. Android freezes a background
 * app a few seconds after it leaves the screen (unless Stay connected is on), and a frozen
 * WebView can't send anything: the tap would wait until the app was next opened. A short
 * foreground service, which Android allows from a notification button, thaws the app and its
 * renderer until the web app confirms or `TTL_MS` passes.
 */
class AnswerService : Service() {
    private val handler = Handler(Looper.getMainLooper())

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        val latest = synchronized(pending) { pending.latest() }
        if (latest == null) {
            stopSelf()
            return START_NOT_STICKY
        }
        // shortService (Android 14+) is meant for a few seconds of work and needs no permission
        val type = when {
            Build.VERSION.SDK_INT >= 34 -> ServiceInfo.FOREGROUND_SERVICE_TYPE_SHORT_SERVICE
            Build.VERSION.SDK_INT >= 29 -> ServiceInfo.FOREGROUND_SERVICE_TYPE_DATA_SYNC
            else -> 0
        }
        try {
            ServiceCompat.startForeground(this, Notifications.ANSWERING_ID, Notifications.answeringNotice(this, latest.label, latest.href, latest.tag), type)
        } catch (e: RuntimeException) {
            Log.w(TAG, "could not start in the foreground: $e")
        }
        current = this
        DeviceEvents.answerAwake(true)
        handler.removeCallbacksAndMessages(null)
        handler.postDelayed(::check, TTL_MS)
        return START_NOT_STICKY
    }

    /** Says which answers didn't go in time, and stops once nothing is left to wait for. */
    private fun check() {
        val (late, empty) = synchronized(pending) { pending.expire(System.currentTimeMillis(), TTL_MS) to pending.isEmpty() }
        for (p in late) Notifications.answering(this, p.tag, "Open Inyon to answer", "Inyon couldn't send “${p.label}” in time.", p.href)
        if (empty) finish() else handler.postDelayed(::check, 1000)
    }

    private fun finish() {
        handler.removeCallbacksAndMessages(null)
        if (current === this) current = null
        DeviceEvents.answerAwake(false)
        ServiceCompat.stopForeground(this, ServiceCompat.STOP_FOREGROUND_REMOVE)
        stopSelf()
    }

    /** shortService only: Android's limit is minutes, far past TTL_MS, but stop if it ever comes. */
    override fun onTimeout(startId: Int, fgsType: Int) {
        synchronized(pending) { pending.expire(Long.MAX_VALUE, 0) }
        finish()
    }

    override fun onDestroy() {
        handler.removeCallbacksAndMessages(null)
        if (current === this) current = null
        super.onDestroy()
    }

    override fun onBind(intent: Intent?): IBinder? = null

    companion object {
        private const val TAG = "InyonAnswer"

        /** How long the web app gets to send an answer; it drops taps older than its own, shorter limit. */
        const val TTL_MS = 15_000L

        private val pending = PendingAnswers()

        @Volatile private var current: AnswerService? = null

        /** Wakes the app for an answer the web app was just handed; false if Android refused. */
        fun start(ctx: Context, tag: String, label: String, href: String, at: Long): Boolean {
            synchronized(pending) { pending.add(PendingAnswers.Pending(tag, label, href, at)) }
            return try {
                ContextCompat.startForegroundService(ctx, Intent(ctx, AnswerService::class.java))
                true
            } catch (e: RuntimeException) {
                Log.w(TAG, "could not wake the app: $e")
                false
            }
        }

        /** The web app posted or cleared `tag`'s notification; stops once nothing else is waiting. */
        fun settled(tag: String) {
            val empty = synchronized(pending) { pending.settle(tag) && pending.isEmpty() }
            if (empty) current?.let { s -> s.handler.post { s.finish() } }
        }
    }
}
