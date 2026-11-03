package tech.abhashchakraborty.inyon.ssh

import com.trilead.ssh2.ChannelCondition
import com.trilead.ssh2.Connection
import com.trilead.ssh2.ConnectionMonitor
import com.trilead.ssh2.InteractiveCallback
import com.trilead.ssh2.ProxyData
import com.trilead.ssh2.ServerHostKeyVerifier
import com.trilead.ssh2.Session
import com.trilead.ssh2.crypto.PEMDecoder
import com.trilead.ssh2.crypto.fingerprint.KeyFingerprint
import com.trilead.ssh2.crypto.keys.Ed25519PrivateKey
import com.trilead.ssh2.crypto.keys.Ed25519PublicKey
import java.io.ByteArrayOutputStream
import java.io.IOException
import java.io.InputStream
import java.net.InetSocketAddress
import java.net.Socket
import java.security.KeyPair
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.ExecutionException
import java.util.concurrent.Executors
import java.util.concurrent.Future
import java.util.concurrent.RejectedExecutionException
import java.util.concurrent.ScheduledFuture
import java.util.concurrent.ThreadFactory
import java.util.concurrent.TimeUnit
import java.util.concurrent.TimeoutException
import java.util.concurrent.atomic.AtomicBoolean
import java.util.concurrent.atomic.AtomicInteger
import java.util.concurrent.atomic.AtomicReference

/**
 * SSH for the phone app: run a command, stream a command's stdout line by line, and run a PTY,
 * on servers identified by ids chosen by the caller. No Android imports, so it can be tested on a
 * JVM against a real sshd.
 *
 * One logical server may use several SSH connections: OpenSSH caps sessions per connection
 * (MaxSessions, 10 by default), so when no existing connection can open another session a new one
 * is made with the same credentials, up to [MAX_LINKS].
 */
class SshCore(private val events: Events) {

    interface Events {
        fun streamLines(streamId: String, lines: List<String>)

        fun streamExit(streamId: String, code: Int)

        fun ptyData(ptyId: String, data: ByteArray)

        fun ptyExit(ptyId: String, code: Int)

        fun closed(serverId: String, reason: String)
    }

    class SshException(val code: String, message: String, cause: Throwable? = null) : Exception(message, cause)

    /** What to authenticate with. Tried in this order: ed25519 seed, private key text, password. */
    class Credentials(
        var username: String? = null,
        var password: String? = null,
        var privateKey: String? = null,
        var passphrase: String? = null,
        var ed25519Seed: ByteArray? = null,
        var ed25519Public: ByteArray? = null,
    ) {
        internal fun sameAs(o: Credentials) =
            username == o.username &&
                password == o.password &&
                privateKey == o.privateKey &&
                passphrase == o.passphrase &&
                ed25519Seed.contentEquals(o.ed25519Seed) &&
                ed25519Public.contentEquals(o.ed25519Public)
    }

    class HostKey(val fingerprint: String, val keyType: String)

    class ExecResult(val code: Int, val stdout: String, val stderr: String)

    private val pool = Executors.newCachedThreadPool(daemon("inyon-ssh"))
    private val timer = Executors.newSingleThreadScheduledExecutor(daemon("inyon-ssh-timer"))
    private val servers = ConcurrentHashMap<String, Server>()
    // one lock per server id, kept for good: dropping an entry on disconnect would hand a concurrent connect a different lock
    private val locks = ConcurrentHashMap<String, Any>()
    // the socket of a login still in progress, so disconnect can cut it short instead of waiting for it
    private val pending = ConcurrentHashMap<String, SocketOpener>()
    private val streams = ConcurrentHashMap<String, StreamJob>()
    private val ptys = ConcurrentHashMap<String, PtyJob>()

    init {
        timer.scheduleWithFixedDelay(::tick, KEEPALIVE_MS, KEEPALIVE_MS, TimeUnit.MILLISECONDS)
    }

    // ---- public API ------------------------------------------------------------------------

    fun probe(host: String, port: Int): HostKey {
        val seen = AtomicReference<HostKey>()
        val conn = Connection(host, port)
        conn.setProxyData(SocketOpener())
        try {
            // refusing the key ends the handshake right after the server has shown it
            conn.connect(
                ServerHostKeyVerifier { _, _, algorithm, key ->
                    seen.set(HostKey(fingerprint(key), keyType(key, algorithm)))
                    false
                },
                CONNECT_TIMEOUT_MS,
                CONNECT_TIMEOUT_MS,
            )
        } catch (e: IOException) {
            if (seen.get() == null) throw unreachable(host, port, e)
        } finally {
            conn.close()
        }
        return seen.get() ?: throw SshException(UNREACHABLE, "no host key received from $host:$port")
    }

