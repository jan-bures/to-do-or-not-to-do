package app.todoornot.reminders

import android.Manifest
import android.app.AlarmManager
import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.net.Uri
import android.os.Build
import app.todoornot.MainActivity
import app.todoornot.R
import app.todoornot.core.ReminderAlarm
import java.time.ZoneId

/**
 * [AlarmSink] over the platform: `AlarmManager` alarms and notifications on the Reminders channel.
 *
 * Every reminder gets an inexact alarm (`setAndAllowWhileIdle`), and, when the owner allows exact alarms,
 * an exact one beside it (`setExactAndAllowWhileIdle`). Taking exact alarms away cancels the exact ones
 * without telling the app, so the inexact one is what still goes off; whichever goes off first shows the
 * reminder, and the other finds it shown.
 */
class AndroidAlarms(private val context: Context) : AlarmSink {
    private val dateFormat = app.todoornot.session.DateFormatPreference(context)
    private val alarms = context.getSystemService(AlarmManager::class.java)
    private val notifications = context.getSystemService(NotificationManager::class.java)

    init {
        notifications?.createNotificationChannel(
            NotificationChannel(CHANNEL, "Reminders", NotificationManager.IMPORTANCE_HIGH).apply {
                description = "A task’s reminder, when its time comes."
            },
        )
    }

    override fun exact(): Boolean = exactAllowed(context)

    override fun canShow(): Boolean = notificationsAllowed(context)

    override fun set(alarm: ReminderAlarm, atMs: Long) {
        val am = alarms ?: return
        am.setAndAllowWhileIdle(AlarmManager.RTC_WAKEUP, atMs, broadcast(ReminderReceiver.FIRE, alarm.id, alarm.task, BACKUP))
        val exact = broadcast(ReminderReceiver.FIRE, alarm.id, alarm.task, EXACT)
        if (!exactAllowed(context)) {
            am.cancel(exact)
            return
        }
        try {
            am.setExactAndAllowWhileIdle(AlarmManager.RTC_WAKEUP, atMs, exact)
        } catch (_: SecurityException) {
            // Taken away just now: the inexact alarm stands.
        }
    }

    override fun cancel(id: String) {
        val am = alarms ?: return
        am.cancel(broadcast(ReminderReceiver.FIRE, id, "", EXACT))
        am.cancel(broadcast(ReminderReceiver.FIRE, id, "", BACKUP))
    }

    override fun show(alarm: ReminderAlarm) {
        val nm = notifications ?: return
        val open = Intent(context, MainActivity::class.java)
            .setData(uri(alarm.id, NOTE))
            .putExtra(MainActivity.EXTRA_TASK, alarm.task)
            .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_CLEAR_TOP or Intent.FLAG_ACTIVITY_SINGLE_TOP)
        val n = Notification.Builder(context, CHANNEL)
            .setSmallIcon(R.drawable.ic_launcher_monochrome)
            .setContentTitle(alarm.title)
            .setContentText(reminderLabel(alarm.atMs, System.currentTimeMillis(), ZoneId.systemDefault(), dateFormat.load()))
            .setWhen(alarm.atMs)
            .setShowWhen(true)
            .setCategory(Notification.CATEGORY_REMINDER)
            .setAutoCancel(true)
            .setContentIntent(PendingIntent.getActivity(context, 0, open, FLAGS))
            .addAction(Notification.Action.Builder(null, "Complete", broadcast(ReminderReceiver.COMPLETE, alarm.id, alarm.task, NOTE)).build())
            .addAction(Notification.Action.Builder(null, "Snooze 10 min", broadcast(ReminderReceiver.SNOOZE, alarm.id, alarm.task, NOTE)).build())
            .build()
        try {
            nm.notify(alarm.id, NOTIFICATION, n)
        } catch (_: SecurityException) {
            // The permission was taken away between the check and the post.
        }
    }

    override fun hide(id: String) {
        notifications?.cancel(id, NOTIFICATION)
    }

    override fun hideAll() {
        val nm = notifications ?: return
        for (s in nm.activeNotifications) {
            if (s.notification.channelId == CHANNEL) nm.cancel(s.tag, s.id)
        }
    }

    /** One pending intent per reminder and purpose: the reminder's URI tells them apart. */
    private fun broadcast(action: String, id: String, task: String, kind: String): PendingIntent {
        val intent = Intent(context, ReminderReceiver::class.java)
            .setAction(action)
            .setData(uri(id, kind))
            .putExtra(ReminderReceiver.EXTRA_REMINDER, id)
            .putExtra(ReminderReceiver.EXTRA_TASK, task)
        return PendingIntent.getBroadcast(context, 0, intent, FLAGS)
    }

    companion object {
        /** The notification channel: "Reminders". */
        const val CHANNEL = "reminders"
        private const val NOTIFICATION = 1
        private const val FLAGS = PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT
        private const val EXACT = "exact"
        private const val BACKUP = "backup"
        private const val NOTE = "note"

        private fun uri(id: String, kind: String): Uri = Uri.fromParts("todo-reminder", id, kind)

        /**
         * Whether a reminder notification would be seen: the app may post notifications (Android 13 asks),
         * they are on for the app, and the Reminders channel isn't turned off.
         */
        fun notificationsAllowed(context: Context): Boolean {
            val granted = Build.VERSION.SDK_INT < Build.VERSION_CODES.TIRAMISU ||
                context.checkSelfPermission(Manifest.permission.POST_NOTIFICATIONS) == PackageManager.PERMISSION_GRANTED
            val nm = context.getSystemService(NotificationManager::class.java) ?: return false
            val channel = nm.getNotificationChannel(CHANNEL)
            return granted && nm.areNotificationsEnabled() && channel?.importance != NotificationManager.IMPORTANCE_NONE
        }

        /** Whether alarms can be exact (Android 12 and later ask; before, they always can). */
        fun exactAllowed(context: Context): Boolean =
            Build.VERSION.SDK_INT < Build.VERSION_CODES.S ||
                context.getSystemService(AlarmManager::class.java)?.canScheduleExactAlarms() == true
    }
}
