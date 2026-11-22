package app.todoornot.reminders

import app.todoornot.core.ReminderAlarm

/**
 * The alarm the phone keeps for one listed reminder: the time it was set for when this was written
 * ([atMs]; a reminder moved since starts afresh), the alarm set for it ([alarmAt]), and a snooze
 * ([snoozeUntil]), which is this phone's own and never synced. No task content: the title is read from
 * the engine when the notification is shown.
 */
data class Record(
    val id: String,
    val task: String,
    val atMs: Long,
    val alarmAt: Long? = null,
    val snoozeUntil: Long? = null,
) {
    /** When it fires next: the snooze, else the reminder's own time. */
    val fireAt: Long get() = snoozeUntil ?: atMs
}

/**
 * One occurrence of a reminder that has been shown: the reminder, and the time it was shown for (its own
 * time, or a snooze's). Kept for [ReminderPlan.MISSED_WINDOW_MS] whatever becomes of the reminder, so one
 * removed and brought back by Undo, or synced away and back, never shows the same occurrence twice.
 */
data class Shown(val id: String, val fireAt: Long)

/** What the phone has done: the alarms it keeps, by reminder, and the occurrences it has shown. */
data class Schedule(val records: Map<String, Record> = emptyMap(), val shown: Set<Shown> = emptySet())

/**
 * What to do to bring the alarms and notifications in line with the reminders: alarms to [set] (by
 * reminder, at a time) and to [cancel], notifications to [show] now and to [hide], and the [schedule] to
 * keep afterwards.
 */
data class Plan(
    val set: List<Pair<ReminderAlarm, Long>>,
    val cancel: List<String>,
    val show: List<ReminderAlarm>,
    val hide: List<String>,
    val schedule: Schedule,
) {
    val empty: Boolean get() = set.isEmpty() && cancel.isEmpty() && show.isEmpty() && hide.isEmpty()
}

/**
 * The scheduling policy, pure: which alarms to set or cancel and which notifications to show or hide, for
 * the reminders the engine lists ([app.todoornot.core.TodoEngine.reminderAlarms]) and what was done before.
 *
 * - A reminder in the future gets an alarm at its time (at its snooze, if snoozed). One already set for
 *   that time isn't set again, unless [plan] is `force`d: when the process starts (a reboot clears every
 *   alarm), after a reboot, a clock or time-zone change, and when exact alarms were allowed or taken away
 *   (taking them away cancels the exact ones).
 * - A reminder whose time has come is shown, once for that time ([Shown]): the alarm going off, or the app
 *   finding it due when it starts or syncs. A missed one (the phone was off, or it synced in late) is
 *   shown once if its time is within the last [MISSED_WINDOW_MS] (24 hours), and never if it is older.
 *   While it can't be shown (`canShow` false: notifications are off, or a catch-up sync is still under
 *   way) it isn't counted as shown, so it shows once it can, still within the window.
 * - A reminder that isn't listed any more (removed, or its task completed or deleted, here or elsewhere)
 *   loses its alarm and its notification. So does the old time of a moved reminder, which starts afresh.
 * - An occurrence shown longer ago than the window is forgotten, and its notification, if still up, goes
 *   with it: the app no longer follows it, so it couldn't take it away when the task is completed.
 */
object ReminderPlan {
    /** How far back a missed reminder is still shown, once. */
    const val MISSED_WINDOW_MS = 24L * 60 * 60 * 1000

    /** What Snooze puts off a reminder by. */
    const val SNOOZE_MS = 10L * 60 * 1000

    /** The earliest reminder time to ask the engine for: the window's start, or an older record's. */
    fun since(schedule: Schedule, now: Long): Long =
        minOf(now - MISSED_WINDOW_MS, schedule.records.values.minOfOrNull { it.atMs } ?: Long.MAX_VALUE)

    fun plan(
        alarms: List<ReminderAlarm>,
        before: Schedule,
        now: Long,
        force: Boolean = false,
        canShow: Boolean = true,
    ): Plan {
        val set = ArrayList<Pair<ReminderAlarm, Long>>()
        val cancel = ArrayList<String>()
        val show = ArrayList<ReminderAlarm>()
        val hide = LinkedHashSet<String>()
        val records = LinkedHashMap<String, Record>()
        val windowStart = now - MISSED_WINDOW_MS
        val shown = before.shown.filterTo(LinkedHashSet()) { it.fireAt >= windowStart }
        // Aged out: no longer followed, so a notification still up goes now.
        for (old in before.shown - shown) if (shown.none { it.id == old.id }) hide += old.id
        val listed = alarms.associateBy { it.id }
        for ((id, old) in before.records) {
            val a = listed[id]
            if (a != null && a.atMs == old.atMs) continue
            // Gone, or moved: its alarm and its notification go with it.
            if (old.alarmAt != null || force) cancel += id
            if (before.shown.any { it.id == id }) hide += id
        }
        for (a in alarms) {
            val r = before.records[a.id]?.takeIf { it.atMs == a.atMs } ?: Record(a.id, a.task, a.atMs)
            val fire = r.fireAt
            when {
                fire > now -> {
                    if (force || r.alarmAt != fire) set += a to fire
                    records[a.id] = r.copy(alarmAt = fire)
                }
                Shown(a.id, fire) in shown -> records[a.id] = r.copy(alarmAt = null)
                fire >= windowStart -> if (canShow) {
                    // Its alarm, if it hasn't gone off yet (a sync found it due first), would only repeat it.
                    if (r.alarmAt != null) cancel += a.id
                    show += a
                    shown += Shown(a.id, fire)
                    records[a.id] = r.copy(alarmAt = null)
                } else {
                    // Not shown yet: kept as it is for a pass that can show it.
                    records[a.id] = r
                }
                // Missed by more than the window: never shown.
                else -> Unit
            }
        }
        return Plan(set, cancel, show, hide.toList(), Schedule(records, shown))
    }

    /**
     * Snoozes a reminder: its notification goes, and it fires again [SNOOZE_MS] from [now]. Null when
     * there's no record of it (it has gone meanwhile).
     */
    fun snooze(schedule: Schedule, id: String, now: Long): Record? =
        schedule.records[id]?.copy(snoozeUntil = now + SNOOZE_MS, alarmAt = now + SNOOZE_MS)
}
