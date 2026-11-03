package tech.abhashchakraborty.inyon.device

import android.Manifest
import android.app.Activity
import android.app.ActivityManager
import android.app.usage.UsageStatsManager
import android.content.ActivityNotFoundException
import android.content.BroadcastReceiver
import android.content.ClipboardManager
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.net.Uri
import android.os.Build
import android.os.PowerManager
import android.provider.Settings
import android.util.Log
import android.webkit.WebView
import androidx.activity.result.ActivityResult
import androidx.core.content.ContextCompat
import androidx.core.view.inputmethod.InputConnectionCompat
import androidx.core.view.inputmethod.InputContentInfoCompat
import androidx.webkit.WebViewFeature
import com.getcapacitor.JSArray
import com.getcapacitor.JSObject
import com.getcapacitor.PermissionState
import com.getcapacitor.Plugin
import com.getcapacitor.PluginCall
import com.getcapacitor.PluginMethod
import com.getcapacitor.annotation.ActivityCallback
import com.getcapacitor.annotation.CapacitorPlugin
import com.getcapacitor.annotation.Permission
import com.getcapacitor.annotation.PermissionCallback
import java.net.InetAddress
import java.util.concurrent.Executors
import org.json.JSONObject

/** Phone-only features: keyboard/clipboard files, background keep-alive, notifications, haptics and QR scanning. */
@CapacitorPlugin(
    name = "InyonDevice",
    permissions = [Permission(alias = InyonDevicePlugin.NOTIFICATIONS, strings = [Manifest.permission.POST_NOTIFICATIONS])],
)
class InyonDevicePlugin : Plugin() {

    private val pool = Executors.newCachedThreadPool()
    private val haptics by lazy { Haptics(context) }

    @Volatile private var launchHref: String? = null
    private var savedRendererPriority: Int? = null
    private var savedRendererWaived = false

    // the services outlive the activity, so they reach the web app through whichever plugin is live
    private val events = object : DeviceEvents.Sink {
        override fun keepAliveEnded(reason: String) = onKeepAliveEnded(reason)

        override fun answerAwake(on: Boolean) = onAnswerAwake(on)

        override fun answer(host: String, pane: String, option: Int, label: String, tag: String, at: Long) = deliverAnswer(host, pane, option, label, tag, at)
    }

    override fun load() {
        DeviceEvents.sink = events
        Notifications.ensureChannels(context)
        (bridge.webView as? InyonWebView)?.contentListener = InyonWebView.ContentListener(::onKeyboardContent)
        followDoze()
        // a notification tapped while the app wasn't running started the activity itself
        activity?.intent?.let { intent -> openedHref(intent)?.let { launchHref = it } }
        // the release smoke test looks for this line
        Log.i(TAG, "ready")
    }

    override fun handleOnNewIntent(intent: Intent) {
        val href = openedHref(intent) ?: return
        if (hasListeners("pushOpened")) {
            notifyListeners("pushOpened", JSObject().put("href", href))
        } else {
            // cold start: the web app isn't listening yet and asks with takeLaunchHref()
            launchHref = href
        }
    }

    /** The in-app path a tapped notification carries, consumed so a recreated activity doesn't open it twice. */
    private fun openedHref(intent: Intent): String? {
        val href = intent.getStringExtra(Notifications.EXTRA_HREF) ?: return null
        val nonce = intent.getStringExtra(HrefGate.EXTRA_NONCE)
        intent.removeExtra(Notifications.EXTRA_HREF)
        intent.removeExtra(HrefGate.EXTRA_NONCE)
        // the activity is exported, so any app could send this: only Inyon's own notifications get through
        return HrefGate.accept(intent.action, href, nonce, HrefGate.nonce(context))
    }

    /** Whether Inyon is on screen; the page asks as it starts, since it may have missed the events. */
    @Volatile private var onScreen = false

    override fun handleOnStart() {
        onScreen = true
        notifyListeners("appVisibility", JSObject().put("visible", true))
    }

    override fun handleOnStop() {
        // the page always thinks it's on screen (InyonWebView), so say when Inyon isn't
        onScreen = false
        notifyListeners("appVisibility", JSObject().put("visible", false))
        // Capacitor only pauses the WebView when KeepRunning is false (it isn't here), but undo any
        // pause anyway while the user asked to stay connected
        if (KeepAliveService.running) {
            val webView = bridge.webView
            webView.onResume()
            webView.resumeTimers()
        }
    }

