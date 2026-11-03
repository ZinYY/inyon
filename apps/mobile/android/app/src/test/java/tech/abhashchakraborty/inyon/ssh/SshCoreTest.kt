package tech.abhashchakraborty.inyon.ssh

import java.io.ByteArrayOutputStream
import java.io.File
import java.util.concurrent.Executors
import java.util.concurrent.Future
import java.util.concurrent.TimeUnit
import java.util.concurrent.locks.ReentrantLock
import kotlin.concurrent.withLock
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertThrows
import org.junit.Assert.assertTrue
import org.junit.Assume.assumeTrue
import org.junit.Before
import org.junit.Test

/**
 * Runs against a real sshd given by INYON_SSH_TEST_HOST / _PORT / _USER / _KEY (an unencrypted
 * OpenSSH ed25519 private key file) and optionally _PASSWORD. Without them the server tests skip.
 */
class SshCoreTest {

    private lateinit var events: Recorder
    private lateinit var core: SshCore

    @Before
    fun setUp() {
        events = Recorder()
        core = SshCore(events)
    }

    @After
    fun tearDown() {
        core.shutdown()
    }

    // ---- offline -------------------------------------------------------------------------------

    @Test
    fun lineSplitterHandlesPartialLinesCrlfAndSplitCharacters() {
        val s = LineSplitter()
        val all = "one\r\ntw\u00f6\n\nthree".toByteArray(Charsets.UTF_8)
        val lines = ArrayList<String>()
        // feed one byte at a time so the two-byte character is cut in half
        for (i in all.indices) lines.addAll(s.feed(all, i, 1))
        assertEquals(listOf("one", "tw\u00f6", ""), lines)
        assertEquals("three", s.finish())
        assertNull(s.finish())
    }

    @Test
    fun keyTypeComesFromTheBlob() {
        val name = "ssh-ed25519".toByteArray(Charsets.US_ASCII)
        val blob = ByteArray(4 + name.size + 4)
        blob[3] = name.size.toByte()
        name.copyInto(blob, 4)
        assertEquals("ssh-ed25519", SshCore.keyType(blob, "fallback"))
        assertEquals("fallback", SshCore.keyType(ByteArray(2), "fallback"))
        assertEquals("SHA256:abc", SshCore.normalizeFingerprint(" SHA256:abc= "))
    }

    @Test
    fun notConnectedIsReported() {
        expectCode(SshCore.NOT_CONNECTED) { core.exec("nope", "true", null, 1000) }
    }

    // ---- against a server ----------------------------------------------------------------------

    @Test
    fun probeReturnsSha256Fingerprint() {
        requireServer()
        val key = core.probe(HOST!!, port())
        assertTrue(key.fingerprint, key.fingerprint.matches(Regex("SHA256:[A-Za-z0-9+/]{43}")))
        assertNotNull(key.keyType)
        assertTrue(key.keyType, key.keyType.startsWith("ssh-") || key.keyType.startsWith("ecdsa-"))
    }

    @Test
    fun connectsWithEd25519SeedAndReconnectIsANoOp() {
        connect()
        val t = System.nanoTime()
        core.connect(ID, HOST!!, port(), hostKey(), seedCredentials())
        assertTrue("second connect should not log in again", System.nanoTime() - t < TimeUnit.MILLISECONDS.toNanos(200))
        assertEquals("ok\n", core.exec(ID, "echo ok", null, 0).stdout)
    }

    @Test
    fun connectsWithPrivateKeyText() {
        requireServer()
        val c = SshCore.Credentials(username = USER, privateKey = File(KEY!!).readText())
        core.connect(ID, HOST!!, port(), hostKey(), c)
        assertEquals(0, core.exec(ID, "true", null, 0).code)
    }

    @Test
    fun connectsWithPassword() {
        requireServer()
        assumeTrue("INYON_SSH_TEST_PASSWORD not set", !PASSWORD.isNullOrEmpty())
        val c = SshCore.Credentials(username = USER, password = PASSWORD)
        core.connect(ID, HOST!!, port(), hostKey(), c)
        assertEquals(0, core.exec(ID, "true", null, 0).code)
    }

