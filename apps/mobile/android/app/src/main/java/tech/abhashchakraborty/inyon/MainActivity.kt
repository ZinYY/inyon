package tech.abhashchakraborty.inyon

import android.content.res.Configuration
import android.graphics.Color
import android.graphics.drawable.ColorDrawable
import android.os.Bundle
import com.getcapacitor.BridgeActivity
import tech.abhashchakraborty.inyon.device.InyonChromeClient
import tech.abhashchakraborty.inyon.device.InyonDevicePlugin
import tech.abhashchakraborty.inyon.device.Orientation
import tech.abhashchakraborty.inyon.migrate.InyonMigratePlugin
import tech.abhashchakraborty.inyon.ssh.InyonSshPlugin

class MainActivity : BridgeActivity() {

    override fun onCreate(savedInstanceState: Bundle?) {
        registerPlugin(InyonSshPlugin::class.java)
        registerPlugin(InyonDevicePlugin::class.java)
        registerPlugin(InyonMigratePlugin::class.java)
        super.onCreate(savedInstanceState)
        Orientation.apply(this)
        // the page's ground in the system's scheme, so nothing white shows between the splash and the first paint
        val night = resources.configuration.uiMode and Configuration.UI_MODE_NIGHT_MASK == Configuration.UI_MODE_NIGHT_YES
        val ground = Color.parseColor(if (night) "#1B1A18" else "#E9E8E4")
        window.setBackgroundDrawable(ColorDrawable(ground))
        bridge.webView.setBackgroundColor(ground)
        // a camera-capture photo input falls back to the gallery when the camera is refused
        bridge.webView.webChromeClient = InyonChromeClient(bridge)
        // Capacitor's cookie and HTTP plugins add JavaScript interfaces, which Android puts in every
        // frame of every page, a sandboxed agent page under /v/ included; setCookie() would let one
        // write cookies for any site. The app uses neither (both are off), so they go before the
        // first page loads.
        for (name in BRIDGELESS) bridge.webView.removeJavascriptInterface(name)
    }

    companion object {
        /** the JavaScript interfaces Capacitor adds that the app doesn't use */
        val BRIDGELESS = listOf("CapacitorCookiesAndroidInterface", "CapacitorHttpAndroidInterface")
    }

    // a foldable opening or closing changes the shortest side without recreating the activity
    override fun onConfigurationChanged(newConfig: Configuration) {
        super.onConfigurationChanged(newConfig)
        Orientation.apply(this)
    }
}