    override fun handleOnDestroy() {
        if (DeviceEvents.sink === events) DeviceEvents.sink = null
        (bridge.webView as? InyonWebView)?.contentListener = null
        dozeReceiver?.let { runCatching { context.unregisterReceiver(it) } }
        dozeReceiver = null
        // the WebView goes with the activity, so there is nothing left to keep alive
        if (KeepAliveService.running) context.stopService(Intent(context, KeepAliveService::class.java))
        pool.shutdown()
    }

    // ---- names and frames ----------------------------------------------------------------------

    /**
     * The addresses a name points to, for the web app's plain-http check: a hub at http://<name>
     * is used only when every address is this phone, Tailscale or NetBird.
     */
    @PluginMethod
    fun lookUp(call: PluginCall) {
        val host = call.getString("host")?.trim().orEmpty()
        if (host.isEmpty() || host.length > 253) {
            call.reject("No name to look up", "BAD_HOST")
            return
        }
        pool.execute {
            val addresses = try {
                InetAddress.getAllByName(host).mapNotNull { it.hostAddress?.substringBefore('%') }
            } catch (_: Exception) {
                emptyList()
            }
            call.resolve(JSObject().put("addresses", JSArray(addresses)))
        }
    }

    /**
     * Whether Capacitor's bridge is reachable only from the app's own top frame. It is when the
     * WebView has WebMessageListener (every WebView from 2020 on); an older one makes Capacitor fall
     * back to a JavaScript interface every frame can call, and then the web app shows no other
     * site's page in a frame (agent pages and previews open in the browser instead).
     */
    @PluginMethod
    fun framesIsolated(call: PluginCall) {
        val isolated = WebViewFeature.isFeatureSupported(WebViewFeature.WEB_MESSAGE_LISTENER) && !bridge.config.isUsingLegacyBridge
        call.resolve(JSObject().put("isolated", isolated))
    }

    // ---- files and text from the keyboard and the clipboard --------------------------------

    private fun onKeyboardContent(content: InputContentInfoCompat, flags: Int): Boolean {
        if (!hasListeners("pasteFile")) return false
        var granted = false
        if (Build.VERSION.SDK_INT >= 25 && flags and InputConnectionCompat.INPUT_CONTENT_GRANT_READ_URI_PERMISSION != 0) {
            try {
                content.requestPermission()
                granted = true
            } catch (e: Exception) {
                Log.w(TAG, "keyboard content permission refused: $e")
                return false
            }
        }
        val release = granted
        val description = content.description
        val typeHint = if (description.mimeTypeCount > 0) description.getMimeType(0) else null
        val uri = content.contentUri
        pool.execute {
            try {
                notifyListeners("pasteFile", ContentReader.read(context, uri, typeHint))
            } catch (e: Exception) {
                pasteError(e)
            } finally {
                if (release) content.releasePermission()
            }
        }
        return true
    }

    private fun pasteError(e: Exception) {
        Log.w(TAG, "could not read pasted content: $e")
        val message = if (e is ContentReader.TooLargeException) e.message else "Couldn't read the pasted file"
        notifyListeners("pasteError", JSObject().put("message", message))
    }

    @PluginMethod
    fun readClipboardFile(call: PluginCall) {
        pool.execute {
            try {
                val clip = clipboard()?.primaryClip
                if (clip != null) {
                    val d = clip.description
                    for (i in 0 until clip.itemCount) {
                        val uri = clip.getItemAt(i).uri ?: continue
                        val hint = d?.let { (0 until it.mimeTypeCount).map(it::getMimeType).firstOrNull { t -> !t.startsWith("text/") && !t.contains("*") } }
                        call.resolve(ContentReader.read(context, uri, hint))
                        return@execute
                    }
                }
                call.resolve(JSObject())
            } catch (e: ContentReader.TooLargeException) {
                call.reject(e.message, "TOO_LARGE")
            } catch (e: Exception) {
                call.reject("Couldn't read the clipboard: ${e.message}", "ERROR")
            }
        }
    }

