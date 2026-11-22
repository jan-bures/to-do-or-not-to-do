package app.todoornot

import app.todoornot.core.ReminderAlarm
import app.todoornot.core.RoomKey
import app.todoornot.core.SmartList
import app.todoornot.core.TodoEngine
import app.todoornot.reminders.AlarmSink
import app.todoornot.reminders.Record
import app.todoornot.reminders.ReminderAccess
import app.todoornot.reminders.ReminderNeed
import app.todoornot.reminders.ReminderPlan
import app.todoornot.reminders.Reminders
import app.todoornot.ui.reminderMillis
import app.todoornot.ui.reminderStart
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import java.time.LocalDateTime
import java.time.ZoneId
import java.time.ZoneOffset
import java.util.TimeZone

private const val MIN = 60_000L
private const val HOUR = 60 * MIN

/** The fixtures' zone: UTC+1, as [FIXTURE_NOW] has it. */
private val ZONE: TimeZone = TimeZone.getTimeZone(ZoneOffset.ofHours(1))

/** What the scheduler did, in place of `AlarmManager` and the notifications. */
private class FakeAlarms : AlarmSink {
    /** The alarms set now, by reminder. */
    val alarms = HashMap<String, Long>()

    /** Notifications showing now, by reminder, with their titles. */
    val showing = HashMap<String, String>()

    /** Every notification shown, in order: its title. */
    val shown = ArrayList<String>()

    var sets = 0

    override fun set(alarm: ReminderAlarm, atMs: Long) {
        alarms[alarm.id] = atMs
        sets++
    }

    override fun cancel(id: String) {
        alarms.remove(id)
    }

    override fun show(alarm: ReminderAlarm) {
        showing[alarm.id] = alarm.title
        shown += alarm.title
    }

    override fun hide(id: String) {
        showing.remove(id)
    }
}

class RemindersTest {
    /** The clock the scheduler reads: [FIXTURE_NOW] to start with. */
    private var now = FIXTURE_NOW.epochMs

    private val store = MapValues()
    private val sink = FakeAlarms()

    private fun scheduler() = Reminders(store, sink, clock = { now }, zone = { ZONE })

    private fun TodoEngine.reminderOf(title: String): ReminderAlarm =
        reminderAlarms(0, FIXTURE_NOW).first { it.title == title }

    private fun TodoEngine.taskTitled(title: String): String =
        room(RoomKey.Smart(SmartList.ALL), FIXTURE_NOW).tasks.first { it.title == title }.id

    private fun alarm(id: String, at: Long, task: String = "t-$id") = ReminderAlarm(id, task, "Task $id", at, "label")

    @Test
    fun every_reminder_to_come_gets_an_alarm_at_its_time() {
        val e = fixtureEngine()
        val r = scheduler()
        r.reconcile(e)
        val expected = e.reminderAlarms(now, FIXTURE_NOW).associate { it.id to it.atMs }
        assertEquals(5, expected.size)
        assertEquals(expected, sink.alarms)
        assertTrue(sink.shown.isEmpty())
        // Nothing changed: nothing is set again.
        val sets = sink.sets
        r.reconcile(e)
        assertEquals(sets, sink.sets)
        // Forced (a reboot, the clock changed), every alarm is set again.
        r.reconcile(e, force = true)
        assertEquals(sets + expected.size, sink.sets)
    }

    @Test
    fun the_first_pass_in_a_process_sets_every_alarm_again() {
        val e = fixtureEngine()
        scheduler().reconcile(e)
        sink.alarms.clear() // A reboot clears them.
        scheduler().reconcile(e)
        assertEquals(5, sink.alarms.size)
    }

    @Test
    fun a_reminder_shows_once_when_its_time_comes() {
        val e = fixtureEngine()
        val r = scheduler()
        r.reconcile(e)
        val venue = e.reminderOf("Send venue booking confirmation")
        now = venue.atMs
        r.reconcile(e) // The alarm went off.
        assertEquals(listOf("Send venue booking confirmation"), sink.shown)
        assertEquals("Send venue booking confirmation", sink.showing[venue.id])
        assertNull(sink.alarms[venue.id])
        // Later passes, and a restart, never show it again.
        now += 5 * MIN
        r.reconcile(e)
        scheduler().reconcile(e)
        assertEquals(1, sink.shown.size)
    }