    fun connect(id: String, host: String, port: Int, hostKey: String?, creds: Credentials?) {
        if (hostKey.isNullOrBlank()) throw SshException(BAD_REQUEST, "hostKey is required")
        if (creds == null || creds.username.isNullOrEmpty()) throw SshException(BAD_REQUEST, "username is required")
        val expected = normalizeFingerprint(hostKey)
        // registered before taking the lock, so a disconnect that arrives while this waits for it finds it too
        val opener = SocketOpener()
        pending[id] = opener
        try {
            synchronized(lockFor(id)) {
                val existing = servers[id]
                if (existing != null) {
                    if (existing.same(host, port, expected, creds) && existing.hasLiveLink()) return
                    closeServer(id)
                }
                val server = Server(id, host, port, expected, creds)
                val link = openLink(server, opener)
                synchronized(server) { server.links.add(link) }
                if (link.lost) {
                    closeLink(link)
                    throw SshException(UNREACHABLE, "connection to $host:$port dropped right after login")
                }
                servers[id] = server
            }
        } finally {
            pending.remove(id, opener)
        }
    }

    fun disconnect(id: String) {
        // a login in progress holds the lock for up to the connect and auth timeouts: cut it so it fails now and lets go
        pending[id]?.kill()
        closeServer(id)
    }

    private fun closeServer(id: String) {
        synchronized(lockFor(id)) {
            val server = servers.remove(id) ?: return
            server.closing = true
            val links = synchronized(server) { server.links.toList().also { server.links.clear() } }
            links.forEach(::closeLink)
        }
    }

    fun exec(id: String, command: String?, stdin: ByteArray?, timeoutMs: Long): ExecResult {
        if (command == null) throw SshException(BAD_REQUEST, "command is required")
        val timeout = if (timeoutMs > 0) timeoutMs else DEFAULT_EXEC_TIMEOUT_MS
        val deadline = System.currentTimeMillis() + timeout
        val opened = openSessionBefore(id, deadline) ?: return ExecResult(124, "", "timed out")
        val session = opened.session
        val out = ByteArrayOutputStream()
        val err = ByteArrayOutputStream()
        val outDone: Future<*>
        val errDone: Future<*>
        try {
            session.execCommand(command)
            outDone = pool.submit(Runnable { drain(session.stdout, out) })
            errDone = pool.submit(Runnable { drain(session.stderr, err) })
            pool.execute {
                // closing stdin sends EOF, so commands that read it don't wait forever
                try {
                    session.stdin.use { if (stdin != null && stdin.isNotEmpty()) it.write(stdin) }
                } catch (_: IOException) {
                }
            }
        } catch (e: Exception) {
            opened.release()
            throw SshException(ERROR, "exec failed: ${describe(e)}", e)
        }

        var timedOut = !await(outDone, deadline) || !await(errDone, deadline)
        var status: Int? = null
        if (!timedOut) {
            val left = maxOf(1, deadline - System.currentTimeMillis())
            val cond = session.waitForCondition(ChannelCondition.EXIT_STATUS or ChannelCondition.EXIT_SIGNAL or ChannelCondition.CLOSED, left)
            status = session.exitStatus
            if (status == null && cond and (ChannelCondition.EXIT_SIGNAL or ChannelCondition.CLOSED) == 0) timedOut = true
        }
        opened.release()
        if (timedOut) {
            // closing the channel ends both readers; give them a moment to hand over what arrived
            awaitQuietly(outDone, 2000)
            awaitQuietly(errDone, 2000)
            return ExecResult(124, utf8(out), utf8(err) + "timed out")
        }
        return ExecResult(status ?: 255, utf8(out), utf8(err))
    }

