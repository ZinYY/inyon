package tech.abhashchakraborty.inyon.device

import android.content.Context
import java.security.MessageDigest
import java.security.SecureRandom

/**
 * Which in-app path an Intent may open. MainActivity is exported (the launcher and inyon:// links
 * need it), so any app on the phone can start it with an extra. Only Inyon's own notifications
 * carry the random nonce this install made (kept in the app's private storage, which backups and
 * device transfers leave out), and only with their own action does a path get through.
 */
object HrefGate {
    const val EXTRA_NONCE = "tech.abhashchakraborty.inyon.extra.NONCE"
    private const val PREFS = "inyon.href"
    private const val KEY = "nonce"

    @Volatile private var cached: String? = null

    /** This install's nonce, made on first use. */
    fun nonce(ctx: Context): String {
        cached?.let { return it }
        synchronized(this) {
            cached?.let { return it }
            val prefs = ctx.applicationContext.getSharedPreferences(PREFS, Context.MODE_PRIVATE)
            val made = prefs.getString(KEY, null) ?: ByteArray(32).also { SecureRandom().nextBytes(it) }.joinToString("") { "%02x".format(it) }.also {
                prefs.edit().putString(KEY, it).apply()
            }
            cached = made
            return made
        }
    }

    /**
     * The path to open, or null: the Intent must have a notification's action, this install's
     * nonce, and a path inside the app (not `//host` or `/\host`, which a browser reads as another site).
     */
    fun accept(action: String?, href: String?, nonce: String?, expected: String): String? {
        if (action != Notifications.ACTION_OPEN_HREF || href == null || nonce == null || expected.isEmpty()) return null
        if (!MessageDigest.isEqual(nonce.toByteArray(), expected.toByteArray())) return null
        if (!href.startsWith("/") || href.startsWith("//") || href.startsWith("/\\")) return null
        return href
    }
}
