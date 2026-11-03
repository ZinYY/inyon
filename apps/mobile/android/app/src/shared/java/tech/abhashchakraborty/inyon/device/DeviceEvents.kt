package tech.abhashchakraborty.inyon.device

/**
 * What the services and notification buttons tell the app, whichever app it is: the Capacitor
 * build's plugin, or the Dioxus build's bridge to Rust. Each sets [sink] while it can hear them.
 */
object DeviceEvents {
    interface Sink {
        /** Stay connected stopped on its own: Android's time limit, an error, or its Stop button. */
        fun keepAliveEnded(reason: String)

        /** An answer tapped on a notification is being sent (true), or that's over (false). */
        fun answerAwake(on: Boolean)

        /** An answer tapped on a notification, to send; false when nothing can send it. */
        fun answer(host: String, pane: String, option: Int, label: String, tag: String, at: Long): Boolean
    }

    @Volatile var sink: Sink? = null

    fun keepAliveEnded(reason: String) {
        sink?.keepAliveEnded(reason)
    }

    fun answerAwake(on: Boolean) {
        sink?.answerAwake(on)
    }

    fun answer(host: String, pane: String, option: Int, label: String, tag: String, at: Long): Boolean =
        sink?.answer(host, pane, option, label, tag, at) ?: false
}
