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
    /** Sets (or moves) the reminder's alarm to [atMs]: exact when the phone allows it, else inexact. */
    fun set(alarm: ReminderAlarm, atMs: Long)

    /** Cancels the reminder's alarm, if any. */
    fun cancel(id: String)

    /** Shows the reminder's notification: its task's title, with Complete and Snooze 10 min. */
    fun show(alarm: ReminderAlarm)

    /** Takes the reminder's notification away, if it is showing. */
    fun hide(id: String)
}

/**
 * Keeps the phone's alarms and notifications in line with the synced reminders ([ReminderPlan] decides,
 * [sink] does it), and remembers what it did in [store] (no task content). [reconcile] runs after every
 * change the engine reports, when an alarm goes off, after a reboot and after a clock or time-zone
 * change; the first pass in a process sets every alarm again, as a reboot clears them. Calls are
 * serialized; call them off the main thread.
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

    /** Brings the alarms in line with [engine]'s reminders. [force] sets every alarm again. */
    @Synchronized
    fun reconcile(engine: TodoEngine, force: Boolean = false) {
        val now = clock()
        val records = load()
        val alarms = engine.reminderAlarms(ReminderPlan.since(records, now), localNow(now, zone()))
        val plan = ReminderPlan.plan(alarms, records, now, force || fresh)
        fresh = false
        apply(plan, records)
        if (!plan.empty) debug("reminders: ${plan.set.size} set, ${plan.cancel.size} cancelled, ${plan.show.size} shown, ${plan.hide.size} hidden")
    }

    /** Snoozes a shown reminder by [ReminderPlan.SNOOZE_MS]: its notification goes, and its alarm is set again. */
    @Synchronized
    fun snooze(id: String, engine: TodoEngine?) {
        val records = load()
        val r = ReminderPlan.snooze(records, id, clock()) ?: return sink.hide(id)
        sink.hide(id)
        val alarm = engine?.let { e -> e.reminderAlarms(r.atMs, localNow(clock(), zone())).firstOrNull { it.id == id } }
        if (alarm == null) {
            // Gone meanwhile: the next pass with the engine forgets it.
            save(records - id)
            sink.cancel(id)
            return
        }
        sink.set(alarm, requireNotNull(r.alarmAt))
        save(records + (id to r))
    }

    /** The owner completed the reminder's task from its notification: the notification goes at once. */
    @Synchronized
    fun dismiss(id: String) = sink.hide(id)

    /** The phone was unpaired: every alarm and notification goes, and the records with them. */
    @Synchronized
    fun clear() {
        for (id in load().keys) {
            sink.cancel(id)
            sink.hide(id)
        }
        save(emptyMap())
    }

    private fun apply(plan: Plan, before: Map<String, Record>) {
        plan.cancel.forEach(sink::cancel)
        plan.hide.forEach(sink::hide)
        plan.set.forEach { (a, at) -> sink.set(a, at) }
        // Save before showing: a notification is never shown twice for the same time, even if the process
        // dies right after.
        if (plan.records != before) save(plan.records)
        plan.show.forEach(sink::show)
    }

    /** The records as written, keyed by reminder ID. */
    fun load(): Map<String, Record> = decode(store.get(KEY))

    private fun save(records: Map<String, Record>) = store.put(mapOf(KEY to encode(records)))

    companion object {
        private const val KEY = "records"

        fun encode(records: Map<String, Record>): String = JSONArray().apply {
            for (r in records.values) {
                put(
                    JSONObject()
                        .put("id", r.id)
                        .put("task", r.task)
                        .put("at", r.atMs)
                        .putOpt("alarm", r.alarmAt)
                        .putOpt("shown", r.shownAt)
                        .putOpt("snooze", r.snoozeUntil),
                )
            }
        }.toString()

        /** Reads [encode]'s records; anything unreadable counts as none (every alarm is then set again). */
        fun decode(text: String?): Map<String, Record> {
            if (text == null) return emptyMap()
            return runCatching {
                val a = JSONArray(text)
                (0 until a.length()).associate { i ->
                    val o = a.getJSONObject(i)
                    fun opt(k: String) = if (o.has(k) && !o.isNull(k)) o.getLong(k) else null
                    val r = Record(o.getString("id"), o.getString("task"), o.getLong("at"), opt("alarm"), opt("shown"), opt("snooze"))
                    r.id to r
                }
            }.getOrDefault(emptyMap())
        }
    }
}
