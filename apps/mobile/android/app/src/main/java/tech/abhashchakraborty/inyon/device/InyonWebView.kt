package tech.abhashchakraborty.inyon.device

import android.content.Context
import android.text.InputType
import android.util.AttributeSet
import android.view.View
import android.view.inputmethod.EditorInfo
import android.view.inputmethod.InputConnection
import androidx.core.view.inputmethod.EditorInfoCompat
import androidx.core.view.inputmethod.InputConnectionCompat
import androidx.core.view.inputmethod.InputContentInfoCompat
import com.getcapacitor.CapacitorWebView

/**
 * Capacitor's WebView, plus rich content from the keyboard (images, GIFs, stickers, files).
 * Chromium's own input connection doesn't advertise any content types, so keyboards refuse to
 * insert them. Swapped in through res/layout/capacitor_bridge_layout_main.xml.
 */
class InyonWebView(context: Context, attrs: AttributeSet?) : CapacitorWebView(context, attrs) {

    /** Called on the IME thread; return true when the content was taken. */
    fun interface ContentListener {
        fun onContent(content: InputContentInfoCompat, flags: Int): Boolean
    }

    @Volatile
    var contentListener: ContentListener? = null

    /**
     * The page is always told its window is visible. Chromium freezes a hidden page a minute after
     * it's hidden (WebView 156 does), and a frozen page posts no alerts and answers nothing, Stay
     * connected or not. The web app learns whether Inyon is on screen from the plugin's
     * appVisibility event instead (native.ts).
     */
    override fun onWindowVisibilityChanged(visibility: Int) {
        super.onWindowVisibilityChanged(View.VISIBLE)
    }

    // the View-based wrapper routes through performReceiveContent on the IME thread
    @Suppress("DEPRECATION")
    override fun onCreateInputConnection(outAttrs: EditorInfo): InputConnection? {
        val ic = super.onCreateInputConnection(outAttrs)
        val listener = contentListener
        if (ic == null || listener == null || isPassword(outAttrs.inputType)) return ic
        val types = LinkedHashSet<String>()
        types.addAll(EditorInfoCompat.getContentMimeTypes(outAttrs))
        types.addAll(MIME_TYPES)
        EditorInfoCompat.setContentMimeTypes(outAttrs, types.toTypedArray())
        return InputConnectionCompat.createWrapper(ic, outAttrs) { content, flags, _ -> listener.onContent(content, flags) }
    }

    private companion object {
        val MIME_TYPES = listOf(
            "image/*",
            "image/png",
            "image/gif",
            "image/jpeg",
            "image/webp",
            "image/heic",
            "video/*",
            "video/mp4",
            "application/pdf",
        )

        fun isPassword(inputType: Int): Boolean {
            val cls = inputType and InputType.TYPE_MASK_CLASS
            val variation = inputType and InputType.TYPE_MASK_VARIATION
            if (cls == InputType.TYPE_CLASS_NUMBER) return variation == InputType.TYPE_NUMBER_VARIATION_PASSWORD
            return cls == InputType.TYPE_CLASS_TEXT &&
                (
                    variation == InputType.TYPE_TEXT_VARIATION_PASSWORD ||
                        variation == InputType.TYPE_TEXT_VARIATION_VISIBLE_PASSWORD ||
                        variation == InputType.TYPE_TEXT_VARIATION_WEB_PASSWORD
                    )
        }
    }
}