    fun stream(id: String, streamId: String?, command: String?, keepStdin: Boolean) {
        if (streamId == null || command == null) throw SshException(BAD_REQUEST, "streamId and command are required")
        if (streams.containsKey(streamId)) throw SshException(BAD_REQUEST, "stream $streamId already exists")
        val opened = openSession(id)
        val job = StreamJob(streamId, opened)
        if (streams.putIfAbsent(streamId, job) != null) {
            opened.release()
            throw SshException(BAD_REQUEST, "stream $streamId already exists")
        }
        try {
            opened.session.execCommand(command)
            // tmux control mode exits when stdin closes, so keepStdin leaves it open and unused
            if (!keepStdin) opened.session.stdin.close()
            pool.execute { drain(opened.session.stderr, null) }
            pool.execute(job::run)
        } catch (e: Exception) {
            streams.remove(streamId, job)
            opened.release()
            throw SshException(ERROR, "stream failed: ${describe(e)}", e)
        }
    }

    fun streamKill(streamId: String) {
        val job = streams[streamId] ?: return
        job.killed = true
        job.opened.session.close()
    }

    fun pty(id: String, ptyId: String?, command: String?, cols: Int, rows: Int) {
        if (ptyId == null) throw SshException(BAD_REQUEST, "ptyId is required")
        if (ptys.containsKey(ptyId)) throw SshException(BAD_REQUEST, "pty $ptyId already exists")
        val opened = openSession(id)
        val job = PtyJob(ptyId, opened)
        if (ptys.putIfAbsent(ptyId, job) != null) {
            opened.release()
            throw SshException(BAD_REQUEST, "pty $ptyId already exists")
        }
        try {
            opened.session.requestPTY("xterm-256color", maxOf(1, cols), maxOf(1, rows), 0, 0, null)
            if (command.isNullOrEmpty()) opened.session.startShell() else opened.session.execCommand(command)
            pool.execute { job.pump(opened.session.stdout) }
            pool.execute { job.pump(opened.session.stderr) }
        } catch (e: Exception) {
            ptys.remove(ptyId, job)
            opened.release()
            throw SshException(ERROR, "pty failed: ${describe(e)}", e)
        }
    }

    fun ptyWrite(ptyId: String, data: ByteArray) {
        val job = pty(ptyId)
        try {
            synchronized(job) {
                val stdin = job.opened.session.stdin
                stdin.write(data)
                stdin.flush()
            }
        } catch (e: IOException) {
            throw SshException(ERROR, "pty write failed: ${describe(e)}", e)
        }
    }

    fun ptyResize(ptyId: String, cols: Int, rows: Int) {
        val job = pty(ptyId)
        try {
            job.opened.session.resizePTY(maxOf(1, cols), maxOf(1, rows), 0, 0)
        } catch (e: IOException) {
            throw SshException(ERROR, "pty resize failed: ${describe(e)}", e)
        }
    }

    fun ptyKill(ptyId: String) {
        val job = ptys[ptyId] ?: return
        job.killed = true
        job.opened.session.close()
    }

    /** Closes every server. The instance can't be used afterwards. */
    fun shutdown() {
        pending.values.forEach(SocketOpener::kill)
        servers.keys.toList().forEach(::disconnect)
        timer.shutdown()
        pool.shutdown()
    }

    // ---- connections -----------------------------------------------------------------------

    private class Server(val id: String, val host: String, val port: Int, val hostKey: String, val creds: Credentials) {
        val links = ArrayList<Link>() // guarded by this
        val growLock = Any()

        @Volatile var closing = false

        fun same(host: String, port: Int, hostKey: String, creds: Credentials) =
            this.host == host && this.port == port && this.hostKey == hostKey && this.creds.sameAs(creds)

        @Synchronized
        fun hasLiveLink() = links.any { !it.lost && !it.closing }
    }

    /** One SSH connection belonging to a server. */
    private inner class Link(val server: Server, val conn: Connection, val opener: SocketOpener) : ConnectionMonitor {
        val sessions = AtomicInteger()

        @Volatile var ready = false

        @Volatile var lost = false

        @Volatile var closing = false

        @Volatile var lastUsed = System.currentTimeMillis()

        @Volatile var pingStarted = 0L

        @Volatile var dropReason: String? = null

        override fun connectionLost(reason: Throwable?) {
            lost = true
            if (ready && !closing) onLinkLost(this, reason)
        }
    }

    /** Opens the TCP socket itself so a hung connection can be torn down without the library's locks. */
    private class SocketOpener : ProxyData {
        @Volatile private var socket: Socket? = null

        @Volatile var killed = false
            private set