    /** The WebView refuses navigator.clipboard.readText(), so text paste comes through here. */
    @PluginMethod
    fun readClipboardText(call: PluginCall) {
        pool.execute {
            try {
                val clip = clipboard()?.primaryClip
                val text = if (clip != null && clip.itemCount > 0) clip.getItemAt(0).coerceToText(context)?.toString() else null
                call.resolve(JSObject().put("text", text ?: ""))
            } catch (e: Exception) {
                call.reject("Couldn't read the clipboard: ${e.message}", "ERROR")
            }
        }
    }

    private fun clipboard() = context.getSystemService(Context.CLIPBOARD_SERVICE) as ClipboardManager?

    // ---- staying connected in the background -----------------------------------------------

    @PluginMethod
    fun startKeepAlive(call: PluginCall) {
        try {
            ContextCompat.startForegroundService(context, KeepAliveService.intent(context, call.getString("title"), call.getString("text")))
        } catch (e: IllegalStateException) {
            // Android 12+ refuses to start a foreground service while the app is in the background
            return call.reject("Can't start the background connection right now: ${e.message}", "NOT_ALLOWED")
        }
        bridge.executeOnMainThread { rendererImportant(true) }
        call.resolve(ok())
    }

    @PluginMethod
    fun updateKeepAlive(call: PluginCall) {
        if (KeepAliveService.running) {
            Notifications.updateKeepAlive(context, call.getString("title") ?: "Inyon", call.getString("text") ?: "Connected")
        }
        call.resolve(ok())
    }

    @PluginMethod
    fun stopKeepAlive(call: PluginCall) {
        context.stopService(Intent(context, KeepAliveService::class.java))
        bridge.executeOnMainThread { rendererImportant(false) }
        call.resolve(ok())
    }

    /** An answer tapped on a notification, for the web app to send; false when nothing is listening. */
    private fun deliverAnswer(host: String, pane: String, option: Int, label: String, tag: String, at: Long): Boolean {
        if (!hasListeners("notificationAnswer")) return false
        notifyListeners("notificationAnswer", JSObject().put("host", host).put("pane", pane).put("option", option).put("label", label).put("tag", tag).put("at", at))
        return true
    }

    /** While an answer is sent, the renderer counts as much as the app, as with Stay connected. */
    private fun onAnswerAwake(on: Boolean) {
        // with Stay connected on, the renderer stays important after the answer too
        bridge.executeOnMainThread { if (on || !KeepAliveService.running) rendererImportant(on) }
    }

    private fun onKeepAliveEnded(reason: String) {
        bridge.executeOnMainThread { rendererImportant(false) }
        notifyListeners("keepAliveStopped", JSObject().put("reason", reason))
    }

    /** Keeps the renderer process as important as the app while hidden; restores the old policy after. */
    private fun rendererImportant(on: Boolean) {
        if (Build.VERSION.SDK_INT < 26) return
        val webView = bridge.webView
        if (on) {
            if (savedRendererPriority == null) {
                savedRendererPriority = webView.rendererRequestedPriority
                savedRendererWaived = webView.rendererPriorityWaivedWhenNotVisible
            }
            webView.setRendererPriorityPolicy(WebView.RENDERER_PRIORITY_IMPORTANT, false)
        } else {
            savedRendererPriority?.let {
                webView.setRendererPriorityPolicy(it, savedRendererWaived)
                savedRendererPriority = null
            }
        }
    }

    @PluginMethod
    fun isIgnoringBatteryOptimizations(call: PluginCall) {
        val pm = context.getSystemService(Context.POWER_SERVICE) as PowerManager?
        call.resolve(JSObject().put("ignoring", pm != null && pm.isIgnoringBatteryOptimizations(context.packageName)))
    }

    /**
     * Android's own "let this app run in the background?" prompt: one tap, unlike the settings list.
     * Resolves once it's answered, with whether Inyon is now unrestricted.
     */
    @PluginMethod
    fun requestBatteryExemption(call: PluginCall) {
        val pm = context.getSystemService(Context.POWER_SERVICE) as PowerManager?
        if (pm == null || pm.isIgnoringBatteryOptimizations(context.packageName)) return call.resolve(JSObject().put("ignoring", pm != null))
        val intent = Intent(Settings.ACTION_REQUEST_IGNORE_BATTERY_OPTIMIZATIONS, Uri.parse("package:${context.packageName}"))
        try {
            startActivityForResult(call, intent, "batteryExemptionResult")
        } catch (_: ActivityNotFoundException) {
            // some phones leave the prompt out: the settings list is the next best thing
            openBatterySettings(call)
        }
    }