    @Test
    fun alarms_go_with_removed_reminders_and_completed_or_deleted_tasks() {
        val e = fixtureEngine()
        val r = scheduler()
        r.reconcile(e)
        // Its reminder removed: the alarm goes.
        val passport = e.reminderOf("Renew passport appointment")
        e.removeReminder(passport.task, FIXTURE_NOW)
        r.reconcile(e)
        assertNull(sink.alarms[passport.id])
        // Its task completed, then reopened: the alarm goes, and comes back.
        val roadmap = e.reminderOf("Prepare Q4 roadmap review")
        val done = requireNotNull(e.setDone(roadmap.task, true))
        r.reconcile(e)
        assertNull(sink.alarms[roadmap.id])
        e.undoCompletion(done)
        r.reconcile(e)
        assertEquals(roadmap.atMs, sink.alarms[roadmap.id])
        // Its task deleted elsewhere: the alarm goes, and so does a notification showing.
        val venue = e.reminderOf("Send venue booking confirmation")
        now = venue.atMs
        r.reconcile(e)
        assertTrue(venue.id in sink.showing)
        e.fixtureDeleteTask(venue.task)
        r.reconcile(e)
        assertTrue(venue.id !in sink.showing)
        assertTrue(venue.id !in sink.alarms)
        assertEquals(setOf(roadmap.id, e.reminderOf("Koupit rohlíky a mléko").id, e.reminderOf("Call Alex about the venue").id), sink.alarms.keys)
    }

    @Test
    fun a_moved_reminder_moves_its_alarm() {
        val e = fixtureEngine()
        val r = scheduler()
        r.reconcile(e)
        val call = e.reminderOf("Call Alex about the venue")
        val later = call.atMs + HOUR
        e.setReminder(call.task, later, FIXTURE_NOW)
        r.reconcile(e)
        assertEquals(later, sink.alarms[call.id])
    }

    @Test
    fun snoozing_hides_the_notification_and_shows_it_again_ten_minutes_later() {
        val e = fixtureEngine()
        val r = scheduler()
        r.reconcile(e)
        val venue = e.reminderOf("Send venue booking confirmation")
        now = venue.atMs + MIN
        r.reconcile(e)
        r.snooze(venue.id, e)
        assertTrue(venue.id !in sink.showing)
        assertEquals(now + ReminderPlan.SNOOZE_MS, sink.alarms[venue.id])
        // Syncs in between change nothing; the snooze isn't synced, and the reminder keeps its time.
        r.reconcile(e)
        assertEquals(now + ReminderPlan.SNOOZE_MS, sink.alarms[venue.id])
        assertEquals(venue.atMs, e.reminderOf("Send venue booking confirmation").atMs)
        now += ReminderPlan.SNOOZE_MS
        r.reconcile(e)
        assertEquals(2, sink.shown.size)
        assertTrue(venue.id in sink.showing)
    }

    @Test
    fun missed_reminders_from_the_last_day_show_once_and_older_ones_never() {
        val e = fixtureEngine()
        val r = scheduler()
        // The phone was off for a while: the venue reminder is 3 hours late, the passport one 30 hours.
        val venue = e.reminderOf("Send venue booking confirmation")
        val passport = e.reminderOf("Renew passport appointment")
        now = venue.atMs + 3 * HOUR
        r.reconcile(e)
        assertEquals(listOf("Send venue booking confirmation"), sink.shown.filter { it == "Send venue booking confirmation" })
        now = passport.atMs + 30 * HOUR
        sink.shown.clear()
        r.reconcile(e)
        assertTrue("older than a day: never shown", "Renew passport appointment" !in sink.shown)
        r.reconcile(e)
        assertTrue("Renew passport appointment" !in sink.shown)
        // Shown ones are forgotten once out of the window, and still never shown again.
        assertTrue(r.load().values.all { it.fireAt >= now - ReminderPlan.MISSED_WINDOW_MS })
    }

