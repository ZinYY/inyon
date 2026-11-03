package tech.abhashchakraborty.inyon.app

import android.app.Application

/**
 * Starts the phone core with the process, before any window: a notification's button or the
 * keep-alive service may be what brought the process up, and the core has to be there for them.
 */
class InyonApp : Application() {
    override fun onCreate() {
        super.onCreate()
        Native.start(this)
    }
}
