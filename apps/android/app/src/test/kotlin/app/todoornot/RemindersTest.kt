package app.todoornot

import app.todoornot.core.ReminderAlarm
import app.todoornot.core.TodoEngine
import app.todoornot.reminders.AlarmSink
import app.todoornot.reminders.Record
import app.todoornot.reminders.ReminderAccess
import app.todoornot.reminders.ReminderNeed
import app.todoornot.reminders.ReminderPlan
import app.todoornot.reminders.Reminders
import app.todoornot.reminders.Schedule
import app.todoornot.reminders.Shown
import app.todoornot.reminders.clockChangeNote
import app.todoornot.reminders.reminderLabel
import app.todoornot.reminders.reminderStart
import app.todoornot.reminders.resolveReminder
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import java.time.Instant
import java.time.LocalDate
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

    /** Whether exact alarms are allowed; taking them away cancels the alarms set while they were. */
    var exact = true

    /** Whether notifications are allowed and the channel is on. */
    var allowed = true

    override fun exact() = exact

    override fun canShow() = allowed

    override fun set(alarm: ReminderAlarm, atMs: Long) {
        alarms[alarm.id] = atMs
        sets++
    }

    override fun cancel(id: String) {
        alarms.remove(id)
    }

    override fun show(alarm: ReminderAlarm) {
        check(allowed) { "shown while notifications are off" }
        showing[alarm.id] = alarm.title
        shown += alarm.title
    }

    override fun hide(id: String) {
        showing.remove(id)
    }

    override fun hideAll() = showing.clear()
}

class RemindersTest {
    /** The clock the scheduler reads: [FIXTURE_NOW] to start with. */
    private var now = FIXTURE_NOW.epochMs

    private val store = MapValues()
    private val sink = FakeAlarms()

    private fun scheduler() = Reminders(store, sink, clock = { now }, zone = { ZONE })

    private fun TodoEngine.reminderOf(title: String): ReminderAlarm =
        reminderAlarms(0, FIXTURE_NOW).first { it.title == title }

