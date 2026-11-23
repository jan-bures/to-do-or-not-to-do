package app.todoornot

import app.todoornot.core.ActivityLoad
import app.todoornot.core.ActivityFooter
import app.todoornot.core.CustomRepeat
import app.todoornot.core.RepeatUnit

import app.todoornot.core.Completion
import app.todoornot.core.quickAddDate
import app.todoornot.core.FailedChange
import app.todoornot.core.FieldEdit
import app.todoornot.core.Home
import app.todoornot.core.LocalNow
import app.todoornot.core.NoHandle
import app.todoornot.core.RoomKey
import app.todoornot.core.RoomView
import app.todoornot.core.RowPlacement
import app.todoornot.core.TaskDetail
import app.todoornot.core.SmartList
import app.todoornot.core.SyncState
import app.todoornot.core.SyncStatus
import app.todoornot.core.TodoEngine
import app.todoornot.design.Tokens
import app.todoornot.model.DuskLook
import app.todoornot.model.Ghost
import app.todoornot.model.Documents
import app.todoornot.model.FileAction
import app.todoornot.model.Pace
import app.todoornot.model.RowLook
import app.todoornot.model.MenuAction
import app.todoornot.model.MenuEntry
import app.todoornot.model.taskMenu
import app.todoornot.model.TodoModel
import app.todoornot.model.Wake
import app.todoornot.model.ease
import app.todoornot.model.rowIn
import app.todoornot.model.Section
import app.todoornot.model.UnpairQuestion
import app.todoornot.model.unsentCount
import app.todoornot.ui.showsEmptyLine
import app.todoornot.ui.unpairWording
import app.todoornot.model.withGhosts
import kotlinx.coroutines.CoroutineDispatcher
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Runnable
import kotlinx.coroutines.launch
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.cancel
import kotlinx.coroutines.test.StandardTestDispatcher
import kotlinx.coroutines.test.TestScope
import kotlinx.coroutines.test.UnconfinedTestDispatcher
import kotlinx.coroutines.test.advanceUntilIdle
import kotlinx.coroutines.test.runTest
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotEquals
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import kotlin.coroutines.CoroutineContext

private val M = Tokens.Motion
private val MY_DAY: RoomKey = RoomKey.Smart(SmartList.MY_DAY)
private val ALL: RoomKey = RoomKey.Smart(SmartList.ALL)
private val AGENTS: RoomKey = RoomKey.Smart(SmartList.AGENT_ACTIVITY)

/** Where a reversal at [at], part way through a completion's hold, has settled. */
private fun settledFrom(at: Long) = at + maxOf(M.OVERLAY_OUT, M.CHECK_TICK)

/** The open rows as the list draws them: the room's own, with its ghosts back in their places. */
private fun TodoModel.drawn(): List<String> =
    withGhosts(ui.value.room.tasks, motion.ghosts(), Section.ROWS).map { it.row.id }

/** The filter [WithAgent] answers itself: the tasks the owner changed last. */
private const val BY_OWNER = "updated_by:owner"

/**
 * The fixture engine with an agent at work: what [agentSetDone] and [agentTouch] change is an agent's
 * latest change, so Agent activity lists it first, and [BY_OWNER] no longer lists it, as the server's own
 * engine would once an agent's command lands (the fixture engine writes only as its owner). A list [gone]
 * (a sync deleted it) falls back to My Day, as the engine resolves it. Everything else is the fixture
 * engine's.
 */
private class WithAgent(private val real: TodoEngine) : TodoEngine(NoHandle) {
    /** The tasks an agent touched last, the latest first, until the owner changes them. */
    private val touched = ArrayList<String>()

    /** Simulates a remote edit that excludes a row for a reason other than the owner's completion. */
    val omitted = HashSet<String>()

    /** A room a sync deleted: the engine shows My Day for it. */
    var gone: RoomKey? = null

    fun agentSetDone(id: String, done: Boolean) {
        real.setDone(id, done)
        agentTouch(id)
    }

    /** An agent changes the task (renames it, say) without completing or reopening it. */
    fun agentTouch(id: String) {
        touched.remove(id)
        touched.add(0, id)
        omitted.remove(id)
    }

    override fun setDone(id: String, done: Boolean): Completion? {
        touched.remove(id)
        return real.setDone(id, done)
    }

    override fun undoCompletion(completion: Completion) = real.undoCompletion(completion)

    override fun roomPlaced(key: RoomKey, filter: String, placement: RowPlacement, now: LocalNow): RoomView {
        val view = real.roomPlaced(if (key == gone) MY_DAY else key, filter, placement, now)
        if (filter == BY_OWNER) {
            return view.copy(tasks = view.tasks.filter { it.id !in touched }, completed = view.completed.filter { it.id !in touched })
        }
        if (key != AGENTS) return view
        val remaining = view.tasks.filter { it.id !in omitted && it.id !in touched }
        if (touched.isEmpty()) return view.copy(tasks = remaining)
        val all = real.roomPlaced(ALL, "", placement, now)
        val rows = touched.mapNotNull { id -> (all.tasks + all.completed).firstOrNull { it.id == id } }
        return view.copy(tasks = rows + remaining)
    }

    override fun homePlaced(placement: RowPlacement, now: LocalNow): Home = real.homePlaced(placement, now)
    override fun task(id: String, now: LocalNow): TaskDetail? = real.task(id, now)
    override fun taskDone(id: String): Boolean? = real.taskDone(id)
    override fun status(): SyncStatus = real.status()
    override fun failed(): List<FailedChange> = real.failed()
}

/** Where a full-pace completion starts to leave, and where it's released. */
private val EXIT_AT = (M.CHECK_TICK_DELAY + M.CHECK_TICK + M.COMPLETE_HOLD).toLong()
private val RELEASE_AT = EXIT_AT + M.ROW_COLLAPSE

/** The main screen's model over the real Rust engine (host build, through JNA) with fixture data. */
@OptIn(ExperimentalCoroutinesApi::class)
class TodoModelTest {
    /** Runs every model coroutine eagerly, so each call has taken effect when it returns. */
    private fun TestScope.eager() = CoroutineScope(SupervisorJob() + UnconfinedTestDispatcher(testScheduler))

    /** The motion's clock: a test moves it and draws frames by hand ([frame]). */
    private var time = 0L

    /**
     * The model, run eagerly. It hears of engine changes only from its own writes: the engine's listener
     * calls back on the engine's threads, racing the test, so only [listening] models use it.
     */
    private fun TestScope.model(name: String = "world", engine: TodoEngine = fixtureEngine(name), pace: Pace = Pace.FULL) =
        TodoModel(engine, eager(), UnconfinedTestDispatcher(testScheduler), now = { FIXTURE_NOW }, clock = { time }, pace = { pace }, listen = {})

    /** The model with the engine's own listener. */
    private fun TestScope.listening(engine: TodoEngine) =
        TodoModel(engine, eager(), UnconfinedTestDispatcher(testScheduler), now = { FIXTURE_NOW }, clock = { time })

    /** Moves the clock to [t] and draws a frame there. */
    private fun TodoModel.frame(t: Long) {
        time = t
        motion.frame(t)
    }

    @Test
    fun my_day_opens_first_with_the_fixture_numbers() = runTest {
        val ui = model().ui.value
        assertEquals(RoomKey.Smart(SmartList.MY_DAY), ui.room.key)
        assertEquals("My Day", ui.room.title)
        assertEquals(3u, ui.room.done)
        assertEquals(12u, ui.room.total)
        // The fixture engine never syncs.
        assertEquals(SyncState.PAUSED, ui.status.state)
    }

    @Test
    fun the_date_chip_uses_submit_syntax() {
        assertEquals("Sun 22 Nov", quickAddDate("Buy seeds zítra !!", FIXTURE_NOW))
        assertNull(quickAddDate("Buy seeds \\tomorrow", FIXTURE_NOW))
        assertNull(quickAddDate("Buy seeds tomorrow at noon", FIXTURE_NOW))
    }

    @Test
    fun notes_save_clear_cancel_and_keep_refused_drafts() {
        val s = Stepped(fixtureEngine()) { time }
        val m = s.model
        s.drain()
        val id = m.ui.value.room.tasks.first().id
        m.select(id)
        s.drain()
        val draft = m.notesDraft(id)
        for (notes in listOf("Two packets\nof seeds.", "")) {
            draft.edit(notes)
            draft.save(m)
            draft.save(m)
            assertTrue(m.notesDraft(id).saving)
            s.drain {
                if (draft.text == null) assertEquals("no flash of the old notes", notes, m.ui.value.detail?.notes)
            }
            assertNull(draft.text)
            assertNull(draft.problem)
            assertEquals(notes, m.ui.value.detail?.notes)
        }
        val long = "a".repeat(100_001)
        draft.edit(long)
        draft.save(m)
        s.drain()
        assertNotNull(draft.problem)
        assertEquals(long, draft.text)
        draft.edit(null) // Cancel.
        assertNull(draft.problem)
        assertNull(draft.text)
        assertEquals("", m.ui.value.detail?.notes)
    }

    @Test
    fun leaving_detail_saves_notes_for_the_task_that_was_open() {
        val s = Stepped(fixtureEngine()) { time }
        val m = s.model
        s.drain()
        val (id, next) = m.ui.value.room.tasks.take(2).map { it.id }
        for ((index, leave) in listOf<() -> Unit>({ m.select(null) }, { m.select(next) }, { m.open(ALL) }).withIndex()) {
            m.select(id)
            s.drain()
            val text = "Draft $index\nSecond line"
            m.notesDraft(id).edit(text)
            leave()
            s.drain()
            assertEquals(text, m.engine.task(id, FIXTURE_NOW)?.notes)
        }
    }

    @Test
    fun leaving_an_untouched_editor_does_not_write_over_synced_notes() {
        val s = Stepped(fixtureEngine()) { time }
        val m = s.model
        s.drain()
        val id = m.ui.value.room.tasks.first().id
        m.select(id)
        s.drain()
        m.notesDraft(id).begin(m.ui.value.detail!!.notes)
        m.engine.setNotes(id, "Changed elsewhere")
        m.select(null)
        s.drain()
        assertEquals("Changed elsewhere", m.engine.task(id, FIXTURE_NOW)?.notes)
    }

    @Test
    fun a_refused_navigation_save_keeps_the_draft_and_notifies_the_owner() {
        val s = Stepped(fixtureEngine()) { time }
        val m = s.model
        s.drain()
        val id = m.ui.value.room.tasks.first().id
        m.select(id)
        s.drain()
        val text = "n".repeat(100_001)
        m.notesDraft(id).edit(text)
        m.select(null)
        s.drain()
        assertTrue(m.ui.value.toast!!.text.startsWith("Couldn't save notes"))
        m.select(id)
        s.drain()
        assertEquals(text, m.notesDraft(id).text)
        assertNotNull(m.notesDraft(id).problem)
    }

    @Test
    fun cancelling_an_unsent_completion_restores_the_agent_and_consumes_only_the_current_toast() {
        val s = Stepped(fixtureEngine()) { time }
        val m = s.model
        m.open(AGENTS)
        s.drain()
        val row = m.ui.value.room.tasks.first { !it.done }
        m.setDone(row.id, true)
        s.drain()
        val toast = m.ui.value.toast!!
        assertFalse(m.motion.motion.isGhost(row.id))
        assertTrue(m.ui.value.room.tasks.any { it.id == row.id })
        m.undo(toast)
        m.undo(toast)
        s.drain()
        assertEquals(0u, m.ui.value.status.pending)
        assertTrue(m.ui.value.room.tasks.any { it.id == row.id && it.agent == row.agent && !it.done })
        assertFalse(m.motion.motion.isGhost(row.id))
    }

    @Test
    fun adding_in_my_day_lands_in_my_day() = runTest {
        val m = model()
        m.add("Post the parcel")
        advanceUntilIdle()
        val room = m.ui.value.room
        assertTrue(room.tasks.any { it.title == "Post the parcel" && it.myDay })
        assertEquals(13u, room.total)
        assertEquals(1u, m.ui.value.status.pending)
    }

