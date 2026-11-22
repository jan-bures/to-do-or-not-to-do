package app.todoornot

import app.todoornot.core.LocalNow
import app.todoornot.core.ProblemKind
import app.todoornot.core.RoomKey
import app.todoornot.core.SmartList
import app.todoornot.core.SyncState
import app.todoornot.core.SyncStatus
import app.todoornot.design.Tokens
import app.todoornot.model.Tone
import app.todoornot.model.TodoModel
import app.todoornot.model.clock
import app.todoornot.model.syncLine
import app.todoornot.ui.StepDraft
import app.todoornot.ui.buttonColors
import app.todoornot.ui.showsAgent
import app.todoornot.ui.showsPriority
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.luminance
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.test.StandardTestDispatcher
import kotlinx.coroutines.test.TestScope
import kotlinx.coroutines.test.UnconfinedTestDispatcher
import kotlinx.coroutines.test.advanceUntilIdle
import kotlinx.coroutines.test.runTest
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/** What the UX review asked for, matching the desktop: the star, steps, new lists, the filter, the drawer. */
@OptIn(ExperimentalCoroutinesApi::class)
class ParityTest {
    private fun TestScope.model(engine: app.todoornot.core.TodoEngine = fixtureEngine(), start: RoomKey = RoomKey.Smart(SmartList.MY_DAY)) =
        TodoModel(engine, CoroutineScope(SupervisorJob() + UnconfinedTestDispatcher(testScheduler)), UnconfinedTestDispatcher(testScheduler), now = { FIXTURE_NOW }, start = start)

    @Test
    fun the_star_marks_a_task_important_and_back() = runTest {
        val engine = fixtureEngine()
        val admin = engine.list("Admin")
        val m = model(engine, admin)
        val id = engine.taskId(admin, "Renew passport appointment")
        m.select(id)
        m.setImportant(id, true)
        advanceUntilIdle()
        assertEquals(3.toUByte(), m.ui.value.detail?.priority)
        assertTrue(engine.room(RoomKey.Smart(SmartList.IMPORTANT), FIXTURE_NOW).tasks.any { it.id == id })
        m.setImportant(id, false)
        advanceUntilIdle()
        assertEquals(0.toUByte(), m.ui.value.detail?.priority)
    }

    @Test
    fun steps_are_added_at_the_end_and_ticked() = runTest {
        val engine = fixtureEngine()
        val admin = engine.list("Admin")
        val m = model(engine, admin)
        val id = engine.taskId(admin, "Renew passport appointment")
        m.select(id)
        val before = m.ui.value.detail!!.steps.size
        var result: String? = "not called"
        m.addStep(id, "Print the form") { result = it }
        advanceUntilIdle()
        assertNull(result)
        val steps = m.ui.value.detail!!.steps
        assertEquals(before + 1, steps.size)
        assertEquals("Print the form", steps.last().title)
        assertFalse(steps.last().done)
        m.setStepDone(steps.last().id, true)
        advanceUntilIdle()
        assertTrue(m.ui.value.detail!!.steps.last().done)
    }

    @Test
    fun a_new_list_is_added_and_opened() = runTest {
        val m = model()
        m.createList("Garden")
        advanceUntilIdle()
        val room = m.ui.value.room
        assertEquals("Garden", room.title)
        assertTrue(room.key is RoomKey.List)
        assertEquals("Nothing here yet.", room.emptyLine)
        assertTrue(m.ui.value.home.groups.flatMap { it.lists }.any { it.name == "Garden" })
    }

    @Test
    fun the_filter_narrows_the_rows_and_switching_rooms_clears_it() = runTest {
        val m = model()
        val all = m.ui.value.room
        m.setFilter("rohlíky")
        advanceUntilIdle()
        val found = m.ui.value.room
        assertTrue(found.filtered)
        assertEquals(listOf("Koupit rohlíky a mléko"), found.tasks.map { it.title })
        assertEquals("the progress is the room's", all.total, found.total)

        m.setFilter("priority>=high")
        advanceUntilIdle()
        assertTrue(m.ui.value.room.tasks.isNotEmpty())
        assertTrue(m.ui.value.room.tasks.all { it.priority >= 3u })

        m.open(RoomKey.Smart(SmartList.ALL))
        advanceUntilIdle()
        assertEquals("", m.ui.value.filter)
        assertFalse(m.ui.value.room.filtered)
    }

