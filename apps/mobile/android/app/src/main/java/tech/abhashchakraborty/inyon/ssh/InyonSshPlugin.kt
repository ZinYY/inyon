package tech.abhashchakraborty.inyon.ssh

import android.util.Log
import com.getcapacitor.JSObject
import com.getcapacitor.Plugin
import com.getcapacitor.PluginCall
import com.getcapacitor.PluginMethod
import com.getcapacitor.annotation.CapacitorPlugin
import org.json.JSONObject

/** Direct SSH to the user's servers, for the web app. The methods themselves live in [SshMethods]. */
@CapacitorPlugin(name = "InyonSsh")
class InyonSshPlugin : Plugin() {

    private lateinit var methods: SshMethods

    override fun load() {
        methods = SshMethods(context) { event, data -> notifyListeners(event, JSObject.fromJSONObject(data)) }
        // the release smoke test looks for this line
        Log.i(TAG, "ready")
    }

    override fun handleOnDestroy() {
        if (::methods.isInitialized) methods.shutdown()
    }

    private fun forward(method: String, call: PluginCall) {
        methods.call(
            method,
            call.data,
            object : SshMethods.Reply {
                override fun resolve(result: JSONObject) = call.resolve(JSObject.fromJSONObject(result))

                override fun reject(message: String, code: String) = call.reject(message, code)
            },
        )
    }

    @PluginMethod
    fun probe(call: PluginCall) = forward("probe", call)

    @PluginMethod
    fun connect(call: PluginCall) = forward("connect", call)

    @PluginMethod
    fun disconnect(call: PluginCall) = forward("disconnect", call)

    @PluginMethod
    fun exec(call: PluginCall) = forward("exec", call)

    @PluginMethod
    fun stream(call: PluginCall) = forward("stream", call)

    @PluginMethod
    fun streamKill(call: PluginCall) = forward("streamKill", call)

    @PluginMethod
    fun pty(call: PluginCall) = forward("pty", call)

    @PluginMethod
    fun ptyWrite(call: PluginCall) = forward("ptyWrite", call)

    @PluginMethod
    fun ptyResize(call: PluginCall) = forward("ptyResize", call)

    @PluginMethod
    fun ptyKill(call: PluginCall) = forward("ptyKill", call)

    @PluginMethod
    fun setSecret(call: PluginCall) = forward("setSecret", call)

    @PluginMethod
    fun getSecret(call: PluginCall) = forward("getSecret", call)

    @PluginMethod
    fun deleteSecret(call: PluginCall) = forward("deleteSecret", call)

    private companion object {
        const val TAG = "InyonSsh"
    }
}
