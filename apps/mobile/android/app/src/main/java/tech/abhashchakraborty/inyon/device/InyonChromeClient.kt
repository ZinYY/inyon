package tech.abhashchakraborty.inyon.device

import android.Manifest
import android.content.ActivityNotFoundException
import android.content.pm.PackageManager
import android.net.Uri
import android.webkit.ValueCallback
import android.webkit.WebChromeClient
import android.webkit.WebView
import androidx.activity.result.contract.ActivityResultContracts
import androidx.core.content.ContextCompat
import com.getcapacitor.Bridge
import com.getcapacitor.BridgeWebChromeClient

/**
 * Capacitor's client, except that a photo input with `capture` opens the gallery when the camera
 * is refused, instead of doing nothing. The app declares CAMERA for its QR scanner, and with it
 * declared the system camera app can only be launched once the user allows it.
 *
 * Must be created in the activity's onCreate, since it registers for activity results.
 */
class InyonChromeClient(private val bridge: Bridge) : BridgeWebChromeClient(bridge) {

    private var pending: Pending? = null

    private class Pending(val webView: WebView, val callback: ValueCallback<Array<Uri>?>, val params: WebChromeClient.FileChooserParams)

    private val askCamera =
        bridge.registerForActivityResult(ActivityResultContracts.RequestPermission()) { granted ->
            val p = pending ?: return@registerForActivityResult
            pending = null
            if (granted) capture(p) else pickInstead(p)
        }

    private val pick =
        bridge.registerForActivityResult(ActivityResultContracts.StartActivityForResult()) { result ->
            val p = pending ?: return@registerForActivityResult
            pending = null
            p.callback.onReceiveValue(WebChromeClient.FileChooserParams.parseResult(result.resultCode, result.data))
        }

    override fun onShowFileChooser(webView: WebView, callback: ValueCallback<Array<Uri>?>, params: WebChromeClient.FileChooserParams): Boolean {
        val wantsCamera = params.isCaptureEnabled && params.acceptTypes.any { it == "image/*" || it == "video/*" }
        if (!wantsCamera || hasCamera()) return super.onShowFileChooser(webView, callback, params)
        // a chooser left open by an earlier tap gets an empty answer, as the WebView expects
        pending?.callback?.onReceiveValue(null)
        pending = Pending(webView, callback, params)
        askCamera.launch(Manifest.permission.CAMERA)
        return true
    }

    private fun capture(p: Pending) {
        super.onShowFileChooser(p.webView, p.callback, p.params)
    }

    private fun pickInstead(p: Pending) {
        try {
            pending = p
            pick.launch(p.params.createIntent())
        } catch (_: ActivityNotFoundException) {
            pending = null
            p.callback.onReceiveValue(null)
        }
    }

    private fun hasCamera() = ContextCompat.checkSelfPermission(bridge.context, Manifest.permission.CAMERA) == PackageManager.PERMISSION_GRANTED
}