    @Test
    fun unpairing_clears_every_alarm_and_notification() {
        val e = fixtureEngine()
        val r = scheduler()
        r.reconcile(e)
        now = e.reminderOf("Send venue booking confirmation").atMs
        r.reconcile(e)
        r.clear()
        assertTrue(sink.alarms.isEmpty())
        assertTrue(sink.showing.isEmpty())
        assertTrue(r.load().isEmpty())
    }

    @Test
    fun the_plan_is_pure_and_its_records_round_trip() {
        val t = 1_000_000L
        val records = mapOf(
            "gone" to Record("gone", "t", t - HOUR, shownAt = t - HOUR),
            "moved" to Record("moved", "t", t + HOUR, alarmAt = t + HOUR),
            "same" to Record("same", "t", t + HOUR, alarmAt = t + HOUR),
        )
        val plan = ReminderPlan.plan(listOf(alarm("moved", t + 2 * HOUR), alarm("same", t + HOUR), alarm("new", t - MIN)), records, t)
        assertEquals(listOf("moved" to t + 2 * HOUR), plan.set.map { (a, at) -> a.id to at })
        // The gone one had no alarm left (it was shown); the moved one's goes before it is set again.
        assertEquals(listOf("moved"), plan.cancel)
        assertEquals(listOf("gone"), plan.hide)
        assertEquals(listOf("new"), plan.show.map { it.id })
        assertEquals(setOf("moved", "same", "new"), plan.records.keys)
        assertEquals(plan.records, Reminders.decode(Reminders.encode(plan.records)))
        assertEquals(emptyMap<String, Record>(), Reminders.decode("not json"))
        assertEquals(t - HOUR - ReminderPlan.MISSED_WINDOW_MS, ReminderPlan.since(records + ("old" to Record("old", "t", t - HOUR - ReminderPlan.MISSED_WINDOW_MS)), t))
    }

    @Test
    fun reminder_access_is_asked_once_each_and_only_where_android_asks() {
        val none = emptySet<ReminderNeed>()
        assertEquals(ReminderNeed.NOTIFICATIONS, ReminderAccess.next(33, notifications = false, exact = false, asked = none))
        assertEquals(ReminderNeed.EXACT_ALARMS, ReminderAccess.next(33, notifications = false, exact = false, asked = setOf(ReminderNeed.NOTIFICATIONS)))
        assertNull(ReminderAccess.next(33, notifications = false, exact = false, asked = ReminderNeed.entries.toSet()))
        assertNull(ReminderAccess.next(34, notifications = true, exact = true, asked = none))
        // Android 12 has no notification permission to ask for; Android 10 neither the exact-alarm one.
        assertEquals(ReminderNeed.EXACT_ALARMS, ReminderAccess.next(31, notifications = false, exact = false, asked = none))
        assertNull(ReminderAccess.next(29, notifications = false, exact = false, asked = none))
    }

    @Test
    fun the_picker_opens_on_the_reminder_or_the_due_day_or_the_next_hour() {
        val zone: ZoneId = ZoneOffset.ofHours(1)
        val at = LocalDateTime.of(2026, 11, 22, 15, 0)
        val atMs = at.atZone(zone).toInstant().toEpochMilli()
        assertEquals(at, reminderStart(atMs, null, FIXTURE_NOW.epochMs, zone))
        assertEquals(LocalDateTime.of(2026, 11, 24, 9, 0), reminderStart(null, "2026-11-24", FIXTURE_NOW.epochMs, zone))
        // Due today, or not at all: the next whole hour (10:35 → 11:00).
        assertEquals(LocalDateTime.of(2026, 11, 21, 11, 0), reminderStart(null, "2026-11-21", FIXTURE_NOW.epochMs, zone))
        assertEquals(LocalDateTime.of(2026, 11, 21, 11, 0), reminderStart(null, null, FIXTURE_NOW.epochMs, zone))
        // The picker's day (its UTC midnight) and time, in the phone's zone.
        val day = at.toLocalDate().toEpochDay() * 86_400_000L
        assertEquals(atMs, reminderMillis(day, 15, 0, zone))
    }
}