    @Test
    fun completing_holds_the_row_until_its_choreography_releases_it_and_undo_brings_it_back() {
        val s = Stepped(fixtureEngine()) { time }
        val m = s.model
        s.drain()
        val task = m.ui.value.room.tasks.first { it.title == "Koupit rohlíky a mléko" }
        m.setDone(task.id, true)
        s.drain()
        // Written at once (the toast offers undo), but the row holds its place and the counts wait.
        val toast = m.ui.value.toast!!
        assertEquals(task.id, toast.completion?.task)
        assertEquals(true, m.engine.taskDone(task.id))
        assertTrue(m.ui.value.room.tasks.any { it.id == task.id })
        assertEquals(3u, m.ui.value.room.done)
        assertEquals("the drawer counts it as open too", 9u, m.ui.value.home.smart.first().open)

        // Holding, then collapsing: still there.
        m.frame(EXIT_AT + M.ROW_COLLAPSE / 2)
        s.drain()
        assertTrue(m.ui.value.room.tasks.any { it.id == task.id })
        // Released: it leaves the open rows, the count changes, and the sun sets off from where it was.
        m.frame(RELEASE_AT)
        s.drain()
        assertFalse(m.ui.value.room.tasks.any { it.id == task.id })
        assertEquals(4u, m.ui.value.room.done)
        assertEquals(8u, m.ui.value.home.smart.first().open)
        assertEquals(3f / 12f, m.motion.sun(MY_DAY)!!, 1e-6f)
        m.frame(RELEASE_AT + M.SUN_TRAVEL)
        assertEquals(4f / 12f, m.motion.sun(MY_DAY)!!, 1e-6f)
        assertEquals(Wake.Idle, m.motion.motion.wake(time))

        // Undo, with Completed closed: it goes straight back to the open rows, entering.
        m.undo(toast)
        s.drain()
        assertNull(m.ui.value.toast)
        assertTrue(m.ui.value.room.tasks.any { it.id == task.id })
        assertEquals(3u, m.ui.value.room.done)
        assertNotNull(m.motion.row(task.id))
        m.frame(time + 5_000)
        assertNull(m.motion.row(task.id))
        assertEquals(Wake.Idle, m.motion.motion.wake(time))
    }

    @Test
    fun a_second_tap_part_way_reverses_in_place_and_nothing_is_released() = runTest {
        val m = model()
        val id = m.ui.value.room.tasks.first().id
        m.setDone(id, true)
        advanceUntilIdle()
        m.frame(EXIT_AT + 50)
        m.setDone(id, false)
        advanceUntilIdle()
        assertEquals(false, m.engine.taskDone(id))
        assertTrue(m.motion.motion.holding().isEmpty())
        m.frame(time + 5_000)
        advanceUntilIdle()
        assertEquals(id, m.ui.value.room.tasks.first().id)
        assertEquals(3u, m.ui.value.room.done)
        assertNull(m.motion.row(id))
    }

    @Test
    fun reopening_from_an_open_completed_section_keeps_its_place_until_it_leaves() = runTest {
        val m = model()
        m.setShowCompleted(true)
        advanceUntilIdle()
        val before = m.ui.value.room.completed
        val done = before.first()
        m.setDone(done.id, false)
        advanceUntilIdle()
        // Still in its completed place, and still counted, while the disc clears and the row leaves.
        assertEquals(before.map { it.id }, m.ui.value.room.completed.map { it.id })
        assertEquals(3u, m.ui.value.room.done)
        assertFalse(m.ui.value.room.tasks.any { it.id == done.id })
        m.frame((M.OVERLAY_OUT + M.ROW_COLLAPSE).toLong())
        advanceUntilIdle()
        assertTrue(m.ui.value.room.tasks.any { it.id == done.id })
        assertEquals(2u, m.ui.value.room.done)
        // It enters the open rows.
        assertEquals(0f, m.motion.rowIn(done.id, Section.ROWS)!!.height)
    }

    @Test
    fun a_change_from_elsewhere_settles_the_row_and_the_sun_eases_to_any_new_count() = runTest {
        val engine = fixtureEngine()
        val m = model(engine = engine)
        val (a, b) = m.ui.value.room.tasks.take(2).map { it.id }
        m.setDone(a, true)
        advanceUntilIdle()
        m.frame(300)
        // Another device reopens it (or the server refused the completion): the row settles open.
        engine.setDone(a, false)
        m.refresh()
        assertTrue(m.motion.motion.holding().isEmpty())
        m.frame(5_000)
        advanceUntilIdle()
        assertEquals(a, m.ui.value.room.tasks.first().id)
        assertEquals(3u, m.ui.value.room.done)
        // Another device completes one: no choreography, but the sun still eases to the new count.
        engine.setDone(b, true)
        m.refresh()
        assertEquals(4u, m.ui.value.room.done)
        assertEquals(3f / 12f, m.motion.sun(MY_DAY)!!, 1e-6f)
        assertNull(m.motion.row(b))
        m.frame(time + M.SUN_TRAVEL / 2)
        assertEquals(3.5f / 12f, m.motion.sun(MY_DAY)!!, 1e-3f)
    }

    @Test
    fun switching_rooms_lets_a_held_task_go_at_once() = runTest {
        val m = model()
        val id = m.ui.value.room.tasks.first().id
        m.setDone(id, true)
        advanceUntilIdle()
        m.open(RoomKey.Smart(SmartList.ALL))
        advanceUntilIdle()
        assertTrue(m.motion.motion.holding().isEmpty())
        assertEquals(Wake.Idle, m.motion.motion.wake(time))
        assertFalse(m.ui.value.room.tasks.any { it.id == id })
    }

    @Test
    fun the_last_task_of_my_day_brings_dusk_once_it_has_left() = runTest {
        val engine = fixtureEngine("finished")
        val last = engine.room(MY_DAY, FIXTURE_NOW).completed.first().id
        engine.setDone(last, false)
        val m = model(engine = engine)
        assertFalse(m.ui.value.room.finished)
        m.setDone(last, true)
        advanceUntilIdle()
        // Holding: the day isn't finished yet, and the header is still the strip.
        assertFalse(m.ui.value.room.finished)
        assertEquals(DuskLook.rest(false), m.motion.dusk(MY_DAY))
        m.frame(RELEASE_AT)
        advanceUntilIdle()
        assertTrue(m.ui.value.room.finished)
        // The sun sets off from where it was for the end of its arc, under the fading strip: no jump.
        assertEquals(11f / 12f, m.motion.sun(MY_DAY)!!, 1e-6f)
        assertEquals(11.5f / 12f, m.motion.motion.sun(MY_DAY, RELEASE_AT + M.SUN_TRAVEL / 2)!!, 1e-3f)
        assertEquals(1f, m.motion.motion.sun(MY_DAY, RELEASE_AT + M.SUN_TRAVEL))
        // Dusk falls from here; the hand-over takes taps only once it has begun to appear.
        assertEquals(0f, m.motion.dusk(MY_DAY)!!.scene)
        assertFalse(m.motion.dusk(MY_DAY)!!.handoverShown(0))
        m.frame(RELEASE_AT + M.DUSK / 2)
        assertFalse(m.motion.dusk(MY_DAY)!!.handoverShown(0))
        m.frame(RELEASE_AT + M.DUSK)
        assertTrue(m.motion.dusk(MY_DAY)!!.handoverShown(0))
        assertEquals(Wake.Idle, m.motion.motion.wake(time))
        // Reopening lifts it again, more briskly.
        m.setShowCompleted(true)
        advanceUntilIdle()
        m.setDone(last, false)
        advanceUntilIdle()
        m.frame(time + M.OVERLAY_OUT + M.ROW_COLLAPSE)
        advanceUntilIdle()
        assertFalse(m.ui.value.room.finished)
        val lifting = time
        m.frame(lifting + M.ROOM_HUE)
        assertEquals(DuskLook.rest(false), m.motion.dusk(MY_DAY))
    }

    @Test
    fun with_reduced_motion_the_finished_days_sun_is_set_at_once() = runTest {
        val engine = fixtureEngine("finished")
        val last = engine.room(MY_DAY, FIXTURE_NOW).completed.first().id
        engine.setDone(last, false)
        val m = model(engine = engine, pace = Pace.REDUCED)
        m.setDone(last, true)
        advanceUntilIdle()
        m.frame(10_000)
        advanceUntilIdle()
        assertTrue(m.ui.value.room.finished)
        assertEquals(1f, m.motion.sun(MY_DAY))
    }

    @Test
    fun reopening_in_the_completed_room_keeps_its_row_in_place_until_it_leaves() = runTest {
        val engine = fixtureEngine()
        val m = model(engine = engine)
        m.open(RoomKey.Smart(SmartList.COMPLETED))
        advanceUntilIdle()
        // The Completed smart list shows its done tasks among its rows, with no Completed section.
        val before = m.ui.value.room.tasks.map { it.id }
        assertTrue(before.size >= 2)
        assertTrue(m.ui.value.room.completed.isEmpty())
        val id = before[1]
        m.setDone(id, false)
        advanceUntilIdle()
        assertEquals(false, engine.taskDone(id))
        // Still in its place, and shown there, while its disc clears.
        assertEquals(before, m.ui.value.room.tasks.map { it.id })
        val look = m.motion.rowIn(id, Section.ROWS)!!
        assertEquals(1f to 1f, look.opacity to look.height)
        m.frame(M.OVERLAY_OUT / 2L)
        val clearing = m.motion.rowIn(id, Section.ROWS)!!
        assertTrue(clearing.fill > 0f && clearing.fill < 1f && clearing.opacity == 1f)
        // Then it collapses out and leaves the room, and the motion comes to rest.
        m.frame((M.OVERLAY_OUT + M.ROW_COLLAPSE).toLong())
        advanceUntilIdle()
        assertEquals(before - id, m.ui.value.room.tasks.map { it.id })
        m.frame(time + 5_000)
        assertNull(m.motion.row(id))
        assertEquals(Wake.Idle, m.motion.motion.wake(time))
    }

    /**
     * A dispatcher whose tasks wait until the test runs them, one at a time, first or last. Only the test's
     * thread queues work here: the [Stepped] model hears of engine changes from the test, not the engine.
     */
    private class Steps : CoroutineDispatcher() {
        private val queue = ArrayDeque<Runnable>()
        private val owner = Thread.currentThread()

        /** Work another thread tried to queue: dropped, and the test fails ([Stepped.drain]). */
        @Volatile var foreign = 0

        override fun dispatch(context: CoroutineContext, block: Runnable) {
            if (Thread.currentThread() != owner) foreign++ else queue.addLast(block)
        }

        fun next(): Boolean = queue.removeFirstOrNull()?.let { it.run(); true } ?: false

        fun last(): Boolean = queue.removeLastOrNull()?.let { it.run(); true } ?: false

        /** Takes the last task out of the queue, to run it later. */
        fun hold(): Runnable = queue.removeLast()
    }

    /**
     * A model whose own coroutines ([main]) and engine calls ([io]) run only when the test steps them. The
     * engine's change notifications are the test's to send ([changed]): its listener, on the engine's own
     * threads, would queue work in between the steps.
     */
    private class Stepped(engine: TodoEngine, zone: java.time.ZoneId = java.time.ZoneOffset.ofHours(1), documents: Documents? = null, clock: () -> Long) {
        val main = Steps()
        val io = Steps()
        val scope = CoroutineScope(SupervisorJob() + main)
        private var onChange: () -> Unit = {}
        val model = TodoModel(engine, scope, io, now = { FIXTURE_NOW }, clock = clock, listen = { onChange = it }, zone = { zone }, documents = documents)

        /** The engine says something changed, as its listener would. */
        fun changed() = onChange()

        /** Runs the model's own waiting tasks, not the engine calls they queue. */
        fun runMain() {
            var more = main.next()
            while (more) more = main.next()
        }

        /** Runs every waiting task, checking [each] after every one. */
        fun drain(each: () -> Unit = {}) {
            while (main.next() || io.next()) each()
            assertEquals("work queued from another thread", 0, main.foreign + io.foreign)
        }