    @ActivityCallback
    private fun batteryExemptionResult(call: PluginCall?, @Suppress("UNUSED_PARAMETER") result: ActivityResult) {
        if (call == null) return
        val pm = context.getSystemService(Context.POWER_SERVICE) as PowerManager?
        call.resolve(JSObject().put("ignoring", pm != null && pm.isIgnoringBatteryOptimizations(context.packageName)))
    }

    /** The system list where the user can exempt Inyon; the app details page where that list is missing. */
    @PluginMethod
    fun openBatterySettings(call: PluginCall) {
        bridge.executeOnMainThread {
            try {
                activity.startActivity(Intent(Settings.ACTION_IGNORE_BATTERY_OPTIMIZATION_SETTINGS))
            } catch (_: ActivityNotFoundException) {
                try {
                    activity.startActivity(Intent(Settings.ACTION_APPLICATION_DETAILS_SETTINGS, Uri.parse("package:${context.packageName}")))
                } catch (_: ActivityNotFoundException) {
                    return@executeOnMainThread call.reject("No battery settings screen on this device", "UNAVAILABLE")
                }
            }
            call.resolve(ok())
        }
    }

    // ---- Doze and battery limits, for the connection log ------------------------------------

    private var dozeReceiver: BroadcastReceiver? = null

    /** Notes when Doze starts and ends, so a link that dropped meanwhile can say so. */
    private fun followDoze() {
        val pm = context.getSystemService(Context.POWER_SERVICE) as PowerManager? ?: return
        idleClock.changed(pm.isDeviceIdleMode, System.currentTimeMillis())
        val receiver = object : BroadcastReceiver() {
            override fun onReceive(c: Context, intent: Intent) {
                idleClock.changed(pm.isDeviceIdleMode, System.currentTimeMillis())
            }
        }
        // a system broadcast, which reaches a receiver that isn't exported too
        ContextCompat.registerReceiver(context, receiver, IntentFilter(PowerManager.ACTION_DEVICE_IDLE_MODE_CHANGED), ContextCompat.RECEIVER_NOT_EXPORTED)
        dozeReceiver = receiver
    }

    /** Doze, battery saver, battery limits and Stay connected, as the web app's Power reads them. */
    @PluginMethod
    fun powerState(call: PluginCall) {
        val pm = context.getSystemService(Context.POWER_SERVICE) as PowerManager?
        val am = context.getSystemService(Context.ACTIVITY_SERVICE) as ActivityManager?
        val bucket = if (Build.VERSION.SDK_INT >= 28) (context.getSystemService(Context.USAGE_STATS_SERVICE) as UsageStatsManager?)?.appStandbyBucket else null
        val state = PowerState.Snapshot(
            idle = pm?.isDeviceIdleMode == true,
            powerSave = pm?.isPowerSaveMode == true,
            ignoringOptimizations = pm?.isIgnoringBatteryOptimizations(context.packageName) == true,
            backgroundRestricted = Build.VERSION.SDK_INT >= 28 && am?.isBackgroundRestricted == true,
            keepAlive = KeepAliveService.running,
            onScreen = onScreen,
            idleSince = idleClock.since,
            idleEnded = idleClock.ended,
            standbyBucket = bucket,
        )
        val out = JSObject()
        for ((k, v) in state.fields()) out.put(k, v)
        call.resolve(out)
    }

    // ---- this phone ------------------------------------------------------------------------

    /** The maker and model ("Nothing", "A142"), for the name a computer lists this phone under. */
    @PluginMethod
    fun deviceName(call: PluginCall) {
        call.resolve(JSObject().put("manufacturer", Build.MANUFACTURER ?: "").put("model", Build.MODEL ?: ""))
    }

    // ---- notifications and push ------------------------------------------------------------

    @PluginMethod
    fun notificationsEnabled(call: PluginCall) {
        call.resolve(JSObject().put("enabled", Notifications.canPost(context)))
    }

