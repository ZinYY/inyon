package dev.dioxus.main

import android.content.Intent
import android.content.res.Configuration
import android.os.Bundle
import android.view.View
import android.webkit.WebView
import androidx.core.view.ViewCompat
import androidx.core.view.WindowCompat
import androidx.core.view.WindowInsetsCompat
import tech.abhashchakraborty.inyon.app.Native
import tech.abhashchakraborty.inyon.device.HrefGate
import tech.abhashchakraborty.inyon.device.Notifications

// wry's Logger reads BuildConfig.DEBUG from its own package
typealias BuildConfig = tech.abhashchakraborty.inyon.BuildConfig

/**
 * The app's one window: wry's WebView with the Dioxus app in it (apps/app). It lives in wry's
 * package because the Rust side binds to `dev.dioxus.main`. The phone core runs without it.
 */
class MainActivity : WryActivity() {

    private var web: WebView? = null

    override fun onWebViewCreate(webView: WebView) {
        web = webView
    }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        insets()
        open(intent)
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        setIntent(intent)
        open(intent)
    }

    override fun onStart() {
        super.onStart()
        // the theme may have changed in Settings since
        bars(Native.night())
        web?.onResume()
        web?.resumeTimers()
        Native.onScreen(this, true)
    }

    /**
     * Off screen the page has nothing to do: the core runs in Rust and posts the alerts itself. So
     * its timers, animations and layout stop until it's back (pauseTimers is for every WebView in
     * the process, and this is the only one), and the CPU isn't woken for it.
     */
    override fun onStop() {
        Native.onScreen(this, false)
        web?.onPause()
        web?.pauseTimers()
        super.onStop()
    }

    /**
     * Android 15 draws every app under the system bars, and the keyboard no longer resizes the
     * window, so the page keeps clear of the bars, the camera cutout and the keyboard itself.
     */
    private fun insets() {
        WindowCompat.setDecorFitsSystemWindows(window, false)
        bars(Native.night())
        val content = findViewById<View>(android.R.id.content)
        ViewCompat.setOnApplyWindowInsetsListener(content) { v, w ->
            val bars = w.getInsets(WindowInsetsCompat.Type.systemBars() or WindowInsetsCompat.Type.displayCutout())
            val ime = w.getInsets(WindowInsetsCompat.Type.ime())
            v.setPadding(bars.left, bars.top, bars.right, maxOf(bars.bottom, ime.bottom))
            WindowInsetsCompat.CONSUMED
        }
    }

    /** The system switched to dark mode or out of it (the manifest keeps the window through it). */
    override fun onConfigurationChanged(newConfig: Configuration) {
        super.onConfigurationChanged(newConfig)
        bars(newConfig.uiMode and Configuration.UI_MODE_NIGHT_MASK == Configuration.UI_MODE_NIGHT_YES)
    }

    /** Light icons on the bars over a dark page, dark ones over a light page. */
    private fun bars(night: Boolean) {
        val dark = Native.darkTheme(night)
        WindowCompat.getInsetsController(window, window.decorView).apply {
            isAppearanceLightStatusBars = !dark
            isAppearanceLightNavigationBars = !dark
        }
    }

    /** A tapped notification's path, taken so a recreated window doesn't open it twice. */
    private fun open(intent: Intent?) {
        val href = intent?.getStringExtra(Notifications.EXTRA_HREF) ?: return
        val nonce = intent.getStringExtra(HrefGate.EXTRA_NONCE)
        intent.removeExtra(Notifications.EXTRA_HREF)
        intent.removeExtra(HrefGate.EXTRA_NONCE)
        // the activity is exported, so any app could send this: only Inyon's own notifications get through
        HrefGate.accept(intent.action, href, nonce, HrefGate.nonce(this))?.let(Native::openHref)
    }
}
