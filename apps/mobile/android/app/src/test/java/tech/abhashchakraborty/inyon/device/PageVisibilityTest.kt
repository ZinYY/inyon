package tech.abhashchakraborty.inyon.device

import java.io.File
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * Read from the sources, since a WebView can't run in a JVM test: the page is never told its
 * window is hidden, or Chromium freezes it a minute into the background and alerts stop, and
 * the plugin says instead when Inyon goes on and off screen, which native.ts follows.
 */
class PageVisibilityTest {
    private val device = File("src/main/java/tech/abhashchakraborty/inyon/device")

    @Test
    fun thePageIsNeverToldItsWindowIsHidden() {
        val view = File(device, "InyonWebView.kt").readText()
        assertTrue(view.contains("override fun onWindowVisibilityChanged(visibility: Int) {\n        super.onWindowVisibilityChanged(View.VISIBLE)\n    }"))
    }

    @Test
    fun theAppSaysWhenItGoesOnAndOffScreen() {
        val plugin = File(device, "InyonDevicePlugin.kt").readText()
        val start = plugin.substringAfter("override fun handleOnStart()").substringBefore("override fun")
        val stop = plugin.substringAfter("override fun handleOnStop()").substringBefore("override fun")
        assertTrue(start, start.contains("""notifyListeners("appVisibility", JSObject().put("visible", true))"""))
        assertTrue(stop, stop.contains("""notifyListeners("appVisibility", JSObject().put("visible", false))"""))
        // and the page can ask, for when it missed them
        assertTrue(start.contains("onScreen = true") && stop.contains("onScreen = false"))
        assertTrue(plugin.contains("fun appVisibility(call: PluginCall)"))
    }
}
