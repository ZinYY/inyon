package tech.abhashchakraborty.inyon.ssh

import android.content.Context
import android.content.SharedPreferences
import android.util.Base64
import androidx.security.crypto.EncryptedSharedPreferences
import androidx.security.crypto.MasterKey
import org.json.JSONArray
import org.json.JSONObject
import tech.abhashchakraborty.inyon.device.KeepAliveService
import java.util.ArrayDeque
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.Executor
import java.util.concurrent.Executors

/**
 * Direct SSH to the user's servers and the phone's secrets, without Capacitor: a method's name and
 * its JSON arguments in, a JSON answer or an error out, events as a name and JSON. The Capacitor
 * plugin ([InyonSshPlugin]) and the Dioxus build's bridge to Rust both call these. All the SSH
 * work lives in [SshCore].
 */
class SshMethods(private val context: Context, private val emit: (event: String, data: JSONObject) -> Unit) {

    /** How a call ends: once, from whichever thread did the work. */
    interface Reply {
        fun resolve(result: JSONObject)

        fun reject(message: String, code: String)
    }

    private val pool = Executors.newCachedThreadPool()

    // keystrokes for one PTY must reach the server in the order they were typed
    private val ptyQueues = ConcurrentHashMap<String, SerialExecutor>()
    private var secrets: SharedPreferences? = null

    private val core = SshCore(
        object : SshCore.Events {
            override fun streamLines(streamId: String, lines: List<String>) {
                // in the background this is how alerts start: stay awake long enough to post one
                KeepAliveService.nudge()
                emit("streamLines", JSONObject().put("streamId", streamId).put("lines", JSONArray(lines)))
            }

            override fun streamExit(streamId: String, code: Int) {
                KeepAliveService.nudge()
                emit("streamExit", JSONObject().put("streamId", streamId).put("code", code))
            }

            override fun ptyData(ptyId: String, data: ByteArray) {
                emit("ptyData", JSONObject().put("ptyId", ptyId).put("data", Base64.encodeToString(data, Base64.NO_WRAP)))
            }

            override fun ptyExit(ptyId: String, code: Int) {
                ptyQueues.remove(ptyId)
                emit("ptyExit", JSONObject().put("ptyId", ptyId).put("code", code))
            }

            override fun closed(serverId: String, reason: String) {
                KeepAliveService.nudge()
                emit("closed", JSONObject().put("id", serverId).put("reason", reason))
            }
        },
    )

    fun shutdown() {
        core.shutdown()
        pool.shutdown()
    }

