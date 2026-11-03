package tech.abhashchakraborty.inyon.device

import android.app.Activity
import android.content.pm.ActivityInfo

/**
 * Phones stay upright; tablets and open foldables turn with the device. "Phone" is Android's own
 * line for a large screen: under 600dp on the shortest side. Android 16 ignores the lock on large
 * screens anyway, so this only ever holds a phone.
 */
object Orientation {
    const val TABLET_DP = 600

    fun apply(activity: Activity) {
        val phone = activity.resources.configuration.smallestScreenWidthDp < TABLET_DP
        val want = if (phone) ActivityInfo.SCREEN_ORIENTATION_PORTRAIT else ActivityInfo.SCREEN_ORIENTATION_UNSPECIFIED
        if (activity.requestedOrientation != want) activity.requestedOrientation = want
    }
}
