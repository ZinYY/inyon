package tech.abhashchakraborty.inyon.app

import android.Manifest
import android.app.Activity
import android.content.Context
import android.content.pm.PackageManager
import android.content.res.Configuration
import android.os.Build
import android.os.Handler
import android.os.Looper
import androidx.core.app.ActivityCompat
import androidx.core.content.ContextCompat
import org.json.JSONObject
import tech.abhashchakraborty.inyon.device.AnswerService
import tech.abhashchakraborty.inyon.device.Choice
import tech.abhashchakraborty.inyon.device.DeviceEvents
import tech.abhashchakraborty.inyon.device.KeepAliveService
import tech.abhashchakraborty.inyon.device.Notifications
import tech.abhashchakraborty.inyon.migrate.MigrationExport
import tech.abhashchakraborty.inyon.ssh.SshMethods
import java.io.File
import java.lang.ref.WeakReference

/**
 * The bridge to the Rust side (libmain.so, apps/app/src/android). The `external` functions are
 * Rust's; the others Rust calls, from its own threads, for what only Android can do: SSH (the
 * Capacitor app's SshMethods), notifications and the keep-alive service.
 */
object Native {
    init {
        System.loadLibrary("main")
    }

    private lateinit var app: Context
    private lateinit var sshMethods: SshMethods
    private val main = Handler(Looper.getMainLooper())

    /** The window on screen, if any: asking for a permission needs one. */
    @Volatile private var screen: WeakReference<Activity>? = null

    @Volatile private var askedNotifications = false

    fun start(ctx: Context) {
        app = ctx.applicationContext
        Notifications.ensureChannels(app)
        sshMethods = SshMethods(app) { event, data -> sshEvent(event, data.toString()) }
        DeviceEvents.sink = object : DeviceEvents.Sink {
            override fun keepAliveEnded(reason: String) = Native.keepAliveEnded(reason)

            // the core runs outside any window, so AnswerService keeping the process up is enough
            override fun answerAwake(on: Boolean) {}

            override fun answer(host: String, pane: String, option: Int, label: String, tag: String, at: Long) =
                Native.answer(JSONObject().put("host", host).put("pane", pane).put("option", option).put("label", label).put("tag", tag).put("at", at).toString())
        }
        startCore(File(app.filesDir, "inyon").path, Build.MANUFACTURER ?: "", Build.MODEL ?: "")
    }

    /** The window came on screen or left it. */
    fun onScreen(activity: Activity, on: Boolean) {
        screen = if (on) WeakReference(activity) else null
        visible(on)
    }

    // ---------- Rust's ----------

    @JvmStatic external fun startCore(dir: String, maker: String, model: String)

    @JvmStatic external fun visible(on: Boolean)

    @JvmStatic external fun sshReply(id: Long, ok: Boolean, json: String)

    @JvmStatic external fun sshEvent(name: String, json: String)

    @JvmStatic external fun keepAliveEnded(reason: String)

    @JvmStatic external fun openHref(href: String)

    @JvmStatic external fun darkTheme(night: Boolean): Boolean

    @JvmStatic external fun answer(json: String): Boolean

    // ---------- called from Rust ----------

    /** An SSH method; the answer goes back through [sshReply] with the call's id. */
    @JvmStatic
    fun ssh(id: Long, method: String, args: String) {
        val data = try {
            JSONObject(args)
        } catch (e: Exception) {
            JSONObject()
        }
        sshMethods.call(
            method,
            data,
            object : SshMethods.Reply {
                override fun resolve(result: JSONObject) = sshReply(id, true, result.toString())

                override fun reject(message: String, code: String) = sshReply(id, false, JSONObject().put("message", message).put("code", code).toString())
            },
        )
    }

    /** A notification as apps/app/src/notify.rs writes it; the same tag replaces the last one. */
    @JvmStatic
    fun postNotification(json: String): Boolean {
        val n = JSONObject(json)
        // a session's notification replaced: an answer sent from it went, or failed and says why
        if (n.has("tag")) AnswerService.settled(n.optString("tag"))
        val choices = mutableListOf<Choice>()
        n.optJSONArray("options")?.let { o ->
            for (i in 0 until o.length()) {
                val c = o.optJSONObject(i) ?: continue
                val label = c.optString("label")
                // a number past Int.MAX_VALUE would wrap to a negative one the core refuses: no button
                val num = c.optLong("n", -1L)
                if (num in 0L..Int.MAX_VALUE.toLong() && label.isNotEmpty()) choices.add(Choice(num.toInt(), label))
            }
        }
        fun opt(k: String) = if (n.has(k)) n.optString(k) else null
        return Notifications.post(app, opt("title") ?: "Inyon", opt("body") ?: "", opt("href"), opt("tag"), opt("kind"), opt("host"), opt("pane"), choices)
    }

    @JvmStatic
    fun clearNotification(tag: String) {
        Notifications.clear(app, tag)
        AnswerService.settled(tag)
    }

    /** Starts Stay connected; null when it's running, or why Android refused. */
    @JvmStatic
    fun startKeepAlive(title: String, text: String): String? {
        if (KeepAliveService.running) return null
        return try {
            ContextCompat.startForegroundService(app, KeepAliveService.intent(app, title, text))
            null
        } catch (e: IllegalStateException) {
            // Android 12+ refuses to start a foreground service while the app is in the background
            e.message ?: "not allowed"
        }
    }

    /** Asks once per process for notifications (Android 13+), while a window is on screen. */
    @JvmStatic
    fun askNotifications() {
        if (Build.VERSION.SDK_INT < 33 || askedNotifications) return
        if (ContextCompat.checkSelfPermission(app, Manifest.permission.POST_NOTIFICATIONS) == PackageManager.PERMISSION_GRANTED) return
        val activity = screen?.get() ?: return
        // checked and set on the main thread, so a window closing first doesn't use up the one ask,
        // and two calls close together still ask once
        main.post {
            if (!askedNotifications && !activity.isFinishing) {
                askedNotifications = true
                ActivityCompat.requestPermissions(activity, arrayOf(Manifest.permission.POST_NOTIFICATIONS), 1)
            }
        }
    }

    /** The Capacitor app's sealed upgrade file, opened, or null (MigrationExport). */
    @JvmStatic
    fun readMigration(): String? = MigrationExport.read(app)?.toString(Charsets.UTF_8)

    @JvmStatic
    fun discardMigration() = MigrationExport.discard(app)

    /** Whether the system is in dark mode now. */
    @JvmStatic
    fun night(): Boolean = app.resources.configuration.uiMode and Configuration.UI_MODE_NIGHT_MASK == Configuration.UI_MODE_NIGHT_YES
}
