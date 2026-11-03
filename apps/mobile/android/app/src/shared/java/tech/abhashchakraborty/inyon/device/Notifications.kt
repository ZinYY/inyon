package tech.abhashchakraborty.inyon.device

import android.Manifest
import android.app.Notification
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.graphics.Bitmap
import android.graphics.Canvas
import android.os.Build
import androidx.core.app.NotificationChannelCompat
import androidx.core.app.NotificationCompat
import androidx.core.app.NotificationManagerCompat
import androidx.core.content.ContextCompat
import tech.abhashchakraborty.inyon.R
import java.util.concurrent.atomic.AtomicInteger
import kotlin.math.roundToInt

/** An answer a question's notification offers: the agent's option number and its label. */
internal data class Choice(val n: Int, val label: String)

/**
 * Notification channels and everything the app posts. Every notification carries the Inyon mark
 * (the status-bar glyph, and the full logo inside) and buttons for what you'd do next; each session
 * has one notification that the next event replaces, grouped under a summary once there are several.
 */
internal object Notifications {

    const val ACTION_OPEN_HREF = "tech.abhashchakraborty.inyon.action.OPEN_HREF"
    const val EXTRA_HREF = "tech.abhashchakraborty.inyon.extra.HREF"
    const val CHANNEL_CONNECTION = "connection"

    /** an agent asking something: heads-up, it's waiting on you */
    const val CHANNEL_ATTENTION = "attention"

    /** an agent that finished */
    const val CHANNEL_AGENTS = "agents"
    const val CHANNEL_MACHINES = "machines"
    const val KEEP_ALIVE_ID = 7001

    /** the quiet notice [AnswerService] shows while it sends an answer */
    const val ANSWERING_ID = 7004

    private const val EVENT_ID = 7002
    private const val SUMMARY_ID = 7003
    private const val GROUP = "tech.abhashchakraborty.inyon.EVENTS"
    private const val ACCENT = 0xFF1D1C1A.toInt()
    private const val MACHINES_HREF = "/machines"
    private var logo: Bitmap? = null
    private val nextId = AtomicInteger((System.currentTimeMillis() / 1000 % 1_000_000).toInt() + 10_000)

    fun ensureChannels(ctx: Context) {
        val nm = NotificationManagerCompat.from(ctx)
        nm.createNotificationChannel(
            NotificationChannelCompat.Builder(CHANNEL_CONNECTION, NotificationManagerCompat.IMPORTANCE_LOW)
                .setName("Connection")
                .setDescription("Shown while Inyon stays connected in the background")
                .setShowBadge(false)
                .build(),
        )
        nm.createNotificationChannel(
            NotificationChannelCompat.Builder(CHANNEL_ATTENTION, NotificationManagerCompat.IMPORTANCE_HIGH)
                .setName("Needs you")
                .setDescription("An agent is asking something or waiting for approval")
                .build(),
        )
        nm.createNotificationChannel(
            NotificationChannelCompat.Builder(CHANNEL_AGENTS, NotificationManagerCompat.IMPORTANCE_DEFAULT)
                .setName("Finished")
                .setDescription("An agent finished its turn")
                .build(),
        )
        nm.createNotificationChannel(
            NotificationChannelCompat.Builder(CHANNEL_MACHINES, NotificationManagerCompat.IMPORTANCE_LOW)
                .setName("Machines")
                .setDescription("A server or laptop went offline or came back")
                .build(),
        )
    }

    /** The app's logo, drawn once at notification size. */
    @Synchronized
    private fun logo(ctx: Context): Bitmap? {
        logo?.let { return it }
        try {
            val d = ContextCompat.getDrawable(ctx, R.mipmap.ic_launcher_round) ?: return null
            val px = (64 * ctx.resources.displayMetrics.density).roundToInt()
            val b = Bitmap.createBitmap(px, px, Bitmap.Config.ARGB_8888)
            d.setBounds(0, 0, px, px)
            d.draw(Canvas(b))
            logo = b
        } catch (_: RuntimeException) {
            // no logo is better than no notification
        }
        return logo
    }

    fun canPost(ctx: Context): Boolean {
        if (
            Build.VERSION.SDK_INT >= 33 &&
            ContextCompat.checkSelfPermission(ctx, Manifest.permission.POST_NOTIFICATIONS) != PackageManager.PERMISSION_GRANTED
        ) {
            return false
        }
        return NotificationManagerCompat.from(ctx).areNotificationsEnabled()
    }

    /** The app's one launcher activity: Capacitor's MainActivity, or the Dioxus build's. */
    private fun launcher(ctx: Context): Intent {
        val component = ctx.packageManager.getLaunchIntentForPackage(ctx.packageName)?.component
        return if (component != null) Intent().setComponent(component) else Intent().setPackage(ctx.packageName)
    }

