package app.todoornot

import app.todoornot.core.LocalNow
import app.todoornot.core.ProblemKind
import app.todoornot.core.RoomKey
import app.todoornot.core.SmartList
import app.todoornot.core.SyncState
import app.todoornot.core.SyncStatus
import app.todoornot.design.Tokens
import app.todoornot.model.StepDraft
import app.todoornot.model.Tone
import app.todoornot.model.TodoModel
import app.todoornot.model.clock
import app.todoornot.model.syncLine
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
        assertTrue(m.ui.value.home.groups.single { it.collection == null }.lists.any { it.name == "Garden" })
    }

    @Test
    fun new_lists_use_the_current_collection_and_quick_add_goes_first() = runTest {
        val engine = fixtureEngine()
        val work = engine.collection("Work")
        val m = model(engine, work)
        m.createList("Project")
        advanceUntilIdle()
        assertEquals("Project", m.ui.value.room.title)
        // From a list in the collection, New list creates a sibling.
        m.createList("Invoices")
        advanceUntilIdle()
        val group = m.ui.value.home.groups.single { it.collection?.key == work }
        assertEquals(listOf("Project", "Invoices"), group.lists.takeLast(2).map { it.name })
        val invoices = m.ui.value.room.key
        assertEquals(group.lists.last().key, invoices)
        m.add("File a receipt")
        m.add("Pay the invoice by fri !!")
        advanceUntilIdle()
        val tasks = m.ui.value.room.tasks
        assertEquals(listOf("Pay the invoice", "File a receipt"), tasks.map { it.title })
        assertEquals(2.toUByte(), tasks.first().priority)
        assertTrue(engine.room(RoomKey.Smart(SmartList.PLANNED), FIXTURE_NOW).tasks.any { it.id == tasks.first().id })
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
        val draft = StepDraft("task")
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
    fun a_step_sent_just_before_a_recreation_is_added_once_and_clears_the_new_field() = runTest {
        // The model outlives the Activity; its own work is queued, so the write is still on its way
        // while the "recreated" detail pane asks the model for the draft again.
        val engine = fixtureEngine()
        val admin = engine.list("Admin")
        val id = engine.taskId(admin, "Renew passport appointment")
        val m = TodoModel(
            engine,
            CoroutineScope(SupervisorJob() + StandardTestDispatcher(testScheduler)),
            UnconfinedTestDispatcher(testScheduler),
            now = { FIXTURE_NOW },
            start = admin,
        )
        m.select(id)
        advanceUntilIdle()
        val before = m.ui.value.detail!!.steps.size
        var sent = 0
        val send: (String, (String?) -> Unit) -> Unit = { t, done -> sent++; m.addStep(id, t, done) }

        val first = m.stepDraft(id)
        first.edit("Print the form")
        first.submit(send)

        // Recreated: the new pane gets the same draft, still sending.
        val second = m.stepDraft(id)
        assertTrue(second === first)
        assertTrue(second.sending)
        assertEquals("Print the form", second.text)
        second.submit(send)
        assertEquals("Done again while it's on its way does nothing", 1, sent)

        advanceUntilIdle()
        assertEquals("", second.text)
        assertFalse(second.sending)
        assertEquals(before + 1, m.ui.value.detail!!.steps.size)

        assertEquals("", m.stepDraft(id).text)
    }

    @Test
    fun a_rotation_after_the_step_is_added_but_before_the_old_pane_leaves_adds_it_once() = runTest {
        val engine = fixtureEngine()
        val admin = engine.list("Admin")
        val id = engine.taskId(admin, "Renew passport appointment")
        val m = TodoModel(
            engine,
            CoroutineScope(SupervisorJob() + StandardTestDispatcher(testScheduler)),
            UnconfinedTestDispatcher(testScheduler),
            now = { FIXTURE_NOW },
            start = admin,
        )
        m.select(id)
        advanceUntilIdle()
        val before = m.ui.value.detail!!.steps.size
        var sent = 0
        val send: (String, (String?) -> Unit) -> Unit = { t, done -> sent++; m.addStep(id, t, done) }

        val old = m.stepDraft(id) // the pane before the rotation
        old.edit("Print the form")
        old.submit(send)
        val new = m.stepDraft(id) // the new pane takes the draft first
        advanceUntilIdle() // the step is added before the old pane leaves

        assertTrue("the new pane's draft is still the model's", m.stepDraft(id) === new)
        assertEquals("", new.text)
        new.submit(send)
        advanceUntilIdle()
        assertEquals(1, sent)
        assertEquals(before + 1, m.ui.value.detail!!.steps.size)
    }

    @Test
    fun switching_tasks_never_carries_a_draft_over() = runTest {
        val engine = fixtureEngine()
        val a = engine.taskId(engine.list("Admin"), "Renew passport appointment")
        val b = engine.taskId(engine.list("Finance"), "Pay quarterly VAT filing")
        val m = model(engine)
        m.select(a)
        m.stepDraft(a).edit("Print the form")
        m.select(b)
        assertEquals("", m.stepDraft(b).text)
        m.select(a)
        assertEquals("Print the form", m.stepDraft(a).text)
    }

    @Test
    fun a_draft_survives_the_pane_switching_from_compact_to_wide() = runTest {
        // Unfolding swaps the cover screen's detail pane for the inner screen's: the old pane leaves
        // while the new one already shows the draft. Both ask the model, and get the same draft.
        val engine = fixtureEngine()
        val a = engine.taskId(engine.list("Admin"), "Renew passport appointment")
        val m = model(engine)
        m.select(a)
        advanceUntilIdle()
        val compact = m.stepDraft(a)
        val wide = m.stepDraft(a)
        assertTrue(compact === wide)
        wide.edit("Print the form") // typed in the wide pane, after the compact one left
        m.select(null)
        advanceUntilIdle()
        m.select(a)
        advanceUntilIdle()
        assertEquals("Print the form", m.stepDraft(a).text)
    }

    @Test
    fun a_done_tasks_draft_is_dropped_but_not_while_its_step_is_on_its_way() = runTest {
        val engine = fixtureEngine()
        val admin = engine.list("Admin")
        val a = engine.taskId(admin, "Renew passport appointment")
        val m = model(engine, admin)
        val draft = m.stepDraft(a).apply { edit("Print the form") }
        var answer: ((String?) -> Unit)? = null
        draft.submit { _, done -> answer = done }

        m.setDone(a, true)
        advanceUntilIdle()
        assertTrue("on its way: kept", m.stepDraft(a) === draft)

        answer!!(null)
        m.refresh()
        assertTrue("answered: dropped", m.stepDraft(a) !== draft)

        val idle = m.stepDraft(a).apply { edit("Call the office") }
        m.refresh()
        assertTrue("a done task's idle draft is dropped", m.stepDraft(a) !== idle)
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