    /** Runs `method` with `args`; unknown methods and missing arguments are refused at once. */
    fun call(method: String, args: JSONObject, reply: Reply) {
        when (method) {
            "probe" -> {
                val host = str(args, "host") ?: return reply.reject("host is required", SshCore.BAD_REQUEST)
                val port = int(args, "port", 22)
                run(reply) {
                    val key = core.probe(host, port)
                    reply.resolve(JSONObject().put("fingerprint", key.fingerprint).put("keyType", key.keyType))
                }
            }
            "connect" -> {
                val id = str(args, "id")
                val host = str(args, "host")
                if (id == null || host == null) return reply.reject("id and host are required", SshCore.BAD_REQUEST)
                val creds = SshCore.Credentials(
                    username = str(args, "username"),
                    password = str(args, "password"),
                    privateKey = str(args, "privateKey"),
                    passphrase = str(args, "passphrase"),
                )
                try {
                    creds.ed25519Seed = base64(args, "ed25519Seed")
                    creds.ed25519Public = base64(args, "ed25519Public")
                } catch (_: IllegalArgumentException) {
                    return reply.reject("ed25519Seed/ed25519Public must be base64", SshCore.BAD_REQUEST)
                }
                val port = int(args, "port", 22)
                val hostKey = str(args, "hostKey")
                run(reply) {
                    core.connect(id, host, port, hostKey, creds)
                    reply.resolve(ok())
                }
            }
            "disconnect" -> {
                val id = str(args, "id") ?: return reply.reject("id is required", SshCore.BAD_REQUEST)
                run(reply) {
                    core.disconnect(id)
                    reply.resolve(ok())
                }
            }
            "exec" -> {
                val id = str(args, "id")
                val command = str(args, "command")
                if (id == null || command == null) return reply.reject("id and command are required", SshCore.BAD_REQUEST)
                val stdin = try {
                    base64(args, "stdin")
                } catch (_: IllegalArgumentException) {
                    return reply.reject("stdin must be base64", SshCore.BAD_REQUEST)
                }
                val timeoutMs = long(args, "timeoutMs", SshCore.DEFAULT_EXEC_TIMEOUT_MS)
                run(reply) {
                    val r = core.exec(id, command, stdin, timeoutMs)
                    reply.resolve(JSONObject().put("code", r.code).put("stdout", r.stdout).put("stderr", r.stderr))
                }
            }
            "stream" -> {
                val id = str(args, "id")
                val streamId = str(args, "streamId")
                val command = str(args, "command")
                if (id == null || streamId == null || command == null) return reply.reject("id, streamId and command are required", SshCore.BAD_REQUEST)
                val keepStdin = args.opt("keepStdin") as? Boolean ?: false
                run(reply) {
                    core.stream(id, streamId, command, keepStdin)
                    reply.resolve(ok())
                }
            }
            "streamKill" -> {
                val streamId = str(args, "streamId") ?: return reply.reject("streamId is required", SshCore.BAD_REQUEST)
                run(reply) {
                    core.streamKill(streamId)
                    reply.resolve(ok())
                }
            }
            "pty" -> {
                val id = str(args, "id")
                val ptyId = str(args, "ptyId")
                if (id == null || ptyId == null) return reply.reject("id and ptyId are required", SshCore.BAD_REQUEST)
                val command = str(args, "command")
                val cols = int(args, "cols", 80)
                val rows = int(args, "rows", 24)
                run(reply) {
                    core.pty(id, ptyId, command, cols, rows)
                    reply.resolve(ok())
                }
            }
            "ptyWrite" -> {
                val ptyId = str(args, "ptyId")
                val data = str(args, "data")
                if (ptyId == null || data == null) return reply.reject("ptyId and data are required", SshCore.BAD_REQUEST)
                val bytes = data.toByteArray(Charsets.UTF_8)
                runInOrder(ptyId, reply) {
                    core.ptyWrite(ptyId, bytes)
                    reply.resolve(ok())
                }
            }
            "ptyResize" -> {
                val ptyId = str(args, "ptyId") ?: return reply.reject("ptyId is required", SshCore.BAD_REQUEST)
                val cols = int(args, "cols", 80)
                val rows = int(args, "rows", 24)
                runInOrder(ptyId, reply) {
                    core.ptyResize(ptyId, cols, rows)
                    reply.resolve(ok())
                }
            }
            "ptyKill" -> {
                val ptyId = str(args, "ptyId") ?: return reply.reject("ptyId is required", SshCore.BAD_REQUEST)
                run(reply) {
                    core.ptyKill(ptyId)
                    reply.resolve(ok())
                }
            }
            "setSecret" -> {
                val key = str(args, "key")
                val value = str(args, "value")
                if (key == null || value == null) return reply.reject("key and value are required", SshCore.BAD_REQUEST)
                run(reply) {
                    check(secrets().edit().putString(key, value).commit()) { "could not save the secret" }
                    reply.resolve(ok())
                }
            }
            "getSecret" -> {
                val key = str(args, "key") ?: return reply.reject("key is required", SshCore.BAD_REQUEST)
                run(reply) {
                    reply.resolve(JSONObject().put("value", secrets().getString(key, null) ?: JSONObject.NULL))
                }
            }
            "deleteSecret" -> {
                val key = str(args, "key") ?: return reply.reject("key is required", SshCore.BAD_REQUEST)
                run(reply) {
                    secrets().edit().remove(key).commit()
                    reply.resolve(ok())
                }
            }
            else -> reply.reject("unknown method: $method", SshCore.BAD_REQUEST)
        }
    }

    /** The native encrypted file the phone's key, hub tokens and server sign-ins live in. */
    @Synchronized
    @Suppress("DEPRECATION")
    private fun secrets(): SharedPreferences =
        secrets ?: EncryptedSharedPreferences.create(
            context,
            SECRETS_FILE,
            MasterKey.Builder(context).setKeyScheme(MasterKey.KeyScheme.AES256_GCM).build(),
            EncryptedSharedPreferences.PrefKeyEncryptionScheme.AES256_SIV,
            EncryptedSharedPreferences.PrefValueEncryptionScheme.AES256_GCM,
        ).also { secrets = it }

    private fun run(reply: Reply, task: () -> Unit) {
        pool.execute { runNow(reply, task) }
    }

    private fun runInOrder(key: String, reply: Reply, task: () -> Unit) {
        ptyQueues.getOrPut(key) { SerialExecutor(pool) }.execute { runNow(reply, task) }
    }

    /** Runs tasks one at a time, in submission order, on a shared executor. */
    private class SerialExecutor(private val executor: Executor) : Executor {
        private val tasks = ArrayDeque<Runnable>()
        private var active: Runnable? = null

        @Synchronized
        override fun execute(r: Runnable) {
            tasks.offer(
                Runnable {
                    try {
                        r.run()
                    } finally {
                        scheduleNext()
                    }
                },
            )
            if (active == null) scheduleNext()
        }

        @Synchronized
        private fun scheduleNext() {
            active = tasks.poll()
            active?.let(executor::execute)
        }
    }

    companion object {
        const val SECRETS_FILE = "inyon_secrets"

        private fun runNow(reply: Reply, task: () -> Unit) {
            try {
                task()
            } catch (e: SshCore.SshException) {
                reply.reject(e.message ?: e.code, e.code)
            } catch (e: Exception) {
                reply.reject(e.message ?: e.toString(), SshCore.ERROR)
            }
        }

        private fun ok(): JSONObject = JSONObject().put("ok", true)

        private fun str(args: JSONObject, name: String): String? = args.opt(name) as? String

        private fun int(args: JSONObject, name: String, fallback: Int) = (args.opt(name) as? Number)?.toInt() ?: fallback

        private fun long(args: JSONObject, name: String, fallback: Long) = (args.opt(name) as? Number)?.toLong() ?: fallback

        private fun base64(args: JSONObject, name: String): ByteArray? = str(args, name)?.let { Base64.decode(it, Base64.DEFAULT) }
    }
}
