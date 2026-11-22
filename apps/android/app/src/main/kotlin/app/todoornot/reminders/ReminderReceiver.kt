package app.todoornot.reminders

import android.app.AlarmManager
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import app.todoornot.AppGraph
import app.todoornot.TodoApplication

/**
 * A reminder's alarm going off, its notification's Complete and Snooze 10 min, and the system's word that
 * the alarms must be set again: after a reboot or an app update, when the clock or the time zone changed,
 * or when exact alarms were allowed. The work runs off the main thread under one deadline for the whole
 * broadcast ([AppGraph.RECEIVER_BUDGET_MS], well inside the system's limit for [goAsync]).
 */
class ReminderReceiver : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        val graph = (context.applicationContext as? TodoApplication)?.graph ?: return
        val id = intent.getStringExtra(EXTRA_REMINDER)
        val task = intent.getStringExtra(EXTRA_TASK)
        val work: (suspend (AppGraph.Deadline) -> Unit)? = when (intent.action) {
            FIRE -> { d -> graph.reminderFired(d) }
            COMPLETE -> if (id != null && task != null) { d -> graph.completeFromReminder(id, task, d) } else null
            SNOOZE -> if (id != null) { _ -> graph.snoozeReminder(id) } else null
            in RESCHEDULE -> { _ -> graph.reconcileReminders(force = true) }
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
