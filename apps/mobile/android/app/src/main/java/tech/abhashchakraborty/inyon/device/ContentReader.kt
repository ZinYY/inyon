package tech.abhashchakraborty.inyon.device

import android.content.ContentResolver
import android.content.Context
import android.net.Uri
import android.provider.OpenableColumns
import android.util.Base64
import android.webkit.MimeTypeMap
import com.getcapacitor.JSObject
import java.io.ByteArrayOutputStream
import java.io.IOException
import java.io.InputStream
import java.text.SimpleDateFormat
import java.util.Date
import java.util.Locale

/** Reads a file handed over by the keyboard or the clipboard into `{ name, type, data }`. */
internal object ContentReader {

    private const val MAX_BYTES = 25 * 1024 * 1024

    class TooLargeException : IOException("The file is larger than 25 MB")

    fun read(ctx: Context, uri: Uri, typeHint: String?): JSObject {
        if (uri.scheme != ContentResolver.SCHEME_CONTENT) throw IOException("unsupported uri: ${uri.scheme}")
        val authority = uri.authority
        // a clip pointing at one of our own providers would read our private files
        if (authority == null || authority.startsWith(ctx.packageName)) throw IOException("unsupported uri")

        val cr = ctx.contentResolver
        var type = cr.getType(uri)
        if ((type == null || type == "application/octet-stream") && typeHint != null) type = typeHint
        if (type == null) type = "application/octet-stream"

        var name: String? = null
        var size = -1L
        try {
            cr.query(uri, arrayOf(OpenableColumns.DISPLAY_NAME, OpenableColumns.SIZE), null, null, null)?.use { c ->
                if (c.moveToFirst()) {
                    val n = c.getColumnIndex(OpenableColumns.DISPLAY_NAME)
                    val s = c.getColumnIndex(OpenableColumns.SIZE)
                    if (n >= 0 && !c.isNull(n)) name = c.getString(n)
                    if (s >= 0 && !c.isNull(s)) size = c.getLong(s)
                }
            }
        } catch (_: RuntimeException) {
            // some keyboards' providers don't answer queries; the name and size are optional
        }
        if (size > MAX_BYTES) throw TooLargeException()

        val bytes = (cr.openInputStream(uri) ?: throw IOException("could not open the file")).use(::readCapped)

        return JSObject()
            .put("name", name?.ifEmpty { null } ?: defaultName(type))
            .put("type", type)
            .put("data", Base64.encodeToString(bytes, Base64.NO_WRAP))
    }

    private fun readCapped(input: InputStream): ByteArray {
        val out = ByteArrayOutputStream()
        val buf = ByteArray(64 * 1024)
        var total = 0
        while (true) {
            val n = input.read(buf)
            if (n == -1) break
            total += n
            if (total > MAX_BYTES) throw TooLargeException()
            out.write(buf, 0, n)
        }
        return out.toByteArray()
    }

    private fun defaultName(type: String): String {
        val ext = MimeTypeMap.getSingleton().getExtensionFromMimeType(type)
        val stamp = SimpleDateFormat("yyyyMMdd-HHmmss", Locale.US).format(Date())
        val base = if (type.startsWith("image/")) "image-" else "file-"
        return base + stamp + (ext?.let { ".$it" } ?: "")
    }
}