        /** A refresh on its way, as a change from the engine starts one. */
        fun refreshLater() {
            scope.launch { model.refresh() }
        }
    }

    @Test
    fun remove_attachment_and_undo_restore_its_exact_reference_offline() {
        val e = fixtureEngine()
        val s = Stepped(e) { time }
        val id = e.taskId(RoomKey.Smart(SmartList.MY_DAY), "Call the dentist to reschedule")
        s.model.select(id)
        s.drain()
        val before = s.model.ui.value.detail!!.attachments
        val target = before.first { it.name == "Appointment.pdf" }
        s.model.organize(app.todoornot.model.OrganizeEdit.RemoveAttachment(target.id))
        s.drain()
        assertEquals(before - target, s.model.ui.value.detail!!.attachments)
        val toast = s.model.ui.value.toast!!
        assertEquals("Removed attachment “Appointment.pdf”", toast.text)
        s.model.undo(toast)
        s.drain()
        assertEquals(before, s.model.ui.value.detail!!.attachments)
        s.scope.cancel()
    }

    @Test
    fun transfer_state_survives_navigation_and_failed_or_cancelled_saves_discard_the_document() {
        val e = fixtureEngine()
        val discarded = mutableListOf<String>()
        var copies = 0
        val docs = object : Documents {
            override fun temporary() = java.io.File.createTempFile("attachment", null)
            override fun copyPicked(uri: String, file: java.io.File, check: () -> Unit): Documents.Picked {
                copies++
                return Documents.Picked("sample.txt", "text/plain")
            }
            override fun prepareOpen(file: java.io.File, row: app.todoornot.core.AttachmentRow) = error("offline")
            override fun save(file: java.io.File, uri: String, check: () -> Unit) = error("offline")
            override fun discardEmpty(uri: String) { discarded += uri }
        }
        val s = Stepped(e, documents = docs) { time }
        val id = e.taskId(RoomKey.Smart(SmartList.MY_DAY), "Call the dentist to reschedule")
        s.model.select(id)
        s.drain()
        val before = s.model.ui.value.detail!!.attachments
        val add = FileAction.Upload(id, "picked")
        s.model.transfer(add)
        s.model.transfer(add)
        assertEquals("Uploading…", s.model.ui.value.attachment?.text)
        val late = s.io.hold()
        s.model.select(null)
        s.drain()
        assertNull(s.model.ui.value.attachment)
        s.model.select(id)
        s.drain()
        assertTrue(s.model.ui.value.attachment!!.busy)
        late.run(); s.drain()
        assertEquals(1, copies)
        val offline = "Connect to upload or download attachments."
        assertEquals(offline, s.model.ui.value.attachment?.text)
        val save = FileAction.Save(id, before.first().id, before.first().name, "failed")
        s.model.transfer(save)
        s.drain()
        assertEquals(offline, s.model.ui.value.attachment?.text)
        s.model.transfer(save.copy(uri = "cancelled"))
        s.model.pause()
        s.drain()
        assertEquals(listOf("failed", "cancelled"), discarded)
        assertEquals("Transfer cancelled.", s.model.ui.value.attachment?.text)
        assertEquals(0u, e.status().pending)
        assertEquals(before, e.task(id, FIXTURE_NOW)!!.attachments)
        s.scope.cancel()
    }

    @Test
    fun activity_is_a_room_not_the_agent_activity_task_list() {
        val e = fixtureEngine()
        val s = Stepped(e) { time }
        val m = s.model
        s.drain()
        val agent = m.ui.value.home.agents.first()
        val id = m.ui.value.room.tasks.first().id
        m.select(id)
        m.setFilter("herbs")
        m.openActivity(agent.id)
        s.drain()
        assertEquals(agent.id, m.ui.value.activity)
        assertNull(m.ui.value.detail)
        assertEquals("", m.ui.value.filter)
        val first = m.ui.value.pagerId
        m.loadActivity(first, ActivityLoad.INITIAL)
        s.drain()
        val offline = ActivityFooter.Error("Couldn’t load activity. Try again.", "Try again")
        assertEquals(offline, m.ui.value.feed?.footer)

        // Publish the pending retry, but hold its network read until the replacement room is visible.
        m.loadActivity(first, ActivityLoad.RETRY)
        while (m.ui.value.feed?.footer !is ActivityFooter.Loading) assertTrue(s.main.next() || s.io.next())
        s.runMain()
        val late = s.io.hold()
        m.openActivity(null)
        s.drain()
        assertEquals("", m.ui.value.activity)
        val fresh = m.ui.value.feed
        assertNotNull(fresh)
        assertEquals(ActivityFooter.None, fresh?.footer)
        late.run()
        s.runMain()
        assertFalse("the replaced pager schedules no more work", s.io.next())
        m.loadActivity(first, ActivityLoad.REFRESH) // A stale gesture from the old room.
        s.drain()
        assertEquals(fresh, m.ui.value.feed)

        m.openActivity("not an agent!")
        s.drain()
        m.loadActivity(m.ui.value.pagerId, ActivityLoad.INITIAL)
        s.drain()
        assertEquals(offline, m.ui.value.feed?.footer)
        m.select(id) // A notification can open a task from Activity.
        s.drain()
        assertNull(m.ui.value.activity)
        assertEquals(id, m.ui.value.selected)
        s.scope.cancel()
    }

    @Test
    fun activity_pager_and_its_actions_survive_a_task_room_fallback() {
        val e = fixtureEngine()
        val s = Stepped(e) { time }
        val m = s.model
        val list = e.list("Errands") as RoomKey.List
        m.open(list)
        s.drain()
        m.openActivity(null)
        s.drain()
        val shown = m.ui.value
        e.deleteList(list.id, e.deleteListQuestion(list.id)!!)
        s.changed()
        s.drain()
        assertNotEquals(shown.generation, m.ui.value.generation)
        assertEquals(shown.pagerId, m.ui.value.pagerId)
        assertEquals(shown.feed, m.ui.value.feed)
        m.loadActivity(shown.pagerId, ActivityLoad.INITIAL)
        s.drain()
        assertTrue(m.ui.value.feed?.footer is ActivityFooter.Error)
        s.scope.cancel()
    }

    @Test
    fun organize_questions_recheck_after_queued_edits_and_undo_the_exact_change() {
        val e = fixtureEngine("blank")
        val id = e.addTask(MY_DAY, "Water herbs", FIXTURE_NOW)
        val s = Stepped(e) { time }
        val m = s.model
        m.select(id)
        val monday = CustomRepeat(RepeatUnit.WEEK, 1u, 1u)
        val preview = m.previewRepeat(id, monday)
        assertTrue(preview.valid)
        assertEquals("Every Monday", preview.description)
        assertEquals("Due will be 23 Nov 2026.", preview.dueNote)
        val invalid = m.previewRepeat(id, monday.copy(weekdays = 0u))
        assertFalse(invalid.valid)
        assertEquals("Choose at least one weekday.", invalid.description)
        m.organize(app.todoornot.model.OrganizeEdit.Repeat(id, app.todoornot.core.RepeatChoice.Preset(app.todoornot.core.RepeatPreset.WEEKDAYS)))
        s.drain()
        assertEquals("2026-11-23", m.ui.value.detail!!.due)
        m.undo(m.ui.value.toast!!)
        m.organize(app.todoornot.model.OrganizeEdit.TagNamed(id, "garden"))
        s.drain()
        assertNull(m.ui.value.detail!!.due)
        val tag = m.ui.value.detail!!.tags.single()
        val target = app.todoornot.core.ColorTarget.Tag(tag.id)
        val asked = m.organizeQuestion(target)!!
        var next: String? = null
        m.organize(app.todoornot.model.OrganizeEdit.ToggleTag(id, tag.id))
        m.confirmDelete(target, asked) { next = it }
        s.drain()
        assertEquals("Delete the tag “garden”? Undo restores the tag.", next)
        m.confirmDelete(target, next!!) { assertNull(it) }
        s.drain()
        assertTrue(m.ui.value.detail!!.tags.isEmpty())
        assertEquals("Deleted tag “garden”", m.ui.value.toast!!.text)
        m.undo(m.ui.value.toast!!)
        s.drain()
        assertEquals(tag.id, m.ui.value.detail!!.tags.single().id)
        m.organize(app.todoornot.model.OrganizeEdit.AddLink(id, "javascript:alert(1)"))
        s.drain()
        assertTrue(m.ui.value.detail!!.sources.isEmpty())
        assertFalse(m.ui.value.toast!!.undoable)
        m.organize(app.todoornot.model.OrganizeEdit.AddLink(id, "  todo.example.com  "))
        s.drain()
        val toast = m.ui.value.toast!!
        assertEquals("Added link “https://todo.example.com”", toast.text)
        val window = m.toastRemaining(toast)
        time += 123
        // A new surface reads the same model deadline, not a fresh countdown.
        assertEquals(window - 123, m.toastRemaining(m.read().toast!!))
        time += window
        m.undo(toast)
        s.drain()
        assertNull(m.ui.value.toast)
        assertEquals(1, m.ui.value.detail!!.sources.size)
        s.scope.cancel()
    }

    @Test
    fun a_second_tap_keeps_the_row_and_the_counts_until_its_write_lands() {
        val engine = fixtureEngine()
        val s = Stepped(engine) { time }
        val m = s.model
        s.drain()
        val id = m.ui.value.room.tasks.first().id
        m.setDone(id, true)
        s.drain()
        assertEquals(true, engine.taskDone(id))
        m.frame(EXIT_AT - 100)
        // A read already on its way (the engine said something changed), then the tap that reverses the
        // completion: the read runs while the reversal is still to be written.
        s.refreshLater()
        m.setDone(id, false)
        s.drain {
            val room = m.ui.value.room
            assertEquals(id, room.tasks.first().id)
            assertFalse(room.completed.any { it.id == id })
            assertEquals(3u, room.done)
            assertFalse(room.finished)
        }
        assertEquals(false, engine.taskDone(id))
        m.frame(time + 5_000)
        s.drain()
        assertEquals(id, m.ui.value.room.tasks.first().id)
        assertTrue(m.motion.motion.holding().isEmpty())
        assertEquals(Wake.Idle, m.motion.motion.wake(time))
    }

    @Test
    fun a_reversal_written_after_its_settle_still_keeps_the_row_and_the_day_unfinished() {
        val engine = fixtureEngine("finished")
        val last = engine.room(MY_DAY, FIXTURE_NOW).completed.first().id
        engine.setDone(last, false)
        val s = Stepped(engine) { time }
        val m = s.model
        s.drain()
        val done = m.ui.value.room.done
        assertFalse(m.ui.value.room.finished)
        // The last task of the day is completed, then the tap is reversed before the row leaves.
        m.setDone(last, true)
        s.drain()
        assertEquals(true, engine.taskDone(last))
        m.frame(EXIT_AT - 100)
        m.setDone(last, false)
        // What the tap started runs up to its write, queued last; the write is held back.
        s.runMain()
        val write = s.io.hold()
        fun unchanged() {
            val room = m.ui.value.room
            assertTrue(room.tasks.any { it.id == last })
            assertFalse(room.completed.any { it.id == last })
            assertEquals(done, room.done)
            assertFalse(room.finished)
            assertEquals(DuskLook.rest(false), m.motion.dusk(MY_DAY))
        }
        s.drain { unchanged() }
        assertEquals("the reversal isn't written yet", true, engine.taskDone(last))
        // The settle ends long before the write lands: the row rests where it is, still kept for its write.
        // Then the engine says something changed.
        m.frame(time + 5_000)
        assertEquals(RowLook.rest(false), m.motion.row(last))
        s.changed()
        s.drain { unchanged() }
        unchanged()
        assertEquals(true, engine.taskDone(last))
        assertEquals(RowLook.rest(false), m.motion.row(last))
        // The write lands: nothing moves, and My Day has the row anyway, so it simply comes to rest.
        write.run()
        s.drain { unchanged() }
        assertEquals(false, engine.taskDone(last))
        unchanged()
        assertNull(m.motion.row(last))
        assertEquals(Wake.Idle, m.motion.motion.wake(time))
    }

