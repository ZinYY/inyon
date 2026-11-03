package tech.abhashchakraborty.inyon.device

import android.content.Context
import android.media.AudioAttributes
import android.media.AudioManager
import android.os.Build
import android.os.VibrationAttributes
import android.os.VibrationEffect
import android.os.Vibrator
import android.os.VibratorManager
import android.provider.Settings
import android.view.HapticFeedbackConstants
import android.view.View

/**
 * The app's haptic vocabulary on the system vibrator. Touch feedback follows the system's touch
 * haptics setting; `event` (an agent finished or needs you) is an alert and follows the ringer.
 */
internal class Haptics(private val context: Context) {

    private val vibrator: Vibrator? =
        if (Build.VERSION.SDK_INT >= 31) {
            context.getSystemService(VibratorManager::class.java)?.defaultVibrator
        } else {
            context.getSystemService(Vibrator::class.java)
        }

    /** False when the device can't vibrate or the user turned this kind of feedback off. */
    fun play(kind: String, view: View?): Boolean {
        val v = vibrator ?: return false
        if (!v.hasVibrator()) return false
        val alert = kind == "event"
        if (if (alert) ringerSilent() else !touchFeedbackOn()) return false

        if (Build.VERSION.SDK_INT < 26) return legacy(v, kind)
        val effect = when (kind) {
            // every touch is one light tick; only a confirmed outcome gets more
            "tap" -> predefined(VibrationEffect.EFFECT_TICK) ?: oneShot(v, 8, 90)
            "select" -> predefined(VibrationEffect.EFFECT_TICK) ?: oneShot(v, 8, 90)
            "long" -> predefined(VibrationEffect.EFFECT_HEAVY_CLICK) ?: oneShot(v, 40, 200)
            "success" -> {
                clicks(v, 0.7f, 1f) ?: run {
                    // CONFIRM is tuned per device, better than a waveform on motors without primitives
                    if (Build.VERSION.SDK_INT >= 30 && view != null) {
                        view.post { view.performHapticFeedback(HapticFeedbackConstants.CONFIRM) }
                        return true
                    }
                    waveform(v, longArrayOf(0, 14, 70, 18), intArrayOf(0, 150, 0, 230))
                }
            }
            "warning" -> waveform(v, longArrayOf(0, 22, 80, 22), intArrayOf(0, 230, 0, 230))
            "error" -> waveform(v, longArrayOf(0, 28, 50, 28, 50, 28), intArrayOf(0, 255, 0, 200, 0, 255))
            "event" -> waveform(v, EVENT, intArrayOf(0, 255, 0, 255))
            else -> return false
        }
        vibrate(v, effect, alert)
        return true
    }

    private fun touchFeedbackOn() =
        @Suppress("DEPRECATION")
        Settings.System.getInt(context.contentResolver, Settings.System.HAPTIC_FEEDBACK_ENABLED, 1) != 0

    private fun ringerSilent() = context.getSystemService(AudioManager::class.java)?.ringerMode == AudioManager.RINGER_MODE_SILENT

    private fun predefined(id: Int): VibrationEffect? = if (Build.VERSION.SDK_INT >= 29) VibrationEffect.createPredefined(id) else null

    private fun oneShot(v: Vibrator, ms: Long, amplitude: Int): VibrationEffect =
        VibrationEffect.createOneShot(ms, if (v.hasAmplitudeControl()) amplitude else VibrationEffect.DEFAULT_AMPLITUDE)

    /** `timings` alternate off and on, starting with off. */
    private fun waveform(v: Vibrator, timings: LongArray, amplitudes: IntArray): VibrationEffect {
        if (v.hasAmplitudeControl()) return VibrationEffect.createWaveform(timings, amplitudes, -1)
        // on/off motors spin up slowly, so very short pulses barely register
        val stretched = LongArray(timings.size) { i -> if (i % 2 == 1) maxOf(timings[i], 20) else timings[i] }
        return VibrationEffect.createWaveform(stretched, -1)
    }

    /** Crisp clicks 70 ms apart from the actuator's own primitives, where it has them. */
    private fun clicks(v: Vibrator, vararg scales: Float): VibrationEffect? {
        if (Build.VERSION.SDK_INT < 30 || !v.areAllPrimitivesSupported(VibrationEffect.Composition.PRIMITIVE_CLICK)) return null
        val c = VibrationEffect.startComposition()
        scales.forEachIndexed { i, scale -> c.addPrimitive(VibrationEffect.Composition.PRIMITIVE_CLICK, scale, if (i == 0) 0 else 70) }
        return c.compose()
    }

    @Suppress("DEPRECATION")
    private fun vibrate(v: Vibrator, effect: VibrationEffect, alert: Boolean) {
        if (Build.VERSION.SDK_INT >= 33) {
            v.vibrate(effect, VibrationAttributes.createForUsage(if (alert) VibrationAttributes.USAGE_NOTIFICATION else VibrationAttributes.USAGE_TOUCH))
        } else {
            val usage = if (alert) AudioAttributes.USAGE_NOTIFICATION_EVENT else AudioAttributes.USAGE_ASSISTANCE_SONIFICATION
            v.vibrate(effect, AudioAttributes.Builder().setUsage(usage).setContentType(AudioAttributes.CONTENT_TYPE_SONIFICATION).build())
        }
    }

    /** Android 7: plain on/off vibration, no amplitudes. */
    @Suppress("DEPRECATION")
    private fun legacy(v: Vibrator, kind: String): Boolean {
        when (kind) {
            "tap" -> v.vibrate(20)
            "select" -> v.vibrate(15)
            "long" -> v.vibrate(45)
            "success" -> v.vibrate(longArrayOf(0, 20, 70, 25), -1)
            "warning" -> v.vibrate(longArrayOf(0, 30, 80, 30), -1)
            "error" -> v.vibrate(longArrayOf(0, 30, 50, 30, 50, 30), -1)
            "event" -> v.vibrate(EVENT, -1)
            else -> return false
        }
        return true
    }

    companion object {
        val KINDS = setOf("tap", "select", "long", "success", "warning", "error", "event")

        // ~60 ms, a 100 ms gap, then ~90 ms: noticeable from a pocket
        private val EVENT = longArrayOf(0, 60, 100, 90)
    }
}