    @Test
    fun a_refused_step_reports_why_and_adds_nothing() = runTest {
        val engine = fixtureEngine()
        val admin = engine.list("Admin")
        val m = model(engine, admin)
        val id = engine.taskId(admin, "Renew passport appointment")
        m.select(id)
        val before = m.ui.value.detail!!.steps.size
        var result: String? = null
        m.addStep(id, "x".repeat(501)) { result = it }
        advanceUntilIdle()
        assertTrue("the reason is given to the editor", !result.isNullOrBlank())
        assertEquals(before, m.ui.value.detail!!.steps.size)
        assertNull("no toast: the detail pane shows the reason", m.ui.value.toast)
    }

    @Test
    fun opening_the_same_room_again_clears_the_filter_and_its_field() = runTest {
        val m = model()
        m.setSearching(true)
        m.setFilter("rohlíky")
        advanceUntilIdle()
        assertTrue(m.ui.value.searching)
        m.open(RoomKey.Smart(SmartList.MY_DAY))
        advanceUntilIdle()
        assertEquals("", m.ui.value.filter)
        assertFalse(m.ui.value.searching)
        assertFalse(m.ui.value.room.filtered)
    }

    @Test
    fun closing_the_filter_field_clears_the_filter() = runTest {
        val m = model()
        m.setSearching(true)
        advanceUntilIdle()
        assertTrue("an open, empty field", m.ui.value.searching)
        assertEquals("", m.ui.value.filter)
        m.setFilter("rohlíky")
        advanceUntilIdle()
        m.setSearching(false)
        advanceUntilIdle()
        assertFalse(m.ui.value.searching)
        assertEquals("", m.ui.value.filter)
        assertFalse(m.ui.value.room.filtered)
    }

    @Test
    fun a_slow_read_for_an_older_filter_never_replaces_the_newer_rows() = runTest {
        // The read for "a" is overtaken: while it runs, the owner types on to "rohlíky", whose read
        // finishes first. The slow one must not publish its rows, or its filter, over the newer ones.
        lateinit var m: TodoModel
        var overtake = false
        m = TodoModel(
            fixtureEngine(),
            CoroutineScope(SupervisorJob() + UnconfinedTestDispatcher(testScheduler)),
            UnconfinedTestDispatcher(testScheduler),
            now = {
                if (overtake) {
                    overtake = false
                    m.setFilter("rohlíky")
                }
                FIXTURE_NOW
            },
        )
        overtake = true
        m.setFilter("a")
        advanceUntilIdle()
        assertEquals("rohlíky", m.ui.value.filter)
        assertEquals(listOf("Koupit rohlíky a mléko"), m.ui.value.room.tasks.map { it.title })
    }

    @Test
    fun a_read_from_before_the_field_closed_never_reopens_it() = runTest {
        // The model's own refreshes are queued (not run at once), so a read can be taken with the field
        // open and finish after the owner has closed it, before the closing refresh has run.
        lateinit var m: TodoModel
        var close = false
        m = TodoModel(
            fixtureEngine(),
            CoroutineScope(SupervisorJob() + StandardTestDispatcher(testScheduler)),
            UnconfinedTestDispatcher(testScheduler),
            now = {
                if (close) {
                    close = false
                    m.setSearching(false)
                }
                FIXTURE_NOW
            },
        )
        m.setSearching(true)
        advanceUntilIdle()
        assertTrue("an open, empty field", m.ui.value.searching)
        close = true
        m.refresh()
        assertFalse("the old read is dropped", m.ui.value.searching)
        advanceUntilIdle()
        assertFalse(m.ui.value.searching)
        assertEquals("", m.ui.value.filter)
    }

    @Test
    fun done_pressed_twice_sends_one_step_and_a_refused_one_can_be_sent_again() {
        val draft = StepDraft()
        val sent = mutableListOf<Pair<String, (String?) -> Unit>>()
        val send: (String, (String?) -> Unit) -> Unit = { title, done -> sent += title to done }
        draft.edit("Print the form ")
        draft.submit(send)
        draft.submit(send)
        assertEquals("the second Done is ignored while the first is on its way", 1, sent.size)
        assertTrue(draft.sending)

        sent.last().second("A step title can be at most 500 characters.")
        assertFalse(draft.sending)
        assertEquals("refused: the draft stays", "Print the form ", draft.text)
        assertEquals("A step title can be at most 500 characters.", draft.problem)

        draft.submit(send)
        assertEquals(2, sent.size)
        assertEquals("Print the form", sent.last().first)
        sent.last().second(null)
        assertEquals("added: the field is cleared for the next one", "", draft.text)
        assertNull(draft.problem)
        assertFalse(draft.sending)
    }

