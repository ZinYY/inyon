package tech.abhashchakraborty.inyon.device

/**
 * Doze, battery limits and Stay connected, as the web app's connection log records them when a
 * link drops and as "Can't connect?" shows them. Plain values here; the plugin reads them from
 * Android (InyonDevicePlugin.powerState).
 */
object PowerState {

    /**
     * When Doze last began and ended. The app is often frozen while the phone dozes and only sees
     * a dropped link once it wakes, so the web app compares the drop with these times.
     */
    class IdleClock {
        @Volatile var since = 0L
            private set
        @Volatile var ended = 0L
            private set

        fun changed(idle: Boolean, now: Long) {
            if (idle) {
                // a repeated "idle" keeps the first one
                if (since == 0L || ended >= since) since = now
            } else if (since > ended) {
                ended = now
            }
        }
    }

    data class Snapshot(
        val idle: Boolean,
        val powerSave: Boolean,
        val ignoringOptimizations: Boolean,
        /** "Restricted" in Android's battery settings for the app (Android 9 and later) */
        val backgroundRestricted: Boolean,
        val keepAlive: Boolean,
        val onScreen: Boolean,
        val idleSince: Long,
        val idleEnded: Long,
        /** UsageStatsManager's standby bucket (Android 9 and later), null before */
        val standbyBucket: Int?,
    ) {
        /** What the web app gets: the names native.ts's `Power` reads. */
        fun fields(): Map<String, Any> = buildMap {
            put("idle", idle)
            put("powerSave", powerSave)
            put("ignoringOptimizations", ignoringOptimizations)
            put("backgroundRestricted", backgroundRestricted)
            put("keepAlive", keepAlive)
            put("onScreen", onScreen)
            put("idleSince", idleSince)
            put("idleEnded", idleEnded)
            standbyBucket?.let { put("standbyBucket", bucketName(it)) }
        }
    }

    /** UsageStatsManager's buckets by name: the further down, the less Android lets the app run. */
    fun bucketName(bucket: Int): String = when {
        bucket <= 5 -> "exempted"
        bucket <= 10 -> "active"
        bucket <= 20 -> "working set"
        bucket <= 30 -> "frequent"
        bucket <= 40 -> "rare"
        bucket <= 45 -> "restricted"
        else -> "never"
    }
}
