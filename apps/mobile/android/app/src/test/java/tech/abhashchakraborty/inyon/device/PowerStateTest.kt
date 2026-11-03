package tech.abhashchakraborty.inyon.device

import java.io.File
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class PowerStateTest {

    @Test
    fun theClockKeepsWhenDozeBeganAndEnded() {
        val clock = PowerState.IdleClock()
        assertEquals(0L, clock.since)
        clock.changed(false, 50)
        // not idle before it ever dozed: nothing ended
        assertEquals(0L, clock.ended)
        clock.changed(true, 100)
        clock.changed(true, 150)
        assertEquals(100L, clock.since)
        clock.changed(false, 400)
        assertEquals(400L, clock.ended)
        clock.changed(false, 450)
        assertEquals(400L, clock.ended)
        // the next Doze starts a new span
        clock.changed(true, 900)
        assertEquals(900L, clock.since)
        assertEquals(400L, clock.ended)
    }

    @Test
    fun standbyBucketsHaveNames() {
        assertEquals("active", PowerState.bucketName(10))
        assertEquals("working set", PowerState.bucketName(20))
        assertEquals("frequent", PowerState.bucketName(30))
        assertEquals("rare", PowerState.bucketName(40))
        assertEquals("restricted", PowerState.bucketName(45))
        assertEquals("never", PowerState.bucketName(50))
    }

    @Test
    fun theFieldsAreTheOnesTheWebAppReads() {
        val s = PowerState.Snapshot(
            idle = true, powerSave = false, ignoringOptimizations = false, backgroundRestricted = true,
            keepAlive = true, onScreen = false, idleSince = 10, idleEnded = 5, standbyBucket = 40,
        )
        val f = s.fields()
        // apps/web/src/lib/connlog.ts `Power`
        val names = setOf("idle", "powerSave", "ignoringOptimizations", "backgroundRestricted", "keepAlive", "onScreen", "idleSince", "idleEnded", "standbyBucket")
        assertEquals(names, f.keys)
        assertEquals("rare", f["standbyBucket"])
        assertEquals(true, f["backgroundRestricted"])
        // before Android 9 there is no bucket, and none is sent
        assertFalse(s.copy(standbyBucket = null).fields().containsKey("standbyBucket"))
        val web = File("../../../web/src/lib/connlog.ts").readText()
        for (n in names) assertTrue(n, web.contains("  $n?:"))
    }

    @Test
    fun thePluginReportsItAndFollowsDoze() {
        val plugin = File("src/main/java/tech/abhashchakraborty/inyon/device/InyonDevicePlugin.kt").readText()
        assertTrue(plugin.contains("fun powerState(call: PluginCall)"))
        assertTrue(plugin.contains("PowerManager.ACTION_DEVICE_IDLE_MODE_CHANGED"))
        // and the web app asks for it by that name
        val native = File("../../../web/src/lib/native.ts").readText()
        assertTrue(native.contains("device.powerState()"))
    }
}