    @Test
    fun opening_another_room_lets_go_of_a_row_kept_for_its_write() {
        val engine = fixtureEngine()
        val s = Stepped(engine) { time }
        val m = s.model
        s.drain()
        val id = m.ui.value.room.tasks.first().id
        m.setDone(id, true)
        s.drain()
        // The completion is written; the tap that reverses it isn't yet. The engine has the task done, but
        // the row stays open, counted as open, past the end of its choreography.
        m.frame(EXIT_AT - 100)
        m.setDone(id, false)
        s.runMain()
        val write = s.io.hold()
        s.drain()
        m.frame(RELEASE_AT + 5_000)
        s.drain()
        assertEquals(true, engine.taskDone(id))
        assertTrue(m.ui.value.room.tasks.any { it.id == id })
        assertEquals(3u, m.ui.value.room.done)
        // Another room, and back, before the write lands: the rows are the engine's own, done.
        m.open(RoomKey.Smart(SmartList.ALL))
        m.open(MY_DAY)
        s.drain()
        assertFalse(m.ui.value.room.tasks.any { it.id == id })
        assertTrue(m.ui.value.room.completed.isEmpty() || !m.ui.value.showCompleted)
        assertEquals(4u, m.ui.value.room.done)
        assertTrue(m.motion.motion.holding().isEmpty())
        assertNull(m.motion.row(id))
        // The write lands: the task is open again, and so is its row.
        write.run()
        s.drain()
        assertEquals(false, engine.taskDone(id))
        assertTrue(m.ui.value.room.tasks.any { it.id == id })
        assertEquals(3u, m.ui.value.room.done)
    }

    @Test
    fun creating_a_list_enters_it_as_opening_a_room_does() {
        val engine = fixtureEngine()
        val s = Stepped(engine) { time }
        val m = s.model
        s.drain()
        val id = m.ui.value.room.tasks.first().id
        m.setShowCompleted(true)
        m.setFilter("a")
        m.select(id)
        s.drain()
        m.setDone(id, true)
        s.drain()
        m.frame(EXIT_AT)
        assertEquals(setOf(id), m.motion.motion.holding())
        m.createList("Garden")
        s.drain()
        val ui = m.ui.value
        assertEquals("Garden", ui.room.title)
        assertNull(ui.detail)
        assertEquals("", ui.filter)
        assertFalse(ui.searching)
        assertFalse(ui.showCompleted)
        // At rest: what played in the room before is let go.
        assertTrue(m.motion.motion.holding().isEmpty())
        assertNull(m.motion.row(id))
        assertEquals(Wake.Idle, m.motion.motion.wake(time))
    }

    /**
     * An edit drops a completing row from Agent activity despite its hold. It plays on as a ghost,
     * inert; the detail checkbox reverses it there. It settles, then collapses out, whether the
     * reversal's write lands [before] the settle ends or after the row has gone.
     */
    private fun anAgentRowPlaysOutAsAGhost(before: Boolean) {
        val engine = WithAgent(fixtureEngine())
        val s = Stepped(engine) { time }
        val m = s.model
        s.drain()
        m.open(AGENTS)
        s.drain()
        val room = m.ui.value.room
        val order = room.tasks.map { it.id }
        val index = room.tasks.indexOfFirst { it.id != order.first() && !it.done }
        val row = room.tasks[index]
        engine.omitted += row.id
        m.setDone(row.id, true, m.ui.value.generation)
        s.drain()
        assertEquals(true, engine.taskDone(row.id))
        assertFalse(m.ui.value.room.tasks.any { it.id == row.id })
        // But it's painted where it was, as it was, filling.
        assertEquals(listOf(Ghost(row, Section.ROWS, order.take(index).reversed())), m.motion.ghosts())
        assertEquals(order, m.drawn())
        assertEquals(true, m.motion.checked(row.id))
        // A tap on it does nothing at all.
        m.frame(EXIT_AT - 100)
        val look = m.motion.row(row.id)
        m.setDone(row.id, false, m.ui.value.generation)
        s.drain()
        assertEquals(true, engine.taskDone(row.id))
        assertEquals(look, m.motion.row(row.id))
        assertEquals(setOf(row.id), m.motion.motion.holding())
        // A manual reopen keeps the owner as last editor, unlike cancelling the unsent completion.
        m.setDone(row.id, false, m.ui.value.generation, row = false)
        var write: Runnable? = null
        if (before) {
            s.drain()
        } else {
            s.runMain()
            write = s.io.hold()
            s.drain()
        }
        assertEquals(look, m.motion.row(row.id))
        assertEquals(order, m.drawn())
        // Settled open, it collapses out from its full height, in place.
        val settled = settledFrom(EXIT_AT - 100)
        m.frame(settled)
        s.drain()
        assertEquals(order, m.drawn())
        assertEquals(RowLook.rest(false), m.motion.rowIn(row.id, Section.ROWS))
        m.frame(settled + M.ROW_COLLAPSE / 2)
        s.drain()
        assertEquals(order, m.drawn())
        val half = m.motion.rowIn(row.id, Section.ROWS)!!
        assertEquals(1f - M.EASE_EXIT.ease((M.ROW_COLLAPSE / 2).toFloat() / M.ROW_COLLAPSE), half.height, 1e-4f)
        // Gone: the list no longer draws it, and the motion is at rest.
        m.frame(settled + M.ROW_COLLAPSE)
        s.drain()
        assertEquals(order - row.id, m.drawn())
        assertTrue(m.motion.ghosts().isEmpty())
        assertNull(m.motion.row(row.id))
        assertEquals(Wake.Idle, m.motion.motion.wake(time))
        if (!before) {
            assertEquals("the reversal isn't written yet", true, engine.taskDone(row.id))
            write!!.run()
            s.drain()
        }
        assertEquals(false, engine.taskDone(row.id))
        assertEquals(order - row.id, m.drawn())
        assertNull(m.motion.row(row.id))
        assertEquals(Wake.Idle, m.motion.motion.wake(time))
    }

    @Test
    fun an_agent_row_plays_out_as_a_ghost_when_its_reversal_is_written_before_the_settle_ends() =
        anAgentRowPlaysOutAsAGhost(before = true)

    @Test
    fun an_agent_row_plays_out_as_a_ghost_when_its_reversal_is_written_after_it_has_gone() =
        anAgentRowPlaysOutAsAGhost(before = false)

    /** A remote edit drops a completing row from the filter: it plays out as a ghost, no snap. */
    @Test
    fun a_row_the_filter_drops_once_toggled_plays_out_as_a_ghost() {
        val engine = WithAgent(fixtureEngine())
        val s = Stepped(engine) { time }
        val m = s.model
        s.drain()
        m.open(ALL)
        m.setFilter(BY_OWNER)
        s.drain()
        val room = m.ui.value.room
        assertTrue(room.filtered)
        val order = room.tasks.map { it.id }
        val index = room.tasks.indexOfLast { !it.done }
        val row = room.tasks[index]
        m.setDone(row.id, true, m.ui.value.generation)
        s.drain()
        engine.agentTouch(row.id)
        s.changed()
        // At every step, until it has gone, the row is drawn where it was.
        fun inPlace() = assertEquals(order, m.drawn())
        s.drain { inPlace() }
        assertFalse(m.ui.value.room.tasks.any { it.id == row.id })
        assertFalse(m.ui.value.room.completed.any { it.id == row.id })
        assertTrue(m.motion.motion.isGhost(row.id))
        for (t in listOf(M.CHECK_FILL.toLong(), EXIT_AT - 1, EXIT_AT + M.ROW_COLLAPSE / 2)) {
            m.frame(t)
            s.drain { inPlace() }
            inPlace()
        }
        val collapsing = m.motion.rowIn(row.id, Section.ROWS)!!
        assertTrue(collapsing.height > 0f && collapsing.height < 1f)
        // At its release it's gone, entering nowhere.
        m.frame(RELEASE_AT)
        s.drain()
        assertEquals(order - row.id, m.drawn())
        assertNull(m.motion.row(row.id))
        assertTrue(m.motion.ghosts().isEmpty())
        m.frame(time + 5_000)
        assertEquals(Wake.Idle, m.motion.motion.wake(time))
    }

    /**
     * A completing row drops out of Agent activity and the owner reverses it; while its ghost collapses
     * out, an agent completes it again. The room lists it again: the ghost goes at once, and the live row
     * shows as the read has it, done and at rest, with nothing left over from the ghost.
     */
    @Test
    fun a_ghost_the_room_lists_again_gives_way_to_the_live_row_at_rest() {
        val engine = WithAgent(fixtureEngine())
        val s = Stepped(engine) { time }
        val m = s.model
        s.drain()
        m.open(AGENTS)
        s.drain()
        val order = m.ui.value.room.tasks.map { it.id }
        val row = m.ui.value.room.tasks.first { it.id != order.first() && !it.done }
        engine.omitted += row.id
        m.setDone(row.id, true, m.ui.value.generation)
        s.drain()
        m.frame(EXIT_AT - 100)
        m.setDone(row.id, false, m.ui.value.generation, row = false)
        s.drain()
        assertEquals(false, engine.taskDone(row.id))
        // Mid-exit.
        val settled = settledFrom(EXIT_AT - 100)
        m.frame(settled + M.ROW_COLLAPSE / 2)
        s.drain()
        assertTrue(m.motion.motion.isGhost(row.id))
        assertTrue(m.motion.rowIn(row.id, Section.ROWS)!!.height < 1f)
        // The agent completes it again, and the engine says so.
        engine.agentSetDone(row.id, true)
        s.changed()
        s.drain()
        val listed = m.ui.value.room.tasks.first()
        assertEquals(row.id, listed.id)
        assertTrue(listed.done)
        assertTrue(m.motion.ghosts().isEmpty())
        assertNull(m.motion.row(row.id))
        assertNull(m.motion.checked(row.id))
        assertEquals(listOf(row.id) + (order - row.id), m.drawn())
        m.frame(time + 5_000)
        s.drain()
        assertNull(m.motion.row(row.id))
        assertTrue(m.ui.value.room.tasks.first().done)
        assertEquals(Wake.Idle, m.motion.motion.wake(time))
    }

    /**
     * A completing row drops out of Agent activity; while its ghost holds, an agent changes it, so
     * the room lists it again. The read that lists it shows it as it is, done, never as the open row
     * the hold would place.
     */
    @Test
    fun a_ghost_the_room_lists_again_mid_hold_shows_as_it_is() {
        val engine = WithAgent(fixtureEngine())
        val s = Stepped(engine) { time }
        val m = s.model
        s.drain()
        m.open(AGENTS)
        s.drain()
        val order = m.ui.value.room.tasks.map { it.id }
        val row = m.ui.value.room.tasks.first { it.id != order.first() && !it.done }
        engine.omitted += row.id
        m.setDone(row.id, true, m.ui.value.generation)
        s.drain()
        assertTrue(m.motion.motion.isGhost(row.id))
        assertEquals(setOf(row.id), m.motion.motion.holding())
        m.frame(EXIT_AT - 100)
        s.drain()
        // An agent renames it, and the engine says so.
        engine.agentTouch(row.id)
        s.changed()
        fun asItIs() {
            val listed = m.ui.value.room.tasks.firstOrNull { it.id == row.id } ?: return
            assertTrue("listed done", listed.done)
            assertEquals(true, m.motion.checked(row.id) ?: listed.done)
        }
        s.drain { asItIs() }
        val listed = m.ui.value.room.tasks.first()
        assertEquals(row.id, listed.id)
        assertTrue(listed.done)
        assertTrue(m.motion.ghosts().isEmpty())
        assertNull(m.motion.row(row.id))
        assertTrue(m.motion.motion.holding().isEmpty())
        assertEquals(listOf(row.id) + (order - row.id), m.drawn())
        m.frame(time + 5_000)
        s.drain { asItIs() }
        assertEquals(Wake.Idle, m.motion.motion.wake(time))
    }