    @PluginMethod
    fun requestNotificationPermission(call: PluginCall) {
        if (Build.VERSION.SDK_INT < 33 || getPermissionState(NOTIFICATIONS) == PermissionState.GRANTED) {
            notificationPermissionResult(call)
        } else {
            try {
                requestPermissionForAlias(NOTIFICATIONS, call, "notificationPermissionResult")
            } catch (e: IllegalStateException) {
                // asked before the activity registered its launchers (a fast first paint): answer
                // with what is known now; the app asks again later
                notificationPermissionResult(call)
            }
        }
    }

    @PermissionCallback
    private fun notificationPermissionResult(call: PluginCall) {
        call.resolve(JSObject().put("granted", Notifications.canPost(context)))
    }

    /** An agent or machine event, with the app's logo and buttons; the same tag replaces the last one. */
    @PluginMethod
    fun postNotification(call: PluginCall) {
        call.getString("tag")?.let(AnswerService::settled)
        val choices = mutableListOf<Choice>()
        val options = call.getArray("options")
        if (options != null) {
            for (i in 0 until options.length()) {
                val o = options.optJSONObject(i) ?: continue
                val label = o.optString("label")
                if (o.has("n") && label.isNotEmpty()) choices.add(Choice(o.optInt("n"), label))
            }
        }
        val posted = Notifications.post(
            context,
            call.getString("title") ?: "Inyon",
            call.getString("body") ?: "",
            call.getString("href"),
            call.getString("tag"),
            call.getString("kind"),
            call.getString("host"),
            call.getString("pane"),
            choices,
        )
        call.resolve(ok().put("posted", posted))
    }

    @PluginMethod
    fun clearNotification(call: PluginCall) {
        call.getString("tag")?.let {
            Notifications.clear(context, it)
            AnswerService.settled(it)
        }
        call.resolve(ok())
    }

    @PluginMethod
    fun appVisibility(call: PluginCall) {
        call.resolve(JSObject().put("visible", onScreen))
    }

    @PluginMethod
    fun takeLaunchHref(call: PluginCall) {
        // the web app asks once as it starts; the release smoke test looks for this line
        Log.i(TAG, "web app started")
        val href = launchHref
        launchHref = null
        call.resolve(JSObject().put("href", href ?: JSONObject.NULL))
    }

    // ---- haptics ---------------------------------------------------------------------------

    @PluginMethod
    fun haptic(call: PluginCall) {
        val kind = call.getString("kind") ?: "tap"
        if (kind !in Haptics.KINDS) return call.reject("unknown haptic kind: $kind", "BAD_REQUEST")
        call.resolve(JSObject().put("played", haptics.play(kind, bridge.webView)))
    }

    // ---- QR codes --------------------------------------------------------------------------

    /** Opens the scanner; resolves { text }, { type: true }, { denied: true } or { cancelled: true }. */
    @PluginMethod
    fun scanQr(call: PluginCall) {
        val intent = ScannerActivity.intent(context, call.getBoolean("haptics", true) ?: true)
        bridge.executeOnMainThread { startActivityForResult(call, intent, "scanResult") }
    }

    @ActivityCallback
    private fun scanResult(call: PluginCall?, result: ActivityResult) {
        if (call == null) return
        val data = result.data
        when {
            result.resultCode != Activity.RESULT_OK || data == null -> call.resolve(cancelled())
            data.hasExtra(ScannerActivity.EXTRA_TEXT) -> call.resolve(JSObject().put("text", data.getStringExtra(ScannerActivity.EXTRA_TEXT) ?: ""))
            data.getBooleanExtra(ScannerActivity.EXTRA_TYPE, false) -> call.resolve(JSObject().put("type", true))
            data.getBooleanExtra(ScannerActivity.EXTRA_DENIED, false) -> call.resolve(JSObject().put("denied", true))
            else -> call.resolve(cancelled())
        }
    }

    companion object {
        const val NOTIFICATIONS = "notifications"
        private const val TAG = "InyonDevice"

        // kept for the process, so a recreated activity still knows when Doze last began and ended
        private val idleClock = PowerState.IdleClock()

        private fun ok(): JSObject = JSObject().put("ok", true)

        private fun failure(message: String): JSObject = JSObject().put("error", message)

        private fun cancelled(): JSObject = JSObject().put("cancelled", true)
    }
}
