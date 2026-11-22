package app.todoornot.reminders

import android.app.AlarmManager
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import app.todoornot.TodoApplication

/**
 * A reminder's alarm going off, its notification's Complete and Snooze 10 min, and the system's word that
 * the alarms must be set again: after a reboot or an app update, when the clock or the time zone changed,
 * or when the exact-alarm permission changed. The work runs off the main thread, within the broadcast's
 * time ([goAsync]).
 */
class ReminderReceiver : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        val graph = (context.applicationContext as? TodoApplication)?.graph ?: return
        val id = intent.getStringExtra(EXTRA_REMINDER)
        val task = intent.getStringExtra(EXTRA_TASK)
        val action = intent.action
        val work: (suspend () -> Unit)? = when (action) {
            FIRE -> { { graph.reconcileReminders() } }
            COMPLETE -> if (id != null && task != null) { { graph.completeFromReminder(id, task) } } else null
            SNOOZE -> if (id != null) { { graph.snoozeReminder(id) } } else null
            in RESCHEDULE -> { { graph.reconcileReminders(force = true) } }
            else -> null
        }
        work ?: return
        val pending = goAsync()
        graph.reminderWork(pending::finish, work)
    }

    companion object {
        const val FIRE = "app.todoornot.reminder.FIRE"
        const val COMPLETE = "app.todoornot.reminder.COMPLETE"
        const val SNOOZE = "app.todoornot.reminder.SNOOZE"
        const val EXTRA_REMINDER = "reminder"
        const val EXTRA_TASK = "task"

        /** The system broadcasts after which every alarm is set again. */
        val RESCHEDULE = setOf(
            Intent.ACTION_BOOT_COMPLETED,
            Intent.ACTION_MY_PACKAGE_REPLACED,
            Intent.ACTION_TIME_CHANGED,
            Intent.ACTION_TIMEZONE_CHANGED,
            AlarmManager.ACTION_SCHEDULE_EXACT_ALARM_PERMISSION_STATE_CHANGED,
        )
    }
}