    /**
     * On a wide screen, the detail pane's checkbox, tapped after another room opens and before its read
     * lands, only writes; so does Undo in that gap. Nothing in the new room is held, placed or painted.
     */
    @Test
    fun the_detail_checkbox_and_undo_in_the_gap_after_a_room_switch_only_write() {
        val engine = fixtureEngine()
        val s = Stepped(engine) { time }
        val m = s.model
        s.drain()
        val id = m.ui.value.room.tasks.first().id
        m.select(id)
        s.drain()
        val shown = m.ui.value
        assertEquals(id, shown.detail?.id)
        m.open(ALL)
        assertEquals("the screen still shows My Day", MY_DAY, m.ui.value.room.key)
        // The detail pane's checkbox, as the pane passes it.
        m.setDone(id, true, shown.generation, row = false)
        assertTrue(m.motion.motion.ids().isEmpty())
        s.drain { assertTrue(m.motion.motion.ids().isEmpty()) }
        assertEquals(true, engine.taskDone(id))
        assertEquals(ALL, m.ui.value.room.key)
        assertTrue(m.ui.value.room.completed.any { it.id == id })
        assertTrue(m.motion.motion.holding().isEmpty())
        // Undo, after My Day opens again and before its read lands.
        val toast = m.ui.value.toast!!
        assertEquals(id, toast.completion?.task)
        m.open(MY_DAY)
        assertEquals(ALL, m.ui.value.room.key)
        m.undo(toast)
        assertTrue(m.motion.motion.ids().isEmpty())
        s.drain { assertTrue(m.motion.motion.ids().isEmpty()) }
        assertEquals(false, engine.taskDone(id))
        assertEquals(MY_DAY, m.ui.value.room.key)
        assertTrue(m.ui.value.room.tasks.any { it.id == id })
        assertTrue(m.motion.motion.reopening().isEmpty())
        assertTrue(m.motion.ghosts().isEmpty())
        // In the room on screen, the detail pane's checkbox moves as ever.
        m.setDone(id, true, m.ui.value.generation, row = false)
        assertEquals(setOf(id), m.motion.motion.holding())
    }

    /**
     * A task completes in a list, and a sync deletes the list (the task now elsewhere, in My Day): the
     * engine falls back to My Day. That's a new room, at rest: nothing of the list's choreography plays on
     * in My Day, and a tap on a row of the list only writes.
     */
    @Test
    fun a_room_the_engine_falls_back_from_leaves_its_motion_behind() {
        val real = fixtureEngine()
        val (list, id) = real.home(FIXTURE_NOW).groups.flatMap { it.lists }.firstNotNullOf { l ->
            real.room(l.key, FIXTURE_NOW).tasks.firstOrNull { it.myDay && !it.done }?.let { l.key to it.id }
        }
        val engine = WithAgent(real)
        val s = Stepped(engine) { time }
        val m = s.model
        s.drain()
        m.open(list)
        s.drain()
        val inList = m.ui.value
        assertEquals(list, inList.room.key)
        m.setDone(id, true, inList.generation)
        s.drain()
        assertEquals(setOf(id), m.motion.motion.holding())
        m.frame(EXIT_AT - 100)
        s.drain()
        engine.gone = list
        s.changed()
        fun atRest() {
            val ui = m.ui.value
            if (ui.room.key == list) return
            assertNotEquals(inList.generation, ui.generation)
            assertFalse("not held open", ui.room.tasks.any { it.id == id })
            assertTrue(m.motion.motion.ids().isEmpty())
            assertTrue(m.motion.ghosts().isEmpty())
        }
        s.drain { atRest() }
        assertEquals(MY_DAY, m.ui.value.room.key)
        atRest()
        assertEquals(real.room(MY_DAY, FIXTURE_NOW).done, m.ui.value.room.done)
        // A tap on a row of the list only writes.
        m.setDone(id, false, inList.generation)
        assertTrue(m.motion.motion.ids().isEmpty())
        s.drain()
        assertEquals(false, real.taskDone(id))
        assertTrue(m.motion.motion.ids().isEmpty())
        m.frame(time + 5_000)
        assertEquals(Wake.Idle, m.motion.motion.wake(time))
    }

    /**
     * Under a filter on who changed a task last, the owner completes a task and undoes it. While it settles,
     * an agent completes it, and the read that has it done no longer lists it: its row plays on as a ghost,
     * then collapses out, never snapping.
     */
    @Test
    fun a_settling_row_the_data_drops_plays_out_as_a_ghost() {
        val engine = WithAgent(fixtureEngine())
        val s = Stepped(engine) { time }
        val m = s.model
        s.drain()
        m.open(ALL)
        m.setFilter(BY_OWNER)
        s.drain()
        val order = m.ui.value.room.tasks.map { it.id }
        val row = m.ui.value.room.tasks.first { !it.done }
        m.setDone(row.id, true, m.ui.value.generation)
        s.drain()
        val undoAt = EXIT_AT - 100
        m.frame(undoAt)
        m.undo(m.ui.value.toast!!)
        s.drain()
        assertEquals(false, engine.taskDone(row.id))
        assertEquals(order, m.drawn())
        // Mid-settle, an agent completes it.
        val settled = settledFrom(undoAt)
        m.frame((undoAt + settled) / 2)
        engine.agentSetDone(row.id, true)
        s.changed()
        fun inPlace() = assertEquals(order, m.drawn())
        s.drain { inPlace() }
        assertFalse(m.ui.value.room.tasks.any { it.id == row.id })
        assertTrue(m.motion.motion.isGhost(row.id))
        for (t in listOf(settled, settled + M.ROW_COLLAPSE / 2)) {
            m.frame(t)
            s.drain { inPlace() }
        }
        assertTrue(m.motion.rowIn(row.id, Section.ROWS)!!.height < 1f)
        m.frame(settled + M.ROW_COLLAPSE)
        s.drain()
        assertEquals(order - row.id, m.drawn())
        assertTrue(m.motion.ghosts().isEmpty())
        m.frame(time + 5_000)
        assertEquals(Wake.Idle, m.motion.motion.wake(time))
    }

    /** A task deleted while its row settles stops at once: there is nothing to paint. */
    @Test
    fun a_settling_row_whose_task_is_deleted_stops_at_once() {
        val engine = fixtureEngine()
        val s = Stepped(engine) { time }
        val m = s.model
        s.drain()
        val id = m.ui.value.room.tasks.first().id
        m.setDone(id, true)
        s.drain()
        val undoAt = EXIT_AT - 100
        m.frame(undoAt)
        m.undo(m.ui.value.toast!!)
        s.drain()
        m.frame((undoAt + settledFrom(undoAt)) / 2)
        engine.fixtureDeleteTask(id)
        s.changed()
        s.drain()
        assertFalse(m.drawn().contains(id))
        assertTrue(m.motion.ghosts().isEmpty())
        assertNull(m.motion.row(id))
    }

    /**
     * Two completing rows are dropped from Agent activity: every row stays where it was painted as
     * the second ghost arrives and as the first leaves.
     */
    @Test
    fun staggered_ghosts_keep_the_rows_in_order() {
        val engine = WithAgent(fixtureEngine())
        val s = Stepped(engine) { time }
        val m = s.model
        s.drain()
        m.open(AGENTS)
        s.drain()
        val rows = m.ui.value.room.tasks
        val order = rows.map { it.id }
        val i = rows.indices.first { it + 2 < rows.size && !rows[it].done && !rows[it + 2].done }
        val a = order[i]
        val c = order[i + 2]
        engine.omitted += a
        m.setDone(a, true, m.ui.value.generation)
        s.drain { assertEquals(order, m.drawn()) }
        assertTrue(m.motion.motion.isGhost(a))
        // The second ghost arrives.
        val second = 500L
        m.frame(second)
        engine.omitted += c
        m.setDone(c, true, m.ui.value.generation)
        assertEquals(order, m.drawn())
        s.drain { assertEquals(order, m.drawn()) }
        assertEquals(setOf(a, c), m.motion.motion.ghostIds())
        // The first leaves.
        m.frame(RELEASE_AT)
        assertEquals(order - a, m.drawn())
        s.drain { assertEquals(order - a, m.drawn()) }
        assertEquals(setOf(c), m.motion.motion.ghostIds())
        m.frame(second + RELEASE_AT)
        s.drain { assertEquals(order - a - c, m.drawn()) }
        assertTrue(m.motion.ghosts().isEmpty())
    }

    /**
     * The filter's only match drops out of Agent activity while completing: "Nothing matches the
     * filter" waits until it has gone, so it never pushes the ghost down.
     */
    @Test
    fun the_empty_line_waits_for_the_last_ghost() {
        val engine = WithAgent(fixtureEngine())
        val s = Stepped(engine) { time }
        val m = s.model
        s.drain()
        m.open(AGENTS)
        s.drain()
        val titles = m.ui.value.room.tasks.groupBy { it.title }
        val row = m.ui.value.room.tasks.first { !it.done && titles[it.title]!!.size == 1 }
        m.setFilter(row.title)
        s.drain()
        assertEquals(listOf(row.id), m.ui.value.room.tasks.map { it.id })
        fun empty() = showsEmptyLine(m.ui.value.room, m.motion.ghosts())
        engine.omitted += row.id
        m.setDone(row.id, true, m.ui.value.generation)
        s.drain { assertFalse(empty()) }
        assertTrue(m.ui.value.room.tasks.isEmpty())
        assertTrue(m.motion.motion.isGhost(row.id))
        for (t in listOf(EXIT_AT, RELEASE_AT - 1)) {
            m.frame(t)
            s.drain { assertFalse(empty()) }
        }
        m.frame(RELEASE_AT)
        s.drain()
        assertTrue(m.motion.ghosts().isEmpty())
        assertTrue(empty())
    }

    /**
     * A tap on a row in the gap after another room opens, before its read lands: written (the toast offers
     * Undo), but nothing in the new room is held, placed, painted or moved.
     */
    @Test
    fun a_tap_on_a_row_of_the_room_before_only_writes() {
        val engine = fixtureEngine()
        val s = Stepped(engine) { time }
        val m = s.model
        s.drain()
        val shown = m.ui.value
        val id = shown.room.tasks.first().id
        m.open(ALL)
        assertEquals("the rows on screen are still My Day's", MY_DAY, m.ui.value.room.key)
        m.setDone(id, true, shown.generation)
        assertTrue(m.motion.motion.ids().isEmpty())
        s.drain { assertNull(m.motion.row(id)) }
        assertEquals(true, engine.taskDone(id))
        val ui = m.ui.value
        assertEquals(ALL, ui.room.key)
        assertNotEquals(shown.generation, ui.generation)
        assertEquals(id, ui.toast?.completion?.task)
        // All has it done at once: not held among its open rows, and counted done.
        assertFalse(ui.room.tasks.any { it.id == id })
        assertTrue(ui.room.completed.any { it.id == id })
        assertEquals(engine.room(ALL, FIXTURE_NOW).done, ui.room.done)
        assertTrue(m.motion.motion.ids().isEmpty())
        assertTrue(m.motion.ghosts().isEmpty())
        assertTrue(m.motion.motion.holding().isEmpty())
        // Only the sun moves, to All's new count.
        m.frame(time + M.SUN_TRAVEL)
        assertEquals(Wake.Idle, m.motion.motion.wake(time))
        // A tap on the new room's rows moves as ever.
        val next = ui.room.tasks.first().id
        m.setDone(next, true, ui.generation)
        assertNotNull(m.motion.row(next))
    }

