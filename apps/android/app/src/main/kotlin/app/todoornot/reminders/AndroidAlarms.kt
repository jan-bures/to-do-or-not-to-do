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

/**
 * [AlarmSink] over the platform: `AlarmManager` alarms (exact with `setExactAndAllowWhileIdle` when the
 * phone allows exact alarms, else `setAndAllowWhileIdle`, which Android may deliver a little late) and
 * notifications on the Reminders channel.
 */
class AndroidAlarms(private val context: Context) : AlarmSink {
    private val alarms = context.getSystemService(AlarmManager::class.java)
    private val notifications = context.getSystemService(NotificationManager::class.java)

    init {
        notifications?.createNotificationChannel(
            NotificationChannel(CHANNEL, "Reminders", NotificationManager.IMPORTANCE_HIGH).apply {
                description = "A task’s reminder, when its time comes."
            },
        )
    }

    override fun set(alarm: ReminderAlarm, atMs: Long) {
        val am = alarms ?: return
        val pi = broadcast(ReminderReceiver.FIRE, alarm.id, alarm.task)
        try {
            if (exactAllowed(context)) {
                am.setExactAndAllowWhileIdle(AlarmManager.RTC_WAKEUP, atMs, pi)
            } else {
                am.setAndAllowWhileIdle(AlarmManager.RTC_WAKEUP, atMs, pi)
            }
        } catch (_: SecurityException) {
            // The exact-alarm permission was taken away just now.
            am.setAndAllowWhileIdle(AlarmManager.RTC_WAKEUP, atMs, pi)
        }
    }

    override fun cancel(id: String) {
        alarms?.cancel(broadcast(ReminderReceiver.FIRE, id, ""))
    }

    override fun show(alarm: ReminderAlarm) {
        val nm = notifications ?: return
        if (!notificationsAllowed(context)) return
        val open = Intent(context, MainActivity::class.java)
            .setData(uri(alarm.id))
            .putExtra(MainActivity.EXTRA_TASK, alarm.task)
            .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_CLEAR_TOP or Intent.FLAG_ACTIVITY_SINGLE_TOP)
        val n = Notification.Builder(context, CHANNEL)
            .setSmallIcon(R.drawable.ic_launcher_monochrome)
            .setContentTitle(alarm.title)
            .setContentText(alarm.label)
            .setWhen(alarm.atMs)
            .setShowWhen(true)
            .setCategory(Notification.CATEGORY_REMINDER)
            .setAutoCancel(true)
            .setContentIntent(PendingIntent.getActivity(context, 0, open, FLAGS))
            .addAction(Notification.Action.Builder(null, "Complete", broadcast(ReminderReceiver.COMPLETE, alarm.id, alarm.task)).build())
            .addAction(Notification.Action.Builder(null, "Snooze 10 min", broadcast(ReminderReceiver.SNOOZE, alarm.id, alarm.task)).build())
            .build()
        nm.notify(alarm.id, NOTIFICATION, n)
    }

    override fun hide(id: String) {
        notifications?.cancel(id, NOTIFICATION)
    }

    /** One pending intent per reminder and action: the reminder's URI tells them apart. */
    private fun broadcast(action: String, id: String, task: String): PendingIntent {
        val intent = Intent(context, ReminderReceiver::class.java)
            .setAction(action)
            .setData(uri(id))
            .putExtra(ReminderReceiver.EXTRA_REMINDER, id)
            .putExtra(ReminderReceiver.EXTRA_TASK, task)
        return PendingIntent.getBroadcast(context, 0, intent, FLAGS)
    }

    companion object {
        /** The notification channel: "Reminders". */
        const val CHANNEL = "reminders"
        private const val NOTIFICATION = 1
        private const val FLAGS = PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT

        private fun uri(id: String): Uri = Uri.fromParts("todo-reminder", id, null)

        /** Whether reminders can show: the app may post notifications (Android 13 asks), and they are on. */
        fun notificationsAllowed(context: Context): Boolean {
            val granted = Build.VERSION.SDK_INT < Build.VERSION_CODES.TIRAMISU ||
                context.checkSelfPermission(Manifest.permission.POST_NOTIFICATIONS) == PackageManager.PERMISSION_GRANTED
            return granted && context.getSystemService(NotificationManager::class.java)?.areNotificationsEnabled() != false
        }

        /** Whether alarms can be exact (Android 12 and later ask; before, they always can). */
        fun exactAllowed(context: Context): Boolean =
            Build.VERSION.SDK_INT < Build.VERSION_CODES.S ||
                context.getSystemService(AlarmManager::class.java)?.canScheduleExactAlarms() == true
    }
}
