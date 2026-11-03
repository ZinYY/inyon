package tech.abhashchakraborty.inyon.migrate

import androidx.security.crypto.EncryptedSharedPreferences
import androidx.security.crypto.MasterKey
import com.getcapacitor.JSObject
import com.getcapacitor.Plugin
import com.getcapacitor.PluginCall
import com.getcapacitor.PluginMethod
import com.getcapacitor.annotation.CapacitorPlugin
import java.util.concurrent.Executors
import org.json.JSONObject

/**
 * Keeps [MigrationExport]'s file current. The web app passes what lives in WebView storage
 * (src/lib/upgradeExport.ts) on every start and every time the app goes to the background; the
 * secrets are added here from the file InyonSshPlugin keeps them in, so they never cross the bridge.
 */
@CapacitorPlugin(name = "InyonMigrate")
class InyonMigratePlugin : Plugin() {

    // one at a time, in order: a later export must not be overwritten by an earlier one
    private val queue = Executors.newSingleThreadExecutor()

    @PluginMethod
    fun exportForUpgrade(call: PluginCall) {
        val json = call.getString("json") ?: return call.reject("json is required", "BAD_REQUEST")
        queue.execute {
            try {
                val payload = JSONObject(json)
                require(payload.optInt("v") == MigrationExport.PAYLOAD_VERSION) { "unknown export version" }
                payload.put("secrets", JSONObject(secrets()))
                MigrationExport.write(context, payload.toString().toByteArray(Charsets.UTF_8))
                call.resolve(JSObject().put("ok", true))
            } catch (e: Exception) {
                call.reject(e.message ?: e.toString(), "ERROR", e)
            }
        }
    }

    override fun handleOnDestroy() {
        queue.shutdown()
    }

    /** Everything in inyon_secrets: the phone's key, hub tokens, server passwords and pasted keys. */
    @Suppress("DEPRECATION")
    private fun secrets(): Map<String, String> =
        EncryptedSharedPreferences.create(
            context,
            "inyon_secrets",
            MasterKey.Builder(context).setKeyScheme(MasterKey.KeyScheme.AES256_GCM).build(),
            EncryptedSharedPreferences.PrefKeyEncryptionScheme.AES256_SIV,
            EncryptedSharedPreferences.PrefValueEncryptionScheme.AES256_GCM,
        ).all.mapNotNull { (k, v) -> (v as? String)?.let { k to it } }.toMap()
}