    /**
     * A read that finds the task deleted is published after the task was restored and the owner reversed
     * its completion: the reversal, still to be written, keeps its row.
     */
    @Test
    fun a_read_of_a_deleted_task_never_lets_go_of_a_newer_reversal() {
        val engine = fixtureEngine()
        val s = Stepped(engine) { time }
        val m = s.model
        s.drain()
        val id = m.ui.value.room.tasks.first().id
        m.setDone(id, true)
        s.drain()
        m.frame(EXIT_AT - 100)
        // Deleted elsewhere; a read takes that in.
        engine.fixtureDeleteTask(id)
        s.refreshLater()
        s.main.next()
        s.io.next()
        // Restored (the engine says so), and the owner reverses the completion before that read is
        // published. The reversal's write waits.
        engine.fixtureRestoreTask(id)
        s.changed()
        m.setDone(id, false)
        s.main.last()
        val write = s.io.hold()
        // The old read shows the task gone for a moment; it never lets the newer reversal go, so the task
        // is never placed done.
        fun neverDone() {
            val room = m.ui.value.room
            assertFalse(room.completed.any { it.id == id })
            assertEquals(3u, room.done)
        }
        fun kept() {
            neverDone()
            assertTrue(m.ui.value.room.tasks.any { it.id == id })
        }
        s.drain { neverDone() }
        kept()
        assertEquals("the reversal isn't written yet", true, engine.taskDone(id))
        m.frame(time + 5_000)
        s.drain { kept() }
        kept()
        write.run()
        s.drain { kept() }
        assertEquals(false, engine.taskDone(id))
        assertNull(m.motion.row(id))
        assertEquals(Wake.Idle, m.motion.motion.wake(time))
    }

    @Test
    fun a_read_with_a_superseded_placement_is_never_published() {
        val engine = fixtureEngine()
        val s = Stepped(engine) { time }
        val m = s.model
        s.drain()
        val id = m.ui.value.room.tasks.first().id
        m.setDone(id, true)
        s.drain()
        m.frame(RELEASE_AT - 10)
        // Two reads start; the second reads the engine first, while the row is still held.
        s.refreshLater()
        s.refreshLater()
        s.main.next()
        s.main.next()
        s.io.last()
        // The row is released, and the first read follows it.
        m.frame(RELEASE_AT)
        s.io.next()
        s.main.last()
        assertFalse(m.ui.value.room.tasks.any { it.id == id })
        assertEquals(4u, m.ui.value.room.done)
        // The second read, held with the old placement, arrives last: it would put the row back.
        s.drain {
            assertFalse(m.ui.value.room.tasks.any { it.id == id })
            assertEquals(4u, m.ui.value.room.done)
        }
    }

    @Test
    fun a_read_from_before_the_completed_toggle_never_undoes_it() {
        val s = Stepped(fixtureEngine()) { time }
        val m = s.model
        s.drain()
        // A read takes the engine's state, then Completed opens before it's published.
        s.refreshLater()
        s.main.next()
        s.io.next()
        m.setShowCompleted(true)
        assertTrue(m.ui.value.showCompleted)
        s.drain { assertTrue(m.ui.value.showCompleted) }
        assertTrue(m.ui.value.showCompleted)
    }

    @Test
    fun agent_activity_keeps_the_row_score_and_sun_until_release_even_when_filtered() = runTest {
        val engine = fixtureEngine()
        val m = model(engine = engine)
        m.open(AGENTS)
        m.setFilter("updated_by:agent")
        advanceUntilIdle()
        val before = m.ui.value.room
        val order = before.tasks.map { it.id }
        val sun = m.motion.sun(AGENTS)
        // An open task an agent changed, below the latest change.
        val row = before.tasks.drop(1).first { !it.done }
        assertNotNull(row.agentUpdate)
        m.setDone(row.id, true, m.ui.value.generation)
        advanceUntilIdle()
        assertEquals(true, engine.taskDone(row.id))
        assertFalse(engine.room(AGENTS, FIXTURE_NOW).tasks.any { it.id == row.id })
        // The placed read still lists it in the same order and acts as if it's open.
        assertEquals(before, m.ui.value.room)
        assertEquals(order, m.drawn())
        assertTrue(m.motion.ghosts().isEmpty())
        assertEquals(true, m.motion.checked(row.id))
        m.frame(EXIT_AT + M.ROW_COLLAPSE / 2)
        m.refresh()
        assertEquals(before, m.ui.value.room)
        assertEquals(sun, m.motion.sun(AGENTS))
        val look = m.motion.rowIn(row.id, Section.ROWS)!!
        assertTrue(look.height > 0f && look.height < 1f)
        m.frame(RELEASE_AT)
        advanceUntilIdle()
        val released = m.ui.value.room
        assertEquals(order - row.id, m.drawn())
        assertEquals(before.done, released.done)
        assertEquals(before.total - 1u, released.total)
        assertEquals(before.open - 1u, released.open)
        assertNull(m.motion.row(row.id))
        m.frame(time + 5_000)
        assertEquals(Wake.Idle, m.motion.motion.wake(time))
    }

    @Test
    fun ghosts_are_drawn_back_in_their_places() {
        val rows = (0 until 4).map { fixtureEngine().room(MY_DAY, FIXTURE_NOW).tasks[it] }
        val (a, b, c, d) = rows
        fun ghost(t: app.todoornot.core.TaskRow, vararg above: app.todoornot.core.TaskRow, section: Section = Section.ROWS) =
            Ghost(t, section, above.map { it.id })
        fun ids(rows: List<app.todoornot.model.Shown>) = rows.map { it.row.id }
        // Each under the row it was painted under (C was painted under B, with the ghost of A above).
        val drawn = withGhosts(listOf(b, d), listOf(ghost(c, b, a), ghost(a)), Section.ROWS)
        assertEquals(listOf(a, b, c, d).map { it.id }, ids(drawn))
        assertEquals(listOf(true, false, true, false), drawn.map { it.ghost })
        // The row it was under has gone: under the nearest one above that's still drawn, else at the top.
        assertEquals(listOf(b, c, d).map { it.id }, ids(withGhosts(listOf(b, d), listOf(ghost(c, b, a)), Section.ROWS)))
        assertEquals(listOf(a, c, d).map { it.id }, ids(withGhosts(listOf(a, d), listOf(ghost(c, b, a)), Section.ROWS)))
        assertEquals(listOf(c, d).map { it.id }, ids(withGhosts(listOf(d), listOf(ghost(c, b, a)), Section.ROWS)))
        // Under another ghost, which goes under its own.
        assertEquals(listOf(b, c, d).map { it.id }, ids(withGhosts(listOf(d), listOf(ghost(c, b, a), ghost(b, a)), Section.ROWS)))
        // Another section's ghosts, and a ghost the rows list again, aren't drawn here.
        val other = withGhosts(listOf(b), listOf(ghost(a, section = Section.COMPLETED), ghost(b), ghost(c, d)), Section.ROWS)
        assertEquals(listOf(c.id to true, b.id to false), other.map { it.row.id to it.ghost })
    }

    /** The engine's own listener, calling back on its own threads, refreshes the screen. */
    @Test
    fun a_change_the_engine_announces_reaches_the_screen() = runTest {
        val engine = fixtureEngine()
        val m = listening(engine)
        val id = m.ui.value.room.tasks.first().id
        // Written behind the model's back, as a sync would.
        engine.setDone(id, true)
        val deadline = System.nanoTime() + 10_000_000_000L
        while (m.ui.value.room.done != 4u) {
            assertTrue("the screen never heard of the change", System.nanoTime() < deadline)
            Thread.sleep(10)
        }
        assertFalse(m.ui.value.room.tasks.any { it.id == id })
    }

    @Test
    fun selecting_renaming_and_my_day() = runTest {
        val engine = fixtureEngine()
        val m = model(engine = engine)
        val personal = engine.collection("Personal")
        m.open(personal)
        advanceUntilIdle()
        assertEquals("Personal", m.ui.value.room.title)
        val id = engine.taskId(personal, "Refill the prescription")
        m.select(id)
        advanceUntilIdle()
        assertEquals("Refill the prescription", m.ui.value.detail?.title)

        m.rename(id, "Refill both prescriptions")
        m.setMyDay(id, true)
        advanceUntilIdle()
        val detail = m.ui.value.detail!!
        assertEquals("Refill both prescriptions", detail.title)
        assertTrue(detail.myDay)

        m.select(null)
        advanceUntilIdle()
        assertNull(m.ui.value.detail)
    }

    @Test
    fun row_and_detail_read_the_same_important_state() = runTest {
        val engine = fixtureEngine()
        val personal = engine.collection("Personal")
        val id = engine.taskId(personal, "Refill the prescription")
        val m = model(engine = engine)
        m.open(personal)
        advanceUntilIdle()
        m.select(id)
        for (on in listOf(true, false)) {
            m.setImportant(id, on)
            advanceUntilIdle()
            val ui = m.ui.value
            assertEquals(on, ui.room.tasks.single { it.id == id }.priority == 3u.toUByte())
            assertEquals(on, ui.detail?.priority == 3u.toUByte())
        }
    }

    @Test
    fun an_empty_title_is_refused_with_a_message() = runTest {
        val m = model()
        m.add("   ")
        advanceUntilIdle()
        assertEquals("A task needs a title.", m.ui.value.toast?.text)
        assertEquals(12u, m.ui.value.room.total)
    }

    /** A model whose coroutines and engine calls wait in the queue until the test runs them. */
    private fun TestScope.queued(engine: TodoEngine): Pair<TodoModel, CoroutineScope> {
        val d = StandardTestDispatcher(testScheduler)
        val scope = CoroutineScope(SupervisorJob() + d)
        return TodoModel(engine, scope, d, now = { FIXTURE_NOW }) to scope
    }

    @Test
    fun going_to_the_background_before_a_queued_start_runs_leaves_the_engine_stopped() = runTest {
        val engine = fixtureEngine()
        val (m, scope) = queued(engine)
        try {
            m.resume()
            m.pause()
            advanceUntilIdle()
            assertEquals(SyncState.PAUSED, engine.status().state)
            assertEquals(SyncState.PAUSED, m.ui.value.status.state)
        } finally {
            engine.stop()
            scope.cancel()
        }
    }

    @Test
    fun coming_back_to_the_foreground_last_leaves_the_engine_syncing() = runTest {
        val engine = fixtureEngine()
        val (m, scope) = queued(engine)
        try {
            m.resume()
            m.pause()
            m.resume()
            advanceUntilIdle()
            assertNotEquals(SyncState.PAUSED, engine.status().state)
            m.pause()
            advanceUntilIdle()
            assertEquals(SyncState.PAUSED, engine.status().state)
        } finally {
            engine.stop()
            scope.cancel()
        }
    }

    @Test
    fun a_task_lands_in_the_room_it_was_typed_in_even_if_another_opens_first() = runTest {
        val engine = fixtureEngine()
        val (m, scope) = queued(engine)
        try {
            val errands = engine.list("Errands")
            m.open(errands)
            advanceUntilIdle()
            m.add("Return the library books")
            // The owner switches rooms before the add has run.
            m.open(RoomKey.Smart(SmartList.MY_DAY))
            advanceUntilIdle()
            assertTrue(engine.room(errands, FIXTURE_NOW).tasks.any { it.title == "Return the library books" })
            assertFalse(m.ui.value.room.tasks.any { it.title == "Return the library books" })
        } finally {
            scope.cancel()
        }
    }

    @Test
    fun unpairing_asks_first_with_the_unsent_count_and_cancel_changes_nothing() = runTest {
        var unpaired = 0
        val m = TodoModel(fixtureEngine(), eager(), UnconfinedTestDispatcher(testScheduler), now = { FIXTURE_NOW }, onUnpair = { unpaired++ })
        m.add("Post the parcel")
        advanceUntilIdle()
        m.unpair()
        advanceUntilIdle()
        assertEquals(UnpairQuestion(unsent = 1), m.ui.value.unpairing)
        assertEquals(0, unpaired)

        m.cancelUnpair()
        advanceUntilIdle()
        assertNull(m.ui.value.unpairing)
        assertEquals(0, unpaired)
        assertEquals(1u, m.ui.value.status.pending)
    }

    @Test
    fun confirming_unpairs_once() = runTest {
        var unpaired = 0
        val m = TodoModel(fixtureEngine(), eager(), UnconfinedTestDispatcher(testScheduler), now = { FIXTURE_NOW }, onUnpair = { unpaired++ })
        m.unpair()
        advanceUntilIdle()
        assertEquals(UnpairQuestion(unsent = 0), m.ui.value.unpairing)
        m.confirmUnpair(0)
        advanceUntilIdle()
        assertEquals(1, unpaired)
        assertNull(m.ui.value.unpairing)
    }