        override fun openConnection(hostname: String, port: Int, connectTimeout: Int): Socket {
            val s = Socket()
            socket = s
            if (killed) s.close()
            s.tcpNoDelay = true
            s.keepAlive = true
            s.connect(InetSocketAddress(hostname, port), connectTimeout)
            return s
        }

        fun kill() {
            killed = true
            try {
                socket?.close()
            } catch (_: IOException) {
            }
        }
    }

    private class HostKeyCheck(val expected: String) : ServerHostKeyVerifier {
        @Volatile var presented: String? = null

        override fun verifyServerHostKey(hostname: String?, port: Int, algorithm: String?, key: ByteArray): Boolean {
            val fp = fingerprint(key)
            if (fp == expected) return true
            presented = fp
            return false
        }
    }

    private fun openLink(server: Server, opener: SocketOpener = SocketOpener()): Link {
        val conn = Connection(server.host, server.port)
        conn.setProxyData(opener)
        val link = Link(server, conn, opener)
        conn.addConnectionMonitor(link)
        val check = HostKeyCheck(server.hostKey)
        // the library has no timeout for authentication, so bound the whole login
        val guard = timer.schedule(Runnable(opener::kill), AUTH_TIMEOUT_MS, TimeUnit.MILLISECONDS)
        var ok = false
        try {
            try {
                conn.connect(check, CONNECT_TIMEOUT_MS, CONNECT_TIMEOUT_MS)
            } catch (e: IOException) {
                check.presented?.let {
                    throw SshException(HOST_KEY_MISMATCH, "host key mismatch for ${server.host}: expected ${server.hostKey}, server presented $it")
                }
                throw unreachable(server.host, server.port, e)
            }
            authenticate(link, server.creds)
            link.ready = true
            ok = true
            return link
        } catch (e: SshException) {
            // the socket was cut from outside (a disconnect, or the login guard), so the library's own complaint is beside the point
            if (opener.killed && e.code != HOST_KEY_MISMATCH) {
                throw SshException(UNREACHABLE, "login to ${server.host}:${server.port} was cut off (cancelled or timed out)", e)
            }
            throw e
        } finally {
            guard.cancel(false)
            if (!ok) {
                link.closing = true
                conn.close()
            }
        }
    }

    private fun authenticate(link: Link, c: Credentials) {
        val conn = link.conn
        val user = c.username
        val lastError = arrayOfNulls<String>(1)

        // Tailscale SSH (and any server that trusts the network itself) lets you in with no key or password
        try {
            if (conn.authenticateWithNone(user)) return
        } catch (e: Exception) {
            if (link.lost) throw SshException(UNREACHABLE, "connection lost during authentication: ${describe(e)}", e)
        }

        val seed = c.ed25519Seed
        val public = c.ed25519Public
        if (seed != null && public != null) {
            if (seed.size != 32 || public.size != 32) throw SshException(BAD_REQUEST, "ed25519Seed and ed25519Public must be 32 bytes each")
            val pair = KeyPair(Ed25519PublicKey(public.clone()), Ed25519PrivateKey(seed.clone()))
            if (attempt(link, user, "publickey", lastError) { conn.authenticateWithPublicKey(user, pair) }) return
        }

        val privateKey = c.privateKey
        if (!privateKey.isNullOrEmpty()) {
            val pair = try {
                PEMDecoder.decode(privateKey.toCharArray(), c.passphrase?.ifEmpty { null })
            } catch (e: Exception) {
                lastError[0] = "private key could not be read (wrong passphrase or unsupported format): ${describe(e)}"
                null
            }
            if (pair != null && attempt(link, user, "publickey", lastError) { conn.authenticateWithPublicKey(user, pair) }) return
        }

        val password = c.password
        if (password != null) {
            if (attempt(link, user, "password", lastError) { conn.authenticateWithPassword(user, password) }) return
            val answer = object : InteractiveCallback {
                private var rounds = 0

                override fun replyToChallenge(name: String?, instruction: String?, numPrompts: Int, prompt: Array<String>?, echo: BooleanArray?): Array<String> {
                    // only plain password prompts can be answered; give up on anything that keeps asking
                    if (numPrompts > 0 && ++rounds > 2) throw IOException("unexpected keyboard-interactive prompts")
                    return Array(numPrompts) { password }
                }
            }
            if (attempt(link, user, "keyboard-interactive", lastError) { conn.authenticateWithKeyboardInteractive(user, answer) }) return
        }

        lastError[0]?.let { throw SshException(AUTH_FAILED, "authentication failed: $it") }
        val methods = try {
            conn.getRemainingAuthMethods(user).contentToString()
        } catch (_: Exception) {
            "unknown"
        }
        throw SshException(AUTH_FAILED, "authentication failed for $user (server accepts $methods)")
    }