    /**
     * Opens (or brings back) the app; with an href, the plugin hands it to the web app. `tag` (or
     * failing that the href) is made part of the Intent's identity: Android matches PendingIntents
     * without their extras, so two sessions whose request codes collide would otherwise share one
     * and both open whichever href was posted last.
     */
    private fun openApp(ctx: Context, href: String?, requestCode: Int, tag: String? = null): PendingIntent {
        val intent = launcher(ctx)
        if (href != null) {
            // its own action, so Android never treats it as "just bring the task to the front"
            intent.setAction(ACTION_OPEN_HREF).putExtra(EXTRA_HREF, href)
            // the nonce only this install knows: another app's Intent with an href opens nothing (HrefGate)
            intent.putExtra(HrefGate.EXTRA_NONCE, HrefGate.nonce(ctx))
        } else {
            intent.setAction(Intent.ACTION_MAIN).addCategory(Intent.CATEGORY_LAUNCHER)
        }
        // not setData: Capacitor's App plugin would take any data as a URL to open
        val identity = tag ?: href
        if (identity != null) {
            if (Build.VERSION.SDK_INT >= 29) {
                intent.setIdentifier(identity)
            } else {
                // categories count towards identity too, and an Intent aimed at a component by name starts it whatever they say
                intent.addCategory("inyon.open.$identity")
            }
        }
        intent.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_SINGLE_TOP)
        return PendingIntent.getActivity(ctx, requestCode, intent, PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE)
    }

    /** A button handled by [NotificationActions] without opening the app. */
    private fun broadcast(ctx: Context, action: String, requestCode: Int, tag: String? = null): PendingIntent {
        val intent = Intent(ctx, NotificationActions::class.java).setAction(action).putExtra(NotificationActions.EXTRA_TAG, tag)
        return PendingIntent.getBroadcast(ctx, requestCode, intent, PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE)
    }

    private fun NotificationCompat.Builder.button(label: String, intent: PendingIntent) =
        addAction(NotificationCompat.Action.Builder(0, label, intent).build())

    /** Whether choosing `label` turns something down, which needs no unlock. */
    fun declines(label: String): Boolean = DECLINE.containsMatchIn(label.trim())

    private val DECLINE = Regex("^(no|deny|reject|decline|cancel|skip|stop|don'?t|do not)\\b", RegexOption.IGNORE_CASE)

    /** The buttons a question gets: one per answer, for one to three answers; none otherwise. */
    fun answerButtons(choices: List<Choice>): List<Pair<Choice, Boolean>> =
        if (choices.size in 1..3) choices.map { it to !declines(it.label) } else emptyList()

    /** The in-app path of a session, as the web app writes it. */
    fun sessionHref(host: String, pane: String): String = "/m/${android.net.Uri.encode(host)}/p/${pane.removePrefix("%")}"

    /**
     * A button that answers the question in place. One that approves something runs it on the
     * computer, so on the lock screen it waits for a fingerprint or the PIN first.
     */
    private fun answerAction(ctx: Context, c: Choice, unlock: Boolean, host: String, pane: String, tag: String, requestCode: Int): NotificationCompat.Action {
        val intent = Intent(ctx, NotificationActions::class.java)
            .setAction(NotificationActions.ANSWER)
            .putExtra(NotificationActions.EXTRA_TAG, tag)
            .putExtra(NotificationActions.EXTRA_HOST, host)
            .putExtra(NotificationActions.EXTRA_PANE, pane)
            .putExtra(NotificationActions.EXTRA_OPTION, c.n)
            .putExtra(NotificationActions.EXTRA_LABEL, c.label)
        val pi = PendingIntent.getBroadcast(ctx, requestCode, intent, PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE)
        return NotificationCompat.Action.Builder(0, c.label, pi).setAuthenticationRequired(unlock).build()
    }

    fun keepAlive(ctx: Context, title: String, text: String): Notification {
        ensureChannels(ctx)
        return NotificationCompat.Builder(ctx, CHANNEL_CONNECTION)
            .setSmallIcon(R.drawable.ic_stat_inyon)
            .setColor(ACCENT)
            .setContentTitle(title)
            .setContentText(text)
            .setContentIntent(openApp(ctx, null, KEEP_ALIVE_ID))
            .button("Stop", broadcast(ctx, NotificationActions.STOP_KEEP_ALIVE, KEEP_ALIVE_ID))
            .button("Open", openApp(ctx, null, KEEP_ALIVE_ID + 1))
            .setOngoing(true)
            .setOnlyAlertOnce(true)
            .setShowWhen(false)
            .setPriority(NotificationCompat.PRIORITY_LOW)
            .setCategory(NotificationCompat.CATEGORY_SERVICE)
            .setForegroundServiceBehavior(NotificationCompat.FOREGROUND_SERVICE_IMMEDIATE)
            .build()
    }

    /** Replaces the keep-alive text without restarting the service (allowed from the background). */
    fun updateKeepAlive(ctx: Context, title: String, text: String) {
        if (!canPost(ctx)) return
        try {
            NotificationManagerCompat.from(ctx).notify(KEEP_ALIVE_ID, keepAlive(ctx, title, text))
        } catch (_: SecurityException) {
            // permission revoked between the check and the call
        }
    }

    /**
     * An agent or machine event. `tag` names the session (or machine): its next event replaces
     * this one. `kind` is waiting, done, offline or online and picks the channel and the buttons.
     * False when notifications are off.
     */
    fun post(ctx: Context, title: String, body: String, href: String?, tag: String?, kind: String?, host: String? = null, pane: String? = null, choices: List<Choice> = emptyList()): Boolean {
        if (!canPost(ctx)) return false
        ensureChannels(ctx)
        val waiting = kind == "waiting" || kind == "permission"
        val machine = kind == "offline" || kind == "online"
        val channel = if (waiting) CHANNEL_ATTENTION else if (machine) CHANNEL_MACHINES else CHANNEL_AGENTS
        val id = if (tag != null) EVENT_ID else nextId.incrementAndGet()
        // each notification's buttons need their own PendingIntents, or the last one posted wins
        val requestCode = (tag?.hashCode() ?: id) * 4
        val b = NotificationCompat.Builder(ctx, channel)
            .setSmallIcon(R.drawable.ic_stat_inyon)
            .setColor(ACCENT)
            .setLargeIcon(logo(ctx))
            .setContentTitle(title)
            .setContentText(body)
            .setStyle(NotificationCompat.BigTextStyle().bigText(body))
            .setContentIntent(openApp(ctx, href, requestCode, tag))
            .setWhen(System.currentTimeMillis())
            .setShowWhen(true)
            .setAutoCancel(true)
            .setGroup(GROUP)
            .setPriority(if (waiting) NotificationCompat.PRIORITY_HIGH else if (machine) NotificationCompat.PRIORITY_LOW else NotificationCompat.PRIORITY_DEFAULT)
            .setCategory(if (waiting) NotificationCompat.CATEGORY_REMINDER else NotificationCompat.CATEGORY_STATUS)
        if (machine) {
            b.button("Open computers", openApp(ctx, MACHINES_HREF, requestCode + 1, tag))
        } else {
            val answers = if (waiting && tag != null && host != null && pane != null) answerButtons(choices) else emptyList()
            if (answers.isNotEmpty()) {
                // Android shows three buttons at most: the answers take them, and a tap still opens the session
                answers.forEachIndexed { i, (c, unlock) -> b.addAction(answerAction(ctx, c, unlock, host!!, pane!!, tag!!, requestCode + 1 + i)) }
            } else {
                b.button("Open", openApp(ctx, href, requestCode + 1, tag))
                // tagged ones only: an untagged notification has an id the button couldn't know
                if (waiting && tag != null) b.button("Mark read", broadcast(ctx, NotificationActions.MARK_READ, requestCode + 2, tag))
            }
        }
        return try {
            NotificationManagerCompat.from(ctx).notify(tag, id, b.build())
            summarize(ctx)
            true
        } catch (_: SecurityException) {
            // permission revoked between the check and the call
            false
        }
    }

    /** A session's notification while its answer is on the way: no buttons, no sound. */
    fun answering(ctx: Context, tag: String, title: String, text: String, href: String) {
        if (!canPost(ctx)) return
        val b = NotificationCompat.Builder(ctx, CHANNEL_ATTENTION)
            .setSmallIcon(R.drawable.ic_stat_inyon)
            .setColor(ACCENT)
            .setLargeIcon(logo(ctx))
            .setContentTitle(title)
            .setContentText(text)
            .setContentIntent(openApp(ctx, href, tag.hashCode() * 4, tag))
            .setOnlyAlertOnce(true)
            .setSilent(true)
            .setAutoCancel(true)
            .setGroup(GROUP)
        try {
            NotificationManagerCompat.from(ctx).notify(tag, EVENT_ID, b.build())
        } catch (_: SecurityException) {
            // permission revoked between the check and the call
        }
    }

    /** The quiet notice while an answer tapped on a notification is sent (the [AnswerService]'s own). */
    fun answeringNotice(ctx: Context, label: String, href: String, tag: String): Notification {
        ensureChannels(ctx)
        return NotificationCompat.Builder(ctx, CHANNEL_CONNECTION)
            .setSmallIcon(R.drawable.ic_stat_inyon)
            .setColor(ACCENT)
            .setContentTitle("Answering “$label”…")
            .setContentText("Inyon is sending your answer.")
            .setContentIntent(openApp(ctx, href, tag.hashCode() * 4, tag))
            .setOngoing(true)
            .setSilent(true)
            .setPriority(NotificationCompat.PRIORITY_LOW)
            .setForegroundServiceBehavior(NotificationCompat.FOREGROUND_SERVICE_IMMEDIATE)
            .build()
    }

    /** Takes a session's notification away (you opened it, or marked it read). */
    fun clear(ctx: Context, tag: String) {
        NotificationManagerCompat.from(ctx).cancel(tag, EVENT_ID)
        summarize(ctx)
    }

    /** One summary over two or more events, listing them; none for a single one. */
    private fun summarize(ctx: Context) {
        val nm = ctx.getSystemService(NotificationManager::class.java) ?: return
        val events = nm.activeNotifications.filter { it.notification.group == GROUP && it.id != SUMMARY_ID }
        if (events.size < 2) {
            nm.cancel(SUMMARY_ID)
            return
        }
        val inbox = NotificationCompat.InboxStyle()
        for (n in events) {
            val t = n.notification.extras.getCharSequence(Notification.EXTRA_TITLE)
            val x = n.notification.extras.getCharSequence(Notification.EXTRA_TEXT)
            inbox.addLine(if (x.isNullOrEmpty()) "$t" else "$t · $x")
        }
        val count = "${events.size} sessions"
        inbox.setSummaryText(count)
        val b = NotificationCompat.Builder(ctx, CHANNEL_AGENTS)
            .setSmallIcon(R.drawable.ic_stat_inyon)
            .setColor(ACCENT)
            .setLargeIcon(logo(ctx))
            .setContentTitle("Inyon")
            .setContentText(count)
            .setStyle(inbox)
            .setContentIntent(openApp(ctx, "/", SUMMARY_ID))
            .setGroup(GROUP)
            .setGroupSummary(true)
            .setGroupAlertBehavior(NotificationCompat.GROUP_ALERT_CHILDREN)
            .setAutoCancel(true)
        try {
            NotificationManagerCompat.from(ctx).notify(SUMMARY_ID, b.build())
        } catch (_: SecurityException) {
            // permission revoked between the check and the call
        }
    }
}