    /**
     * An edit already handed to IO when the owner confirms: the confirmation waits for it, sees the count
     * has changed and asks again, and only the new count's confirmation unpairs.
     */
    @Test
    fun an_edit_in_flight_when_confirming_asks_again() = runTest {
        var unpaired = 0
        val d = StandardTestDispatcher(testScheduler)
        val scope = CoroutineScope(SupervisorJob() + d)
        try {
            val m = TodoModel(fixtureEngine(), scope, d, now = { FIXTURE_NOW }, onUnpair = { unpaired++ })
            m.unpair()
            advanceUntilIdle()
            assertEquals(UnpairQuestion(unsent = 0), m.ui.value.unpairing)

            m.add("Typed just before confirming")
            m.confirmUnpair(0) // Queued behind the add, which hasn't run yet.
            advanceUntilIdle()
            assertEquals(0, unpaired)
            assertEquals(UnpairQuestion(unsent = 1, changed = true), m.ui.value.unpairing)

            m.confirmUnpair(1)
            advanceUntilIdle()
            assertEquals(1, unpaired)
            assertNull(m.ui.value.unpairing)
        } finally {
            scope.cancel()
        }
    }

    /** The count changed some other way (another writer, a refusal from the server) while the question was open. */
    @Test
    fun a_count_that_changed_while_asking_asks_again() = runTest {
        var unpaired = 0
        val engine = fixtureEngine()
        val m = TodoModel(engine, eager(), UnconfinedTestDispatcher(testScheduler), now = { FIXTURE_NOW }, onUnpair = { unpaired++ })
        m.unpair()
        advanceUntilIdle()
        assertEquals(UnpairQuestion(unsent = 0), m.ui.value.unpairing)

        engine.addTask(RoomKey.Smart(SmartList.MY_DAY), "Written behind the model's back", FIXTURE_NOW)
        m.confirmUnpair(0)
        advanceUntilIdle()
        assertEquals(0, unpaired)
        assertEquals(UnpairQuestion(unsent = 1, changed = true), m.ui.value.unpairing)

        m.cancelUnpair()
        advanceUntilIdle()
        m.unpair()
        advanceUntilIdle()
        assertEquals("asking afresh clears the note", UnpairQuestion(unsent = 1), m.ui.value.unpairing)
        m.confirmUnpair(1)
        advanceUntilIdle()
        assertEquals(1, unpaired)
    }

    /** Queued and refused come from one status snapshot (the engine reads both under one lock). */
    @Test
    fun the_unsent_count_is_queued_plus_refused_from_one_snapshot() = runTest {
        assertEquals(5, unsentCount(SyncStatus(SyncState.OFFLINE, 2u, 3u, false, false, null, null)))
        val engine = fixtureEngine()
        val m = model(engine = engine)
        m.add("One")
        m.add("Two")
        m.unpair()
        advanceUntilIdle()
        val status = engine.status()
        assertEquals(unsentCount(status), m.ui.value.unpairing?.unsent)
        assertEquals(2, unsentCount(status))
    }

    @Test
    fun the_question_says_what_would_be_lost() {
        assertEquals("Unpair", unpairWording(0).second)
        assertTrue(unpairWording(0).first.contains("nothing is lost"))
        assertEquals("Unpair and discard", unpairWording(1).second)
        assertTrue(unpairWording(1).first.startsWith("1 change on this phone hasn’t reached the server"))
        assertTrue(unpairWording(3).first.startsWith("3 changes"))
        assertEquals("Unpair and discard", unpairWording(null).second)
    }

    /** The admin list and a task in it with steps, from the fixture, opened in a [Stepped] model. */
    private fun Stepped.openAdmin(): Pair<RoomKey, String> {
        val m = model
        val admin = m.engine.list("Admin")
        m.open(admin)
        drain()
        val id = m.ui.value.room.tasks.first { it.title == "Renew passport appointment" }.id
        m.select(id)
        drain()
        return admin to id
    }

    @Test
    fun task_actions_use_the_captured_task_and_offer_exact_undo() {
        val s = Stepped(fixtureEngine()) { time }
        val m = s.model
        val (admin, id) = s.openAdmin()
        val before = requireNotNull(m.ui.value.detail)
        val list = (admin as RoomKey.List).id
        val dest = (m.engine.list("Errands") as RoomKey.List).id
        val pending = m.ui.value.status.pending
        m.moveTaskTo(id, list, list) // Current destination: no command or toast.
        s.drain()
        assertEquals(pending, m.ui.value.status.pending)
        assertNull(m.ui.value.toast)
        m.moveTaskTo(id, list, dest)
        s.drain()
        assertFalse(m.ui.value.room.tasks.any { it.id == id })
        assertNull(m.ui.value.detail)
        val moved = requireNotNull(m.ui.value.toast)
        assertEquals("Moved “${before.title}” to Errands", moved.text)
        m.undo(moved)
        s.drain()
        assertTrue(m.ui.value.room.tasks.any { it.id == id })
        m.select(id)
        s.drain()
        m.deleteTask(id)
        m.undo(moved) // A stale toast doesn't undo the deletion.
        s.drain()
        assertNull(m.ui.value.detail)
        val deleted = requireNotNull(m.ui.value.toast)
        assertEquals("Deleted “${before.title}”", deleted.text)
        m.undo(deleted)
        m.undo(deleted)
        s.drain()
        assertEquals(before, m.engine.task(id, FIXTURE_NOW))
        assertEquals(pending, m.ui.value.status.pending)

        // Deleting from the row while another detail is open must not close that pane.
        m.add("Keep this detail open")
        s.drain()
        val other = m.ui.value.room.tasks.first().id
        m.select(other)
        s.drain()
        m.deleteTask(id)
        s.drain()
        assertEquals(other, m.ui.value.selected)
    }

    @Test
    fun trash_restore_keeps_the_original_list_and_undo_restores_the_same_deletion() {
        val s = Stepped(fixtureEngine("trash")) { time }
        val m = s.model
        m.open(RoomKey.Trash)
        s.drain()
        val room = m.ui.value.room
        val rows = requireNotNull(room.trash).rows
        assertEquals(app.todoornot.core.RoomSummary.None, room.summary)
        assertTrue(room.tasks.isEmpty())
        assertEquals(listOf("Return the paint samples", "Spring fair", "Flyers", "Send the first draft to print"), rows.map { it.title })
        assertEquals("Restore its list first.", rows.last().blocked)
        val first = rows.first()
        assertNull(m.engine.task(first.id, FIXTURE_NOW))
        m.restoreTrash(first)
        s.drain()
        assertEquals("Restored “${first.title}”", m.ui.value.toast?.text)
        assertEquals(rows.drop(1), m.ui.value.room.trash?.rows)
        val errands = m.ui.value.home.groups.flatMap { it.lists }.single { it.name == "Errands" }.key
        assertTrue(m.engine.room(errands, FIXTURE_NOW).tasks.any { it.id == first.id })
        m.undo(requireNotNull(m.ui.value.toast))
        s.drain()
        assertEquals(rows, m.ui.value.room.trash?.rows)
        assertNull(m.engine.task(first.id, FIXTURE_NOW))
    }

    @Test
    fun move_picker_filters_list_and_collection_words_and_ticks_the_current_list() {
        val s = Stepped(fixtureEngine()) { time }
        val m = s.model
        val (admin, id) = s.openAdmin()
        m.renameRoom(admin, "Úřady")
        s.drain()
        s.scope.launch {
            val groups = m.moveDestinations(id, "URAD  pers")
            assertEquals(listOf("Personal"), groups.map { it.heading })
            val match = groups.single().lists.single()
            assertEquals((admin as RoomKey.List).id, match.id)
            assertTrue(match.current)
            assertTrue(m.moveDestinations(id, "not-a-list").isEmpty())
        }.also { job ->
            s.drain()
            assertTrue(job.isCompleted && !job.isCancelled)
        }
    }

    @Test
    fun menu_reorders_open_rows_in_a_manual_list_and_undo_restores_their_order() {
        val s = Stepped(fixtureEngine()) { time }
        val m = s.model
        val (admin, _) = s.openAdmin()
        m.add("Mint")
        m.add("Parsley")
        s.drain()
        val room = m.ui.value.room
        val ids = room.tasks.map { it.id }
        val menu = taskMenu(room, ids.first(), false, false, 0).filterIsInstance<MenuEntry.Item>()
        assertFalse(menu.single { it.label == "Move up" }.enabled)
        val down = menu.single { it.label == "Move down" }.action as MenuAction.ReorderTask
        assertEquals((admin as RoomKey.List).id, down.list)
        m.reorderTask(down.list, ids.first(), requireNotNull(down.neighbour), down.by)
        s.drain()
        assertEquals(ids.take(2).reversed() + ids.drop(2), m.ui.value.room.tasks.map { it.id })
        m.undo(requireNotNull(m.ui.value.toast))
        s.drain()
        assertEquals(ids, m.ui.value.room.tasks.map { it.id })
        m.open(RoomKey.Smart(SmartList.ALL))
        s.drain()
        val pending = m.ui.value.status.pending
        m.reorderTask(down.list, ids.first(), requireNotNull(down.neighbour), down.by)
        s.drain()
        assertEquals(pending, m.ui.value.status.pending)
        val smart = taskMenu(m.ui.value.room, ids.first(), true, true, 3).filterIsInstance<MenuEntry.Item>()
        assertEquals(listOf("Reopen", "Remove from My Day", "Remove importance", "Priority…", "Move to…", "Delete task"), smart.map { it.label })
    }

    @Test
    fun task_menu_toggle_icons_show_current_state_and_priority_keeps_its_flag() {
        val s = Stepped(fixtureEngine()) { time }
        val (_, id) = s.openAdmin()
        val room = s.model.ui.value.room
        fun icons(myDay: Boolean, priority: Int) = taskMenu(room, id, false, myDay, priority)
            .filterIsInstance<MenuEntry.Item>().drop(1).take(3).map { it.icon }
        assertEquals(listOf("sun", "star", "flag"), icons(false, 2))
        assertEquals(listOf("sun-filled", "star", "flag"), icons(true, 2))
        assertEquals(listOf("sun", "star-filled", "flag"), icons(false, 3))
    }

    @Test
    fun due_dates_and_priorities_are_written_from_the_detail_pane() {
        val s = Stepped(fixtureEngine()) { time }
        val m = s.model
        val (_, id) = s.openAdmin()
        assertEquals("2026-11-21", m.ui.value.today)
        m.setDue(id, "2026-12-24")
        s.drain()
        assertEquals("2026-12-24", m.ui.value.detail?.due)
        m.setDue(id, null)
        s.drain()
        assertNull(m.ui.value.detail?.due)
        for (marks in listOf(1, 2, 3, 0)) {
            m.setPriority(id, marks)
            s.drain()
            assertEquals(marks, m.ui.value.detail?.priority?.toInt())
        }
        // A date that isn't one is refused in plain words, and nothing is queued.
        val pending = m.ui.value.status.pending
        m.setDue(id, "next week")
        s.drain()
        assertEquals("That isn't a date.", m.ui.value.toast?.text)
        assertEquals(pending, m.ui.value.status.pending)
    }

    @Test
    fun steps_are_renamed_moved_and_deleted_and_undo_brings_one_back() {
        val s = Stepped(fixtureEngine()) { time }
        val m = s.model
        val (_, id) = s.openAdmin()
        fun titles() = m.ui.value.detail!!.steps.map { it.title }
        val before = m.ui.value.detail!!.steps
        assertTrue("the fixture task has steps", before.size >= 2)
        val (first, second) = before.take(2)
        m.renameStep(first.id, "  Find the old passport  ")
        s.drain()
        assertEquals("Find the old passport", titles()[0])
        m.moveStep(second.id, -1)
        s.drain()
        assertEquals(second.title, titles()[0])

        m.deleteStep(second.id)
        s.drain()
        assertFalse(second.title in titles())
        val toast = m.ui.value.toast!!
        assertEquals("Deleted step “${second.title}”", toast.text)
        assertTrue(toast.undoable)
        assertNotNull(toast.deletion)
        m.undo(toast)
        m.undo(toast) // A second tap on the same toast does nothing.
        s.drain()
        assertNull(m.ui.value.toast)
        assertEquals(listOf(second.title, "Find the old passport") + before.drop(2).map { it.title }, titles())
        assertEquals(second.done, m.ui.value.detail!!.steps[0].done)
        assertEquals(id, m.ui.value.detail?.id)
    }