    /** The reminder as the detail pane shows it, for an edit. */
    private fun TodoEngine.seen(task: String) = task(task, FIXTURE_NOW)?.reminder

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
    fun taking_exact_alarms_away_sets_every_alarm_again_and_so_does_allowing_them() {
        val e = fixtureEngine()
        val r = scheduler()
        r.reconcile(e)
        // Android cancels the exact alarms and says nothing; the next pass (a sync, the app opening, any
        // broadcast) sees it and sets them all again, inexact.
        sink.exact = false
        sink.alarms.clear()
        r.reconcile(e)
        assertEquals(5, sink.alarms.size)
        val sets = sink.sets
        r.reconcile(e)
        assertEquals("only once", sets, sink.sets)
        sink.exact = true
        r.reconcile(e)
        assertEquals(sets + 5, sink.sets)
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
    fun a_reminder_that_could_not_be_shown_shows_once_notifications_are_allowed() {
        val e = fixtureEngine()
        val r = scheduler()
        r.reconcile(e)
        val venue = e.reminderOf("Send venue booking confirmation")
        sink.allowed = false
        now = venue.atMs
        r.reconcile(e)
        assertTrue(sink.shown.isEmpty())
        assertTrue(r.load().shown.isEmpty())
        // Allowed two hours later: still within the day, so it shows, once.
        now += 2 * HOUR
        sink.allowed = true
        r.reconcile(e)
        r.reconcile(e)
        assertEquals(listOf("Send venue booking confirmation"), sink.shown)
    }

    @Test
    fun nothing_due_shows_while_a_catch_up_sync_is_under_way() {
        val e = fixtureEngine()
        val r = scheduler()
        r.reconcile(e)
        val venue = e.reminderOf("Send venue booking confirmation")
        now = venue.atMs
        // The alarm went off; the sync brings in that the reminder was removed elsewhere.
        r.reconcile(e, catchingUp = true)
        assertTrue(sink.shown.isEmpty())
        e.removeReminder(venue.task, requireNotNull(e.seen(venue.task)))
        r.reconcile(e)
        assertTrue(sink.shown.isEmpty())
    }

    @Test
    fun alarms_go_with_removed_reminders_and_completed_or_deleted_tasks() {
        val e = fixtureEngine()
        val r = scheduler()
        r.reconcile(e)
        // Its reminder removed: the alarm goes.
        val passport = e.reminderOf("Renew passport appointment")
        e.removeReminder(passport.task, requireNotNull(e.seen(passport.task)))
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
    fun a_reminder_removed_and_brought_back_by_undo_never_shows_the_same_time_twice() {
        val e = fixtureEngine()
        val r = scheduler()
        r.reconcile(e)
        val venue = e.reminderOf("Send venue booking confirmation")
        now = venue.atMs
        r.reconcile(e)
        assertEquals(1, sink.shown.size)
        val removed = e.removeReminder(venue.task, requireNotNull(e.seen(venue.task)))
        r.reconcile(e)
        assertTrue(venue.id !in sink.showing)
        e.undoReminder(removed)
        now += MIN
        r.reconcile(e)
        scheduler().reconcile(e)
        assertEquals("shown once, not again after Undo", 1, sink.shown.size)
    }

    @Test
    fun a_moved_reminder_moves_its_alarm() {
        val e = fixtureEngine()
        val r = scheduler()
        r.reconcile(e)
        val call = e.reminderOf("Call Alex about the venue")
        val later = call.atMs + HOUR
        e.setReminder(call.task, e.seen(call.task), later, FIXTURE_NOW)
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
        assertEquals(1, sink.shown.count { it == "Send venue booking confirmation" })
        now = passport.atMs + 30 * HOUR
        sink.shown.clear()
        r.reconcile(e)
        assertTrue("older than a day: never shown", "Renew passport appointment" !in sink.shown)
        r.reconcile(e)
        assertTrue("Renew passport appointment" !in sink.shown)
        // Occurrences shown longer ago than the window are forgotten, and their notification goes with them.
        assertTrue(r.load().shown.all { it.fireAt >= now - ReminderPlan.MISSED_WINDOW_MS })
        assertTrue(venue.id !in sink.showing)
    }

    @Test
    fun unpairing_clears_every_alarm_and_notification() {
        val e = fixtureEngine()
        val r = scheduler()
        r.reconcile(e)
        now = e.reminderOf("Send venue booking confirmation").atMs
        r.reconcile(e)
        sink.showing["left-over"] = "A notification the records no longer name"
        r.clear()
        assertTrue(sink.alarms.isEmpty())
        assertTrue(sink.showing.isEmpty())
        assertEquals(Schedule(), r.load())
    }

    @Test
    fun the_plan_is_pure_and_its_schedule_round_trips() {
        val t = 1_000_000_000L
        val before = Schedule(
            mapOf(
                "gone" to Record("gone", "t", t - HOUR),
                "moved" to Record("moved", "t", t + HOUR, alarmAt = t + HOUR),
                "same" to Record("same", "t", t + HOUR, alarmAt = t + HOUR),
            ),
            setOf(Shown("gone", t - HOUR), Shown("old", t - ReminderPlan.MISSED_WINDOW_MS - MIN)),
        )
        val plan = ReminderPlan.plan(listOf(alarm("moved", t + 2 * HOUR), alarm("same", t + HOUR), alarm("new", t - MIN)), before, t)
        assertEquals(listOf("moved" to t + 2 * HOUR), plan.set.map { (a, at) -> a.id to at })
        assertEquals(listOf("moved"), plan.cancel)
        // The aged-out occurrence's notification goes; so does the gone reminder's.
        assertEquals(setOf("old", "gone"), plan.hide.toSet())
        assertEquals(listOf("new"), plan.show.map { it.id })
        assertEquals(setOf("moved", "same", "new"), plan.schedule.records.keys)
        // The gone reminder's occurrence is kept: if it comes back, it isn't shown again.
        assertEquals(setOf(Shown("gone", t - HOUR), Shown("new", t - MIN)), plan.schedule.shown)
        assertEquals(plan.schedule, Reminders.decode(Reminders.encode(plan.schedule)))
        assertEquals(Schedule(), Reminders.decode("not json"))
        // Unable to show: nothing is shown or counted as shown.
        val blocked = ReminderPlan.plan(listOf(alarm("new", t - MIN)), Schedule(), t, canShow = false)
        assertTrue(blocked.show.isEmpty())
        assertTrue(blocked.schedule.shown.isEmpty())
        assertEquals(t - HOUR - ReminderPlan.MISSED_WINDOW_MS, ReminderPlan.since(Schedule(mapOf("x" to Record("x", "t", t - HOUR - ReminderPlan.MISSED_WINDOW_MS))), t))
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
        // The picker's day and time, in the phone's zone.
        val t = resolveReminder(at.toLocalDate(), 15, 0, zone)
        assertEquals(atMs, t.atMs)
        assertFalse(t.skipped || t.twice)
        assertNull(clockChangeNote(t))
    }

    @Test
    fun a_time_the_clocks_skip_or_repeat_says_which_time_is_saved() {
        // Prague: the clocks go forward at 02:00 on 29 March 2026 and back at 03:00 on 25 October 2026.
        val prague = ZoneId.of("Europe/Prague")
        val skipped = resolveReminder(LocalDate.of(2026, 3, 29), 2, 30, prague)
        assertTrue(skipped.skipped)
        assertEquals(LocalDateTime.of(2026, 3, 29, 3, 30), skipped.saved)
        assertEquals(Instant.parse("2026-03-29T01:30:00Z").toEpochMilli(), skipped.atMs)
        assertEquals("02:30 doesn’t exist that day: the clocks go forward. Saves 03:30.", clockChangeNote(skipped))
        val twice = resolveReminder(LocalDate.of(2026, 10, 25), 2, 30, prague)
        assertTrue(twice.twice)
        // The first 02:30 is still summer time (UTC+2).
        assertEquals(Instant.parse("2026-10-25T00:30:00Z").toEpochMilli(), twice.atMs)
        assertEquals("02:30 happens twice that day: the clocks go back. Saves the first.", clockChangeNote(twice))
        // Across a change of the clocks, the label reads as set, in the zone of the reminder's own day.
        val winter = Instant.parse("2026-03-20T10:00:00Z").toEpochMilli()
        val summerNine = LocalDateTime.of(2026, 4, 2, 9, 0).atZone(prague).toInstant().toEpochMilli()
        assertEquals("Thu 2 Apr, 09:00", reminderLabel(summerNine, winter, prague))
        assertEquals("Sun 29 Mar, 03:30", reminderLabel(skipped.atMs, winter, prague))
    }
}
