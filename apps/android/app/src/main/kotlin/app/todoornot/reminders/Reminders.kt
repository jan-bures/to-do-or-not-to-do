package app.todoornot.reminders

import app.todoornot.core.ReminderAlarm
import app.todoornot.core.TodoEngine
import app.todoornot.core.localNow
import app.todoornot.session.KeyValues
import org.json.JSONArray
import org.json.JSONObject
import java.util.TimeZone

/** Where alarms and notifications go: the system's `AlarmManager` and notifications, or a fake in tests. */
interface AlarmSink {
    /** Whether alarms can be exact now (the owner may allow it, or take it away, at any time). */
    fun exact(): Boolean

    /** Whether a notification would be seen: the app may post them, and the Reminders channel is on. */
    fun canShow(): Boolean

    /**
     * Sets (or moves) the reminder's alarm to [atMs]: exact when allowed, with an inexact one beside it
     * that still goes off if exact alarms are taken away (which cancels the exact ones).
     */
    fun set(alarm: ReminderAlarm, atMs: Long)

    /** Cancels the reminder's alarms, if any. */
    fun cancel(id: String)

    /** Shows the reminder's notification: its task's title, with Complete and Snooze 10 min. */
    fun show(alarm: ReminderAlarm)

    /** Takes the reminder's notification away, if it is showing. */
    fun hide(id: String)

    /** Takes every reminder notification away (unpairing). */
    fun hideAll()
}

/**
 * Keeps the phone's alarms and notifications in line with the synced reminders ([ReminderPlan] decides,
 * [sink] does it), and remembers what it did in [store] (reminder IDs and times, no task content).
 * [reconcile] runs after every change the engine reports, when the app comes to the foreground, when an
 * alarm goes off, after a reboot and after a clock or time-zone change. The first pass in a process sets
 * every alarm again (a reboot clears them), and so does any pass that finds exact alarms allowed or taken
 * away since the last. Calls are serialized; call them off the main thread.
 */
class Reminders(
    private val store: KeyValues,
    private val sink: AlarmSink,
    private val clock: () -> Long = System::currentTimeMillis,
    private val zone: () -> TimeZone = TimeZone::getDefault,
    /** Debug-level log lines: counts only, never task content. */
    private val debug: (String) -> Unit = {},
) {
    private var fresh = true

    /**
     * Brings the alarms in line with [engine]'s reminders. [force] sets every alarm again. While
     * [catchingUp] (a sync is still bringing in what changed elsewhere), nothing due is shown yet.
     */
    @Synchronized
    fun reconcile(engine: TodoEngine, force: Boolean = false, catchingUp: Boolean = false) {
        val now = clock()
        val before = load()
        val alarms = engine.reminderAlarms(ReminderPlan.since(before, now), localNow(now, zone().toZoneId()))
        val exact = sink.exact()
        val wasExact = store.get(EXACT)?.toBooleanStrictOrNull()
        val plan = ReminderPlan.plan(alarms, before, now, force || fresh || wasExact != exact, canShow = !catchingUp && sink.canShow())
        fresh = false
        if (wasExact != exact) store.put(mapOf(EXACT to exact.toString()))
        apply(plan, before)
        if (!plan.empty) debug("reminders: ${plan.set.size} set, ${plan.cancel.size} cancelled, ${plan.show.size} shown, ${plan.hide.size} hidden")
    }

    /** Snoozes a reminder by [ReminderPlan.SNOOZE_MS]: its notification goes, and its alarm is set again. */
    @Synchronized
    fun snooze(id: String, engine: TodoEngine?) {
        val before = load()
        sink.hide(id)
        val r = ReminderPlan.snooze(before, id, clock()) ?: return
        val alarm = engine?.let { e -> e.reminderAlarms(r.atMs, localNow(clock(), zone().toZoneId())).firstOrNull { it.id == id && it.atMs == r.atMs } }
        if (alarm == null) {
            // Gone meanwhile: the next pass with the engine forgets it.
            sink.cancel(id)
            return
        }
        sink.set(alarm, requireNotNull(r.alarmAt))
        save(before.copy(records = before.records + (id to r)))
    }

    /** The owner completed the reminder's task from its notification, and it was saved: the notification goes. */
    @Synchronized
    fun dismiss(id: String) = sink.hide(id)

    /** The phone was unpaired: every alarm and reminder notification goes, and what was kept with them. */
    @Synchronized
    fun clear() {
        for (id in load().records.keys) sink.cancel(id)
        sink.hideAll()
        save(Schedule())
    }

    private fun apply(plan: Plan, before: Schedule) {
        plan.cancel.forEach(sink::cancel)
        plan.hide.forEach(sink::hide)
        plan.set.forEach { (a, at) -> sink.set(a, at) }
        // Save before showing: an occurrence is never shown twice, even if the process dies right after.
        // [Plan.show] is only planned when the sink can show it.
        if (plan.schedule != before) save(plan.schedule)
        plan.show.forEach(sink::show)
    }

    /** What was done, as written. */
    fun load(): Schedule = decode(store.get(KEY))

    private fun save(schedule: Schedule) = store.put(mapOf(KEY to encode(schedule)))

    companion object {
        private const val KEY = "schedule"
        private const val EXACT = "exact"

        fun encode(s: Schedule): String {
            val records = JSONArray()
            for (r in s.records.values) {
                records.put(
                    JSONObject()
                        .put("id", r.id)
                        .put("task", r.task)
                        .put("at", r.atMs)
                        .putOpt("alarm", r.alarmAt)
                        .putOpt("snooze", r.snoozeUntil),
                )
            }
            val shown = JSONArray()
            for (o in s.shown) shown.put(JSONObject().put("id", o.id).put("at", o.fireAt))
            return JSONObject().put("records", records).put("shown", shown).toString()
        }

        /** Reads [encode]'s schedule; anything unreadable counts as none (every alarm is then set again). */
        fun decode(text: String?): Schedule {
            if (text == null) return Schedule()
            return runCatching {
                val o = JSONObject(text)
                val records = o.getJSONArray("records")
                val shown = o.getJSONArray("shown")
                Schedule(
                    (0 until records.length()).associate { i ->
                        val r = records.getJSONObject(i)
                        fun opt(k: String) = if (r.has(k) && !r.isNull(k)) r.getLong(k) else null
                        val record = Record(r.getString("id"), r.getString("task"), r.getLong("at"), opt("alarm"), opt("snooze"))
                        record.id to record
                    },
                    (0 until shown.length()).mapTo(LinkedHashSet()) { i ->
                        val s = shown.getJSONObject(i)
                        Shown(s.getString("id"), s.getLong("at"))
                    },
                )
            }.getOrDefault(Schedule())
        }
    }
}