    private inline fun attempt(link: Link, user: String?, method: String, lastError: Array<String?>, run: () -> Boolean): Boolean {
        return try {
            link.conn.isAuthMethodAvailable(user, method) && run()
        } catch (e: Exception) {
            if (link.lost) throw SshException(UNREACHABLE, "connection lost during authentication: ${describe(e)}", e)
            lastError[0] = "$method: ${describe(e)}"
            false
        }
    }

    private fun onLinkLost(link: Link, reason: Throwable?) {
        val server = link.server
        val empty = synchronized(server) {
            server.links.remove(link)
            server.links.isEmpty()
        }
        // streams and PTYs on this link see EOF without an exit status and report 255 themselves
        if (empty && !server.closing && servers.remove(server.id, server)) {
            val why = link.dropReason ?: if (reason == null) "connection lost" else describe(reason)
            events.closed(server.id, why)
        }
    }

    private fun closeLink(link: Link) {
        link.closing = true
        val guard: ScheduledFuture<*>? = try {
            timer.schedule(Runnable(link.opener::kill), CLOSE_GRACE_MS, TimeUnit.MILLISECONDS)
        } catch (_: RejectedExecutionException) {
            link.opener.kill()
            null
        }
        try {
            link.conn.close()
        } catch (_: RuntimeException) {
        } finally {
            guard?.cancel(false)
        }
    }

    /** Keepalive, dead connection detection and closing extra connections nobody uses. */
    private fun tick() {
        try {
            val now = System.currentTimeMillis()
            for (server in servers.values) {
                val links = synchronized(server) { server.links.toList() }
                for ((i, link) in links.withIndex()) {
                    if (link.lost || link.closing) continue
                    if (i > 0 && link.sessions.get() == 0 && now - link.lastUsed > IDLE_LINK_MS) {
                        val reap = synchronized(server) {
                            (link.sessions.get() == 0 && server.links.remove(link)).also { if (it) link.closing = true }
                        }
                        if (reap) pool.execute { closeLink(link) }
                        continue
                    }
                    val started = link.pingStarted
                    if (started != 0L) {
                        // the previous ping never came back: drop the socket so everything waiting wakes up
                        if (now - started >= KEEPALIVE_MS) {
                            link.dropReason = "no reply to keepalive"
                            link.opener.kill()
                        }
                        continue
                    }
                    link.pingStarted = now
                    pool.execute {
                        try {
                            link.conn.ping()
                            link.pingStarted = 0
                        } catch (e: Exception) {
                            if (link.dropReason == null && !link.lost) link.dropReason = "keepalive failed: ${describe(e)}"
                            link.opener.kill()
                        }
                    }
                }
            }
        } catch (_: RuntimeException) {
            // a failing tick must not cancel the schedule
        }
    }

    // ---- sessions --------------------------------------------------------------------------

    private class Opened(val link: Link, val session: Session) {
        private val released = AtomicBoolean()

        fun release() {
            if (!released.compareAndSet(false, true)) return
            session.close()
            link.sessions.decrementAndGet()
            link.lastUsed = System.currentTimeMillis()
        }
    }

    private fun openSession(id: String): Opened {
        val server = servers[id] ?: throw SshException(NOT_CONNECTED, "not connected: $id")
        tryLinks(server)?.let { return it }
        synchronized(server.growLock) {
            tryLinks(server)?.let { return it }
            if (server.closing || servers[id] !== server) throw SshException(NOT_CONNECTED, "not connected: $id")
            val count = synchronized(server) { server.links.size }
            if (count >= MAX_LINKS) throw SshException(ERROR, "no free SSH session on $id ($count connections, all full)")
            val link = openLink(server)
            synchronized(server) { server.links.add(link) }
            if (link.lost || server.closing || servers[id] !== server) {
                synchronized(server) { server.links.remove(link) }
                closeLink(link)
                throw SshException(NOT_CONNECTED, "not connected: $id")
            }
            try {
                return open(link)
            } catch (e: Exception) {
                throw SshException(ERROR, "could not open an SSH session on $id: ${describe(e)}", e)
            }
        }
    }

