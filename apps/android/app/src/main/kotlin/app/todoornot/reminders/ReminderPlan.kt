package app.todoornot.reminders

import app.todoornot.core.ReminderAlarm

/**
 * What the phone has done about one reminder, kept across restarts: the time it was set for when this
 * was written ([atMs]; a reminder moved since starts afresh), the alarm set for it ([alarmAt]), the time
 * it was last shown for ([shownAt]), and a snooze ([snoozeUntil]), which is this phone's own and never
 * synced. No task content: the title is read from the engine when the notification is shown.
 */
data class Record(
    val id: String,
    val task: String,
    val atMs: Long,
    val alarmAt: Long? = null,
    val shownAt: Long? = null,
    val snoozeUntil: Long? = null,
) {
    /** When it fires next: the snooze, else the reminder's own time. */
    val fireAt: Long get() = snoozeUntil ?: atMs
}

/**
 * What to do to bring the alarms and notifications in line with the reminders: alarms to [set] (by
 * reminder, at a time) and to [cancel], notifications to [show] now and to [hide], and the [records] to
 * keep afterwards.
 */
data class Plan(
    val set: List<Pair<ReminderAlarm, Long>>,
    val cancel: List<String>,
    val show: List<ReminderAlarm>,
    val hide: List<String>,
    val records: Map<String, Record>,
) {
    val empty: Boolean get() = set.isEmpty() && cancel.isEmpty() && show.isEmpty() && hide.isEmpty()
}

/**
 * The scheduling policy, pure: which alarms to set or cancel and which notifications to show or hide, for
 * the reminders the engine lists ([app.todoornot.core.TodoEngine.reminderAlarms]) and what was done before.
 *
 * - A reminder in the future gets an alarm at its time (at its snooze, if snoozed). One already set for
 *   that time isn't set again, unless [plan] is `force`d (after a reboot, a clock or time-zone change, or
 *   when the process starts, as a reboot clears every alarm).
 * - A reminder whose time has come is shown, once for that time: the alarm going off, or the app finding
 *   it due when it starts or syncs. A missed one (the phone was off, or it synced in late) is shown once
 *   if its time is within the last [MISSED_WINDOW_MS] (24 hours), and never if it is older.
 * - A reminder that isn't listed any more (removed, or its task completed or deleted, here or elsewhere)
 *   loses its alarm and its notification. So does the old time of a moved reminder, which starts afresh.
 * - A record whose reminder was shown longer ago than the window is forgotten: the engine is asked only
 *   for reminders from [since] on, so it never comes back.
 */
object ReminderPlan {
    /** How far back a missed reminder is still shown, once. */
    const val MISSED_WINDOW_MS = 24L * 60 * 60 * 1000

    /** What Snooze puts off a reminder by. */
    const val SNOOZE_MS = 10L * 60 * 1000

    /** The earliest reminder time to ask the engine for: the window's start, or an older record's. */
    fun since(records: Map<String, Record>, now: Long): Long =
        minOf(now - MISSED_WINDOW_MS, records.values.minOfOrNull { it.atMs } ?: Long.MAX_VALUE)

    fun plan(alarms: List<ReminderAlarm>, records: Map<String, Record>, now: Long, force: Boolean = false): Plan {
        val set = ArrayList<Pair<ReminderAlarm, Long>>()
        val cancel = ArrayList<String>()
        val show = ArrayList<ReminderAlarm>()
        val hide = ArrayList<String>()
        val next = LinkedHashMap<String, Record>()
        val listed = alarms.associateBy { it.id }
        for ((id, old) in records) {
            val a = listed[id]
            if (a != null && a.atMs == old.atMs) continue
            // Gone, or moved: its alarm and its notification go with it.
            if (old.alarmAt != null || force) cancel += id
            if (old.shownAt != null) hide += id
        }
        for (a in alarms) {
            val r = records[a.id]?.takeIf { it.atMs == a.atMs } ?: Record(a.id, a.task, a.atMs)
            val fire = r.fireAt
            when {
                r.shownAt == fire -> {
                    // Shown for this time; forgotten once that is past the window.
                    if (fire >= now - MISSED_WINDOW_MS) next[a.id] = r.copy(alarmAt = null)
                }
                fire > now -> {
                    if (force || r.alarmAt != fire) set += a to fire
                    next[a.id] = r.copy(alarmAt = fire)
                }
                fire >= now - MISSED_WINDOW_MS -> {
                    // Its alarm, if it hasn't gone off yet (a sync found it due first), would only repeat it.
                    if (r.alarmAt != null) cancel += a.id
                    show += a
                    next[a.id] = r.copy(alarmAt = null, shownAt = fire)
                }
                // Missed by more than the window: never shown.
                else -> Unit
            }
        }
        return Plan(set, cancel, show, hide, next)
    }

    /**
     * Snoozes a shown reminder: its notification goes, and it fires again [SNOOZE_MS] from [now]. Null when
     * there's no record of it (it has gone meanwhile).
     */
    fun snooze(records: Map<String, Record>, id: String, now: Long): Record? =
        records[id]?.copy(snoozeUntil = now + SNOOZE_MS, alarmAt = now + SNOOZE_MS)
}