    @Test
    fun hostKeyMismatchIsRejected() {
        requireServer()
        expectCode(SshCore.HOST_KEY_MISMATCH) {
            core.connect(ID, HOST!!, port(), "SHA256:AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA", seedCredentials())
        }
    }

    @Test
    fun wrongKeyIsAuthFailed() {
        requireServer()
        val c = seedCredentials()
        c.ed25519Seed = ByteArray(32)
        c.ed25519Public = ByteArray(32).also { it[0] = 1 }
        expectCode(SshCore.AUTH_FAILED) { core.connect(ID, HOST!!, port(), hostKey(), c) }
    }

    @Test
    fun unreachableIsReported() {
        expectCode(SshCore.UNREACHABLE) { core.probe("127.0.0.1", 1) }
    }

    @Test
    fun execReturnsStdoutStderrAndCode() {
        connect()
        val r = core.exec(ID, "sh -c 'echo out; echo err >&2; exit 7'", null, 0)
        assertEquals(7, r.code)
        assertEquals("out\n", r.stdout)
        assertEquals("err\n", r.stderr)
    }

    @Test
    fun execHandlesLargeOutputOnBothStreams() {
        connect()
        // ~4 MB on stdout and ~1 MB on stderr at the same time
        val r = core.exec(ID, "sh -c 'head -c 1000000 /dev/zero | tr \"\\0\" e >&2 & head -c 4000000 /dev/zero | tr \"\\0\" o; wait'", null, 30000)
        assertEquals(0, r.code)
        assertEquals(4000000, r.stdout.length)
        assertEquals(1000000, r.stderr.length)
    }

    @Test
    fun execWritesStdin() {
        connect()
        val r = core.exec(ID, "cat", "hello\nworld\n".toByteArray(Charsets.UTF_8), 0)
        assertEquals(0, r.code)
        assertEquals("hello\nworld\n", r.stdout)
        // without stdin the command sees EOF instead of hanging
        assertEquals(0, core.exec(ID, "cat", null, 5000).code)
    }

    @Test
    fun execTimeoutReturns124() {
        connect()
        val t = System.currentTimeMillis()
        val r = core.exec(ID, "sh -c 'echo started; sleep 10'", null, 1000)
        assertTrue("took ${System.currentTimeMillis() - t} ms", System.currentTimeMillis() - t < 5000)
        assertEquals(124, r.code)
        assertEquals("started\n", r.stdout)
        assertTrue(r.stderr, r.stderr.endsWith("timed out"))
    }

    @Test
    fun streamDeliversLinesInOrderAndExitCode() {
        connect()
        core.stream(ID, "s1", "sh -c 'i=0; while [ \$i -lt 500 ]; do echo line\$i; i=\$((i+1)); done; printf tail; exit 3'", false)
        assertEquals(3, events.awaitStreamExit("s1", 15000))
        val lines = events.lines("s1")
        assertEquals(501, lines.size)
        for (i in 0 until 500) assertEquals("line$i", lines[i])
        assertEquals("tail", lines[500])
    }

    @Test
    fun streamWithKeepStdinStaysOpen() {
        connect()
        // cat exits as soon as stdin closes
        core.stream(ID, "cat", "sh -c 'echo ready; cat'", true)
        events.awaitLine("cat", "ready", 10000)
        Thread.sleep(500)
        assertNull("cat exited although stdin was kept open", events.streamExitCode("cat"))
        core.streamKill("cat")
        assertEquals(143, events.awaitStreamExit("cat", 10000))
    }

    @Test
    fun streamKillStopsIt() {
        connect()
        core.stream(ID, "tick", "sh -c 'while true; do echo tick; sleep 0.1; done'", false)
        events.awaitLine("tick", "tick", 10000)
        core.streamKill("tick")
        assertEquals(143, events.awaitStreamExit("tick", 10000))
    }