    /**
     * Opening a session blocks without a timeout on a connection that died silently (until the
     * keepalive notices), so exec waits for it only until its own deadline. Null when that passed.
     */
    private fun openSessionBefore(id: String, deadline: Long): Opened? {
        if (!servers.containsKey(id)) throw SshException(NOT_CONNECTED, "not connected: $id")
        val pending = pool.submit<Opened> { openSession(id) }
        try {
            return pending.get(maxOf(1, deadline - System.currentTimeMillis()), TimeUnit.MILLISECONDS)
        } catch (e: Exception) {
            when (e) {
                is TimeoutException, is InterruptedException -> {
                    if (e is InterruptedException) Thread.currentThread().interrupt()
                    pool.execute {
                        try {
                            pending.get().release()
                        } catch (_: Exception) {
                        }
                    }
                    return null
                }
                is ExecutionException -> {
                    val cause = e.cause
                    if (cause is SshException) throw cause
                    throw SshException(ERROR, "could not open an SSH session on $id: ${describe(cause ?: e)}", cause)
                }
                else -> throw e
            }
        }
    }

    /** Tries the least busy connections first; a refusal usually means that one is at MaxSessions. */
    private fun tryLinks(server: Server): Opened? {
        val links = synchronized(server) { server.links.toList() }.sortedBy { it.sessions.get() }
        for (link in links) {
            if (link.lost || link.closing) continue
            try {
                return open(link)
            } catch (_: Exception) {
            }
        }
        return null
    }

    private fun open(link: Link): Opened {
        link.sessions.incrementAndGet()
        try {
            val session = link.conn.openSession()
            link.lastUsed = System.currentTimeMillis()
            return Opened(link, session)
        } catch (e: Exception) {
            link.sessions.decrementAndGet()
            throw e
        }
    }

    private abstract class Job(val id: String, val opened: Opened) {
        @Volatile var killed = false

        fun exitCode(): Int {
            if (!killed) {
                opened.session.waitForCondition(ChannelCondition.EXIT_STATUS or ChannelCondition.EXIT_SIGNAL or ChannelCondition.CLOSED, EXIT_STATUS_WAIT_MS)
            }
            if (killed) return 143
            return opened.session.exitStatus ?: 255
        }
    }

    private inner class StreamJob(id: String, opened: Opened) : Job(id, opened) {
        private val splitter = LineSplitter()
        private val pending = ArrayList<String>() // guarded by this
        private var flushScheduled = false
        private var finished = false

        fun run() {
            val buf = ByteArray(16384)
            val input = opened.session.stdout
            try {
                while (true) {
                    val n = input.read(buf)
                    if (n <= 0) break
                    val lines = splitter.feed(buf, 0, n)
                    if (lines.isNotEmpty()) add(lines)
                }
            } catch (_: IOException) {
            }
            val tail = splitter.finish()
            synchronized(this) {
                if (tail != null) pending.add(tail)
                flushLocked()
                finished = true
            }
            val code = exitCode()
            opened.release()
            streams.remove(id, this)
            events.streamExit(id, code)
        }

        @Synchronized
        private fun add(lines: List<String>) {
            pending.addAll(lines)
            if (pending.size >= FLUSH_LINES) {
                flushLocked()
                return
            }
            if (flushScheduled) return
            flushScheduled = true
            try {
                timer.schedule(Runnable(::flush), FLUSH_MS, TimeUnit.MILLISECONDS)
            } catch (_: RejectedExecutionException) {
                flushScheduled = false
                flushLocked()
            }
        }

        @Synchronized
        fun flush() {
            flushScheduled = false
            if (!finished) flushLocked()
        }

        private fun flushLocked() {
            if (pending.isEmpty()) return
            val batch = ArrayList(pending)
            pending.clear()
            events.streamLines(id, batch)
        }
    }

    private inner class PtyJob(id: String, opened: Opened) : Job(id, opened) {
        private val open = AtomicInteger(2)

        fun pump(input: InputStream) {
            val buf = ByteArray(32768)
            try {
                while (true) {
                    val n = input.read(buf)
                    if (n <= 0) break
                    events.ptyData(id, buf.copyOf(n))
                }
            } catch (_: IOException) {
            }
            if (open.decrementAndGet() != 0) return
            val code = exitCode()
            opened.release()
            ptys.remove(id, this)
            events.ptyExit(id, code)
        }
    }