    @Test
    fun a_deleted_step_whose_task_went_meanwhile_says_why_undo_shows_nothing() {
        val s = Stepped(fixtureEngine()) { time }
        val m = s.model
        val (_, id) = s.openAdmin()
        val step = m.ui.value.detail!!.steps.first()
        m.deleteStep(step.id)
        s.drain()
        val toast = m.ui.value.toast!!
        m.engine.fixtureDeleteTask(id)
        m.undo(toast)
        s.drain()
        assertEquals("Step restored, but its task was deleted elsewhere.", m.ui.value.toast?.text)
        assertFalse(m.ui.value.toast!!.undoable)
    }

    @Test
    fun lists_and_collections_are_renamed_and_moved() {
        val s = Stepped(fixtureEngine()) { time }
        val m = s.model
        s.drain()
        val personal = m.engine.collection("Personal")
        val errands = m.engine.list("Errands")
        m.renameRoom(errands, "Shopping")
        m.renameRoom(personal, "Home")
        s.drain()
        val home = m.ui.value.home
        val group = home.groups.first { it.collection?.key == personal }
        assertEquals("Home", group.collection?.name)
        assertTrue(group.lists.any { it.key == errands && it.name == "Shopping" })
        // A blank name is refused, and says why.
        m.renameRoom(errands, "  ")
        s.drain()
        assertEquals("A list needs a name.", m.ui.value.toast?.text)

        val before = group.lists.map { it.key }
        val i = before.indexOf(errands)
        val by = if (i > 0) -1 else 1
        m.moveRoom(errands, by)
        s.drain()
        val after = m.ui.value.home.groups.first { it.collection?.key == personal }.lists.map { it.key }
        assertEquals(i + by, after.indexOf(errands))
        val collections = { m.ui.value.home.groups.mapNotNull { it.collection?.key } }
        val at = collections().indexOf(personal)
        val step = if (at > 0) -1 else 1
        m.moveRoom(personal, step)
        s.drain()
        assertEquals(at + step, collections().indexOf(personal))
    }

    @Test
    fun deleting_the_open_list_goes_to_my_day_and_undo_brings_it_and_its_tasks_back() {
        val s = Stepped(fixtureEngine()) { time }
        val m = s.model
        val (admin, _) = s.openAdmin()
        val id = (admin as RoomKey.List).id
        val tasks = m.ui.value.room.tasks.map { it.id }
        val question = m.deleteListQuestion(id)!!
        assertTrue(question, question.startsWith("Delete the list “Admin” and its "))
        val generation = m.ui.value.generation
        val results = mutableListOf<String?>()
        m.deleteList(id, question) { results += it }
        s.drain()
        assertEquals(listOf<String?>(null), results)
        val ui = m.ui.value
        assertEquals(MY_DAY, ui.room.key)
        assertTrue("a new room generation", ui.generation > generation)
        assertNull("the open task went with its list", ui.detail)
        assertTrue(ui.home.groups.flatMap { it.lists }.none { it.key == admin })
        assertNull(m.deleteListQuestion(id))
        val toast = ui.toast!!
        assertEquals("Deleted list “Admin”", toast.text)
        m.undo(toast)
        s.drain()
        assertTrue(m.ui.value.home.groups.flatMap { it.lists }.any { it.key == admin })
        assertEquals(MY_DAY, m.ui.value.room.key)
        m.open(admin)
        s.drain()
        assertEquals(tasks, m.ui.value.room.tasks.map { it.id })
    }

    @Test
    fun deleting_another_list_keeps_the_room() = runTest {
        val m = model()
        val errands = m.engine.list("Errands") as RoomKey.List
        m.deleteList(errands.id, m.deleteListQuestion(errands.id)!!) {}
        advanceUntilIdle()
        assertEquals(MY_DAY, m.ui.value.room.key)
        assertEquals("Deleted list “Errands”", m.ui.value.toast?.text)
        assertEquals(0L, m.ui.value.generation)
    }

    @Test
    fun the_toast_offers_undo_only_for_what_it_can_reverse() {
        assertFalse(app.todoornot.model.Toast(1, "Saved").undoable)
        assertEquals("2026-11-21", app.todoornot.model.isoDay(FIXTURE_NOW))
        // Just before midnight in UTC is already the next day an hour east.
        val late = LocalNow(java.time.Instant.parse("2026-11-21T23:30:00Z").toEpochMilli(), 3600, "Europe/Prague")
        assertEquals("2026-11-22", app.todoornot.model.isoDay(late))
        assertEquals("2026-11-21", app.todoornot.model.isoDay(late.copy(utcOffsetSecs = 0, zoneId = "UTC")))
    }

    @Test
    fun a_delete_question_gone_stale_asks_again_and_deletes_nothing() {
        val s = Stepped(fixtureEngine()) { time }
        val m = s.model
        s.drain()
        val errands = m.engine.list("Errands") as RoomKey.List
        val asked = m.deleteListQuestion(errands.id)!!
        // A task arrives in the list while the question is open (a sync, say).
        m.engine.addTask(errands, "Buy stamps", FIXTURE_NOW)
        val results = mutableListOf<String?>()
        m.deleteList(errands.id, asked) { results += it }
        s.drain()
        val again = results.single()!!
        assertNotEquals(asked, again)
        assertTrue(again, again.startsWith("Delete the list “Errands” and its "))
        assertTrue("nothing deleted", m.ui.value.home.groups.flatMap { it.lists }.any { it.key == errands })
        assertNull(m.ui.value.toast)
        // Confirmed as it reads now: deleted, with Undo.
        m.deleteList(errands.id, again) { results += it }
        s.drain()
        assertEquals(null, results.last())
        assertTrue(m.ui.value.toast!!.undoable)
        // Gone by the time it's confirmed: nothing to delete, and the owner is told.
        m.deleteList(errands.id, again) { results += it }
        s.drain()
        assertEquals(null, results.last())
        assertEquals("That list was deleted elsewhere.", m.ui.value.toast?.text)
    }

    @Test
    fun reminders_are_set_moved_and_removed_from_the_detail_pane_with_undo() {
        val s = Stepped(fixtureEngine()) { time }
        val m = s.model
        val (_, id) = s.openAdmin()
        fun row() = m.ui.value.detail!!.fields.first { it.edit == FieldEdit.REMINDER }
        fun seen() = m.ui.value.detail?.reminder
        val before = requireNotNull(seen())
        assertEquals("Today, 18:00", row().value)
        // Tomorrow at 15:00 local (UTC+1).
        val at = java.time.Instant.parse("2026-11-22T14:00:00Z").toEpochMilli()
        m.setReminder(id, before, at)
        s.drain()
        assertEquals(at, seen()?.atMs)
        assertEquals(before.id, seen()?.id)
        assertEquals("Tomorrow, 15:00", row().value)
        val moved = m.ui.value.toast!!
        assertEquals("Reminder set for Tomorrow, 15:00", moved.text)
        assertTrue(moved.undoable)
        m.undo(moved)
        m.undo(moved) // A second tap on the same toast does nothing.
        s.drain()
        assertEquals(before, seen())
        assertNull(m.ui.value.toast)

        m.setReminder(id, seen(), null)
        s.drain()
        assertNull(seen())
        assertEquals("–", row().value)
        assertTrue(row().empty)
        val removed = m.ui.value.toast!!
        assertEquals("Reminder removed", removed.text)
        m.undo(removed)
        s.drain()
        assertEquals(before, seen())

        // A time that has passed is refused in plain words, and nothing is queued.
        val pending = m.ui.value.status.pending
        m.setReminder(id, seen(), FIXTURE_NOW.epochMs - 60_000)
        s.drain()
        assertEquals("Pick a time that hasn’t passed.", m.ui.value.toast?.text)
        assertFalse(m.ui.value.toast!!.undoable)
        assertEquals(pending, m.ui.value.status.pending)
    }

    @Test
    fun a_picker_left_open_while_the_reminder_changed_writes_nothing_and_says_so() {
        val s = Stepped(fixtureEngine()) { time }
        val m = s.model
        val (_, id) = s.openAdmin()
        // The picker opens on the reminder; another device moves it meanwhile.
        val opened = requireNotNull(m.ui.value.detail?.reminder)
        val elsewhere = opened.atMs + 3_600_000
        m.engine.setReminder(id, opened, elsewhere, FIXTURE_NOW)
        s.changed()
        s.drain()
        val pending = m.ui.value.status.pending
        m.setReminder(id, opened, opened.atMs + 7_200_000)
        s.drain()
        assertEquals("The reminder changed while the picker was open. Check it, then try again.", m.ui.value.toast?.text)
        assertEquals(pending, m.ui.value.status.pending)
        assertEquals(elsewhere, m.ui.value.detail?.reminder?.atMs)
        // Removing it from the stale picker is refused the same way.
        m.setReminder(id, opened, null)
        s.drain()
        assertEquals(pending, m.ui.value.status.pending)
        assertEquals(elsewhere, m.ui.value.detail?.reminder?.atMs)
    }

    @Test
    fun completing_from_a_reminder_goes_through_the_apps_command() {
        val s = Stepped(fixtureEngine()) { time }
        val m = s.model
        val (_, id) = s.openAdmin()
        val pending = m.ui.value.status.pending
        assertTrue(m.engine.reminderAlarms(0, FIXTURE_NOW).any { it.task == id })
        var done: Boolean? = null
        var saved = 0
        s.scope.launch { done = m.completeFromNotification(id, syncFor = 0) { saved++ } }
        s.drain()
        assertEquals(true, done)
        assertEquals("the notification may go once it is saved", 1, saved)
        assertEquals(true, m.engine.taskDone(id))
        assertEquals(true, m.ui.value.detail?.done)
        assertEquals(pending + 1u, m.ui.value.status.pending)
        // No toast to undo it: the notification is the owner's only view of it.
        assertNull(m.ui.value.toast)
        // A done task alerts for nothing.
        assertTrue(m.engine.reminderAlarms(0, FIXTURE_NOW).none { it.task == id })
        // Completing it again (a second tap on a stale notification) changes nothing.
        s.scope.launch { done = m.completeFromNotification(id, syncFor = 0) { saved++ } }
        s.drain()
        assertEquals(pending + 1u, m.ui.value.status.pending)
        // A completion that can't be saved keeps the notification.
        s.scope.launch { done = m.completeFromNotification("not a task", syncFor = 0) { saved++ } }
        s.drain()
        assertEquals(false, done)
        assertEquals(2, saved)
    }

    @Test
    fun the_reminder_row_reads_in_the_phones_zone_across_a_change_of_the_clocks() {
        // Prague, in winter (UTC+1); the reminder is after the clocks go forward on 28 March 2027 (UTC+2).
        val prague = java.time.ZoneId.of("Europe/Prague")
        val s = Stepped(fixtureEngine(), zone = prague) { time }
        val m = s.model
        val (_, id) = s.openAdmin()
        val nine = java.time.LocalDateTime.of(2027, 4, 1, 9, 0).atZone(prague).toInstant().toEpochMilli()
        m.setReminder(id, m.ui.value.detail?.reminder, nine)
        s.drain()
        val row = m.ui.value.detail!!.fields.first { it.edit == FieldEdit.REMINDER }
        // Not 08:00, as the phone's offset today would put it.
        assertEquals("Thu 1 Apr 2027, 09:00", row.value)
        assertEquals("Reminder set for Thu 1 Apr 2027, 09:00", m.ui.value.toast?.text)
        // A reminder before the change reads in winter time, as before.
        val dec = java.time.LocalDateTime.of(2026, 12, 1, 9, 0).atZone(prague).toInstant().toEpochMilli()
        m.setReminder(id, m.ui.value.detail?.reminder, dec)
        s.drain()
        assertEquals("Tue 1 Dec, 09:00", m.ui.value.detail!!.fields.first { it.edit == FieldEdit.REMINDER }.value)
    }
}