    @Test
    fun ptyEchoAndResize() {
        connect()
        core.pty(ID, "p1", "sh -c 'echo hi; sleep 1; stty size'", 80, 24)
        events.awaitPtyOutput("p1", "hi", 10000)
        core.ptyResize("p1", 100, 30)
        assertEquals(0, events.awaitPtyExit("p1", 10000))
        assertTrue(events.ptyOutput("p1"), events.ptyOutput("p1").contains("30 100"))
    }

    @Test
    fun ptyWriteReachesTheProgram() {
        connect()
        core.pty(ID, "p2", "sh -c 'read line; echo got:\$line'", 80, 24)
        Thread.sleep(300)
        core.ptyWrite("p2", "abc\r".toByteArray(Charsets.UTF_8))
        assertEquals(0, events.awaitPtyExit("p2", 10000))
        assertTrue(events.ptyOutput("p2"), events.ptyOutput("p2").contains("got:abc"))
    }

    @Test
    fun ptyKillEndsIt() {
        connect()
        core.pty(ID, "p3", "sleep 30", 80, 24)
        Thread.sleep(300)
        core.ptyKill("p3")
        assertEquals(143, events.awaitPtyExit("p3", 10000))
    }

    @Test
    fun fifteenConcurrentStreamsUseMoreThanOneConnection() {
        connect()
        // MaxSessions defaults to 10 per connection, so these can't all fit on one
        val n = 15
        for (i in 0 until n) core.stream(ID, "c$i", "sh -c 'echo start; sleep 3; echo end'", false)
        for (i in 0 until n) events.awaitLine("c$i", "start", 20000)
        // with every stream still running, plain execs still get a session
        assertEquals("x\n", core.exec(ID, "echo x", null, 10000).stdout)
        for (i in 0 until n) {
            assertEquals(0, events.awaitStreamExit("c$i", 20000))
            assertEquals(listOf("start", "end"), events.lines("c$i"))
        }
    }

    @Test
    fun concurrentExecs() {
        connect()
        val ex = Executors.newFixedThreadPool(20)
        try {
            val results: List<Future<SshCore.ExecResult>> = (0 until 20).map { k ->
                ex.submit<SshCore.ExecResult> { core.exec(ID, "sh -c 'sleep 1; echo $k'", null, 20000) }
            }
            for (i in 0 until 20) assertEquals("$i\n", results[i].get(30, TimeUnit.SECONDS).stdout)
        } finally {
            ex.shutdownNow()
        }
    }

    @Test
    fun disconnectClosesEverything() {
        connect()
        core.stream(ID, "d1", "sh -c 'while true; do echo tick; sleep 0.2; done'", false)
        core.pty(ID, "d2", "sleep 60", 80, 24)
        events.awaitLine("d1", "tick", 10000)
        core.disconnect(ID)
        assertEquals(255, events.awaitStreamExit("d1", 10000))
        assertEquals(255, events.awaitPtyExit("d2", 10000))
        expectCode(SshCore.NOT_CONNECTED) { core.exec(ID, "true", null, 1000) }
        assertTrue("an explicit disconnect is not a drop", events.closedIds().isEmpty())
    }

    @Test
    fun droppedConnectionEmitsClosedAndEndsStreams() {
        connect()
        core.stream(ID, "drop", "sh -c 'while true; do echo tick; sleep 0.2; done'", false)
        events.awaitLine("drop", "tick", 10000)
        // the shell's parent is the sshd process serving this connection
        core.exec(ID, "sh -c 'kill -9 \$PPID'", null, 3000)
        assertEquals(255, events.awaitStreamExit("drop", 10000))
        events.awaitClosed(ID, 10000)
        expectCode(SshCore.NOT_CONNECTED) { core.exec(ID, "true", null, 1000) }
    }

    // ---- helpers -------------------------------------------------------------------------------

    // probed once: newer sshd versions penalise sources that keep connecting without logging in
    private fun hostKey(): String = hostKey ?: core.probe(HOST!!, port()).fingerprint.also { hostKey = it }

    private fun connect() {
        requireServer()
        core.connect(ID, HOST!!, port(), hostKey(), seedCredentials())
    }