    private fun pty(ptyId: String?): PtyJob = ptyId?.let { ptys[it] } ?: throw SshException(NOT_FOUND, "no such pty: $ptyId")

    // ---- helpers ---------------------------------------------------------------------------

    private fun lockFor(id: String): Any = locks.getOrPut(id) { Any() }

    companion object {
        const val HOST_KEY_MISMATCH = "HOST_KEY_MISMATCH"
        const val AUTH_FAILED = "AUTH_FAILED"
        const val UNREACHABLE = "UNREACHABLE"
        const val NOT_CONNECTED = "NOT_CONNECTED"
        const val NOT_FOUND = "NOT_FOUND"
        const val BAD_REQUEST = "BAD_REQUEST"
        const val ERROR = "ERROR"

        const val DEFAULT_EXEC_TIMEOUT_MS = 20000L
        private const val CONNECT_TIMEOUT_MS = 12000
        private const val AUTH_TIMEOUT_MS = 30000L
        private const val KEEPALIVE_MS = 15000L
        private const val MAX_LINKS = 6
        private const val FLUSH_MS = 30L
        private const val FLUSH_LINES = 1000
        private const val IDLE_LINK_MS = 60000L
        private const val EXIT_STATUS_WAIT_MS = 5000L
        private const val CLOSE_GRACE_MS = 3000L

        private fun drain(input: InputStream, sink: ByteArrayOutputStream?) {
            val buf = ByteArray(16384)
            try {
                while (true) {
                    val n = input.read(buf)
                    if (n <= 0) break
                    sink?.write(buf, 0, n)
                }
            } catch (_: IOException) {
            }
        }

        /** False when the deadline passed first. */
        private fun await(f: Future<*>, deadline: Long): Boolean {
            val left = deadline - System.currentTimeMillis()
            if (left <= 0) return f.isDone
            return try {
                f.get(left, TimeUnit.MILLISECONDS)
                true
            } catch (_: TimeoutException) {
                false
            } catch (_: ExecutionException) {
                true
            } catch (_: InterruptedException) {
                Thread.currentThread().interrupt()
                false
            }
        }

        private fun awaitQuietly(f: Future<*>, ms: Long) {
            try {
                f.get(ms, TimeUnit.MILLISECONDS)
            } catch (_: InterruptedException) {
                Thread.currentThread().interrupt()
            } catch (_: ExecutionException) {
            } catch (_: TimeoutException) {
            }
        }

        private fun utf8(b: ByteArrayOutputStream) = String(b.toByteArray(), Charsets.UTF_8)

        fun fingerprint(hostKeyBlob: ByteArray): String = KeyFingerprint.createSHA256Fingerprint(hostKeyBlob)

        /** The key type named inside the SSH key blob ("ssh-rsa" rather than the "rsa-sha2-512" signature name). */
        fun keyType(blob: ByteArray?, fallback: String): String {
            if (blob != null && blob.size >= 4) {
                val n = (blob[0].toInt() and 0xff shl 24) or (blob[1].toInt() and 0xff shl 16) or (blob[2].toInt() and 0xff shl 8) or (blob[3].toInt() and 0xff)
                if (n in 1..64 && n <= blob.size - 4) return String(blob, 4, n, Charsets.US_ASCII)
            }
            return fallback
        }

        fun normalizeFingerprint(fp: String) = fp.trim().trimEnd('=')

        private fun unreachable(host: String, port: Int, e: Throwable) =
            SshException(UNREACHABLE, "could not reach $host:$port: ${describe(e)}", e)

        fun describe(e: Throwable): String {
            val sb = StringBuilder()
            var t: Throwable? = e
            var depth = 0
            while (t != null && depth < 5) {
                val m = t.message
                if (!m.isNullOrEmpty() && sb.indexOf(m) < 0) {
                    if (sb.isNotEmpty()) sb.append(": ")
                    sb.append(m)
                }
                t = t.cause
                depth++
            }
            return if (sb.isNotEmpty()) sb.toString() else e.javaClass.simpleName
        }

        private fun daemon(name: String): ThreadFactory {
            val n = AtomicInteger()
            return ThreadFactory { r -> Thread(r, "$name-${n.incrementAndGet()}").apply { isDaemon = true } }
        }
    }
}