    @Test
    fun a_filter_that_matches_only_done_tasks_lists_them_without_the_empty_line() = runTest {
        val m = model(fixtureEngine("finished"))
        val done = m.ui.value.room.completed.first().title
        m.setFilter(done)
        advanceUntilIdle()
        val room = m.ui.value.room
        assertTrue(room.tasks.isEmpty())
        assertTrue(room.completed.any { it.title == done })
        assertFalse("nothing-matches is for when nothing is listed", app.todoornot.ui.showsEmptyLine(room))
    }

    @Test
    fun a_finished_day_offers_planning_even_without_a_hand_over() = runTest {
        val engine = fixtureEngine("finished")
        val room = model(engine).ui.value.room
        assertTrue(room.finished)
        assertTrue(room.planWeekday != null)
        assertFalse(app.todoornot.ui.showsEmptyLine(room))
    }

    @Test
    fun collections_fold_in_the_drawer() = runTest {
        val engine = fixtureEngine()
        val id = (engine.collection("Personal") as RoomKey.Collection).id
        val m = model(engine)
        m.toggleCollection(id)
        advanceUntilIdle()
        assertEquals(setOf(id), m.ui.value.collapsed)
        m.toggleCollection(id)
        advanceUntilIdle()
        assertEquals(emptySet<String>(), m.ui.value.collapsed)
    }

    @Test
    fun the_finished_day_keeps_its_completed_tasks_and_hands_over() = runTest {
        val room = model(fixtureEngine("finished")).ui.value.room
        assertTrue(room.finished)
        assertTrue(room.tomorrow != null)
        assertTrue(room.completed.isNotEmpty())
    }

    @Test
    fun the_sync_line_reads_as_on_the_desktop() {
        fun s(state: SyncState, pending: UInt = 0u, failed: UInt = 0u, problem: ProblemKind? = null) =
            SyncStatus(state, pending, failed, problem, null)
        assertEquals("Synced 10:35" to Tone.OK, syncLine(s(SyncState.ONLINE), FIXTURE_NOW))
        assertEquals("Synced" to Tone.OK, syncLine(s(SyncState.ONLINE), null))
        assertEquals("Syncing…" to Tone.BUSY, syncLine(s(SyncState.SYNCING, 2u), null))
        assertEquals("Offline, 3 waiting" to Tone.PROBLEM, syncLine(s(SyncState.OFFLINE, 3u), null))
        assertEquals("Offline" to Tone.PROBLEM, syncLine(s(SyncState.OFFLINE), null))
        assertEquals("2 couldn’t sync" to Tone.PROBLEM, syncLine(s(SyncState.ONLINE, failed = 2u), null))
        assertEquals(
            "Signed out by the server" to Tone.PROBLEM,
            syncLine(s(SyncState.OFFLINE, problem = ProblemKind.UNAUTHORIZED), null),
        )
        assertEquals("Not syncing, 1 waiting" to Tone.IDLE, syncLine(s(SyncState.PAUSED, 1u), null))
        assertEquals("09:05", clock(LocalNow(8 * 3_600_000L + 5 * 60_000L, 3600)))
        assertEquals("23:59", clock(LocalNow(-60_000L, 0)))
    }

    @Test
    fun empty_trailing_columns_are_left_out_as_on_the_desktop() {
        val rows = fixtureEngine().room(RoomKey.Smart(SmartList.MY_DAY), FIXTURE_NOW).tasks
        val plain = rows.first { it.agent == null && it.priority == 0.toUByte() }
        assertFalse(showsAgent(plain))
        assertFalse(showsPriority(plain))
        val agent = rows.first { it.agent != null }
        assertTrue(showsAgent(agent))
        assertTrue("an agent's mark keeps its place from the right edge", showsPriority(agent))
        rows.filter { it.agent == null && it.priority > 0u }.forEach {
            assertFalse(showsAgent(it))
            assertTrue(showsPriority(it))
        }
    }

    /** WCAG 2.x contrast from the emitted colours. */
    private fun contrast(a: Color, b: Color): Double {
        val (hi, lo) = listOf(a.luminance(), b.luminance()).sortedDescending()
        return (hi + 0.05) / (lo + 0.05)
    }

    @Test
    fun a_disabled_button_keeps_its_text_readable_in_every_room() {
        for ((theme, rooms) in listOf("light" to Tokens.lightRooms, "dark" to Tokens.darkRooms)) {
            for ((hue, r) in rooms) {
                for (primary in listOf(true, false)) {
                    val c = buttonColors(r, primary, danger = false, enabled = false)
                    assertTrue("$theme $hue: ${contrast(c.ink, c.bg)}", contrast(c.ink, c.bg) >= 4.5)
                    assertTrue("$theme $hue has an edge", c.edge != null)
                }
            }
        }
        assertNull(buttonColors(Tokens.lightRooms.getValue("my-day"), primary = true, danger = false, enabled = true).edge)
    }
}