/** Notification buttons that act in place. Not exported: only our own PendingIntents reach it. */
class NotificationActions : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        when (intent.action) {
            MARK_READ -> intent.getStringExtra(EXTRA_TAG)?.let { Notifications.clear(context, it) }
            ANSWER -> {
                val tag = intent.getStringExtra(EXTRA_TAG) ?: return
                val host = intent.getStringExtra(EXTRA_HOST) ?: return
                val pane = intent.getStringExtra(EXTRA_PANE) ?: return
                val label = intent.getStringExtra(EXTRA_LABEL) ?: return
                val option = intent.getIntExtra(EXTRA_OPTION, -1)
                val href = Notifications.sessionHref(host, pane)
                val at = System.currentTimeMillis()
                // the web app sends it and then clears the session's notification, or posts why not;
                // AnswerService keeps the app awake for that, since Android may have frozen it
                if (DeviceEvents.answer(host, pane, option, label, tag, at)) {
                    if (AnswerService.start(context, tag, label, href, at)) {
                        Notifications.clear(context, tag)
                    } else {
                        Notifications.answering(context, tag, "Answering “$label”…", "Inyon is sending your answer.", href)
                    }
                } else {
                    Notifications.answering(context, tag, "Open Inyon to answer", "Inyon wasn't running, so “$label” wasn't sent.", href)
                }
            }
            STOP_KEEP_ALIVE -> {
                context.stopService(Intent(context, KeepAliveService::class.java))
                DeviceEvents.keepAliveEnded("stopped")
            }
        }
    }

    companion object {
        const val MARK_READ = "tech.abhashchakraborty.inyon.action.MARK_READ"
        const val ANSWER = "tech.abhashchakraborty.inyon.action.ANSWER"
        const val EXTRA_HOST = "host"
        const val EXTRA_PANE = "pane"
        const val EXTRA_OPTION = "option"
        const val EXTRA_LABEL = "label"
        const val STOP_KEEP_ALIVE = "tech.abhashchakraborty.inyon.action.STOP_KEEP_ALIVE"
        const val EXTRA_TAG = "tag"
    }
}