    private companion object {
        val HOST: String? = System.getenv("INYON_SSH_TEST_HOST")
        val PORT: String? = System.getenv("INYON_SSH_TEST_PORT")
        val USER: String? = System.getenv("INYON_SSH_TEST_USER")
        val KEY: String? = System.getenv("INYON_SSH_TEST_KEY")
        val PASSWORD: String? = System.getenv("INYON_SSH_TEST_PASSWORD")
        const val ID = "test-server"

        var hostKey: String? = null

        fun requireServer() {
            assumeTrue("INYON_SSH_TEST_* not set", HOST != null && USER != null && KEY != null)
        }

        fun port() = if (PORT.isNullOrEmpty()) 22 else PORT.toInt()

        fun seedCredentials(): SshCore.Credentials {
            val key = OpenSshEd25519.parse(File(KEY!!).readText())
            return SshCore.Credentials(username = USER, ed25519Seed = key.seed, ed25519Public = key.publicKey)
        }

        fun expectCode(code: String, block: () -> Unit) {
            val e = assertThrows(SshCore.SshException::class.java) { block() }
            assertEquals(code, e.code)
        }
    }

    /** Collects events and lets tests wait for them. */
    private class Recorder : SshCore.Events {
        private val lock = ReentrantLock()
        private val changed = lock.newCondition()
        private val lines = HashMap<String, MutableList<String>>()
        private val streamExits = HashMap<String, Int>()
        private val ptyData = HashMap<String, ByteArrayOutputStream>()
        private val ptyExits = HashMap<String, Int>()
        private val closed = ArrayList<String>()

        private inline fun record(block: () -> Unit) = lock.withLock {
            block()
            changed.signalAll()
        }

        override fun streamLines(streamId: String, lines: List<String>) {
            record {
                if (streamExits.containsKey(streamId)) throw AssertionError("lines after exit on $streamId")
                this.lines.getOrPut(streamId) { ArrayList() }.addAll(lines)
            }
        }

        override fun streamExit(streamId: String, code: Int) {
            record { if (streamExits.put(streamId, code) != null) throw AssertionError("second exit on $streamId") }
        }

        override fun ptyData(ptyId: String, data: ByteArray) {
            record { ptyData.getOrPut(ptyId) { ByteArrayOutputStream() }.write(data, 0, data.size) }
        }

        override fun ptyExit(ptyId: String, code: Int) {
            record { ptyExits[ptyId] = code }
        }

        override fun closed(serverId: String, reason: String) {
            record { closed.add(serverId) }
        }

        fun lines(id: String): List<String> = lock.withLock { lines[id]?.toList() ?: emptyList() }

        fun streamExitCode(id: String): Int? = lock.withLock { streamExits[id] }

        fun ptyOutput(id: String): String = lock.withLock { ptyData[id]?.toByteArray()?.toString(Charsets.UTF_8) ?: "" }

        fun closedIds(): List<String> = lock.withLock { closed.toList() }

        fun awaitStreamExit(id: String, ms: Long): Int = await(ms, { "streamExit $id" }) { streamExits[id] }

        fun awaitPtyExit(id: String, ms: Long): Int = await(ms, { "ptyExit $id (output: ${ptyOutput(id)})" }) { ptyExits[id] }

        fun awaitLine(id: String, line: String, ms: Long) {
            await(ms, { "line '$line' on $id" }) { if (lines(id).contains(line)) true else null }
        }

        fun awaitClosed(id: String, ms: Long) {
            await(ms, { "closed $id" }) { if (closed.contains(id)) true else null }
        }

        fun awaitPtyOutput(id: String, text: String, ms: Long) {
            await(ms, { "'$text' on pty $id" }) { if (ptyOutput(id).contains(text)) true else null }
        }

        /** Waits until [value] is non-null. */
        private fun <T : Any> await(ms: Long, what: () -> String, value: () -> T?): T = lock.withLock {
            val end = System.currentTimeMillis() + ms
            var v = value()
            while (v == null) {
                val left = end - System.currentTimeMillis()
                if (left <= 0) throw AssertionError("timed out waiting for ${what()}")
                changed.await(left, TimeUnit.MILLISECONDS)
                v = value()
            }
            v
        }
    }
}
