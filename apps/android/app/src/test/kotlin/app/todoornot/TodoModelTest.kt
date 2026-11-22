package app.todoornot

import app.todoornot.core.RoomKey
import app.todoornot.core.SmartList
import app.todoornot.core.SyncState
import app.todoornot.core.SyncStatus
import app.todoornot.core.TodoEngine
import app.todoornot.design.Tokens
import app.todoornot.model.DuskLook
import app.todoornot.model.Pace
import app.todoornot.model.RowLook
import app.todoornot.model.TodoModel
import app.todoornot.model.Wake
import app.todoornot.model.ease
import app.todoornot.model.rowIn
import app.todoornot.model.Section
import app.todoornot.model.UnpairQuestion
import app.todoornot.model.unsentCount
import app.todoornot.ui.unpairWording
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
        assertEquals(task.id, toast.reopen)
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
    private class Stepped(engine: TodoEngine, clock: () -> Long) {
        val main = Steps()
        val io = Steps()
        val scope = CoroutineScope(SupervisorJob() + main)
        private var onChange: () -> Unit = {}
        val model = TodoModel(engine, scope, io, now = { FIXTURE_NOW }, clock = clock, listen = { onChange = it })

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
     * An agent's task is completed in Agent activity and the tap reversed: both written, the owner is now
     * its last editor and the room no longer has it. Once the settle ends, it collapses out (never
     * snapping), whether the reversal's write landed [before] or after the settle ended, with no change
     * announced by the engine.
     */
    private fun aReversedAgentRowLeaves(before: Boolean) {
        val engine = fixtureEngine()
        val s = Stepped(engine) { time }
        val m = s.model
        s.drain()
        val agents = RoomKey.Smart(SmartList.AGENT_ACTIVITY)
        m.open(agents)
        s.drain()
        val order = m.ui.value.room.tasks.map { it.id }
        val row = m.ui.value.room.tasks.drop(1).first { !it.done }
        m.setDone(row.id, true)
        s.drain()
        m.frame(EXIT_AT - 100)
        m.setDone(row.id, false)
        var write: Runnable? = null
        if (before) {
            s.drain()
        } else {
            s.runMain()
            write = s.io.hold()
            s.drain()
        }
        fun inPlace() = assertEquals(order, m.ui.value.room.tasks.map { it.id })
        inPlace()
        // The settle ends.
        m.frame(EXIT_AT + 5_000)
        val settled = time
        s.drain()
        if (!before) {
            // Its write is still to land: the row stays, at rest, however long that takes.
            assertEquals(true, engine.taskDone(row.id))
            inPlace()
            assertEquals(RowLook.rest(false), m.motion.rowIn(row.id, Section.ROWS))
            m.frame(time + 5_000)
            s.drain()
            inPlace()
            assertEquals(RowLook.rest(false), m.motion.rowIn(row.id, Section.ROWS))
            write!!.run()
            s.drain()
        }
        assertEquals(false, engine.taskDone(row.id))
        assertFalse("the owner is its last editor", engine.room(agents, FIXTURE_NOW).tasks.any { it.id == row.id })
        // It collapses out where it is, from its full height.
        val start = if (before) settled else time
        inPlace()
        assertEquals(1f, m.motion.rowIn(row.id, Section.ROWS)!!.height)
        m.frame(start + M.ROW_COLLAPSE / 2)
        s.drain()
        inPlace()
        val half = m.motion.rowIn(row.id, Section.ROWS)!!
        assertEquals(1f - M.EASE_EXIT.ease((M.ROW_COLLAPSE / 2).toFloat() / M.ROW_COLLAPSE), half.height, 1e-4f)
        // Gone: nothing of it shows, and then the list no longer has it.
        m.frame(start + M.ROW_COLLAPSE)
        assertTrue(m.motion.rowIn(row.id, Section.ROWS)!!.gone)
        s.drain()
        assertEquals(order - row.id, m.ui.value.room.tasks.map { it.id })
        assertNull(m.motion.row(row.id))
        assertEquals(Wake.Idle, m.motion.motion.wake(time))
    }

    @Test
    fun a_reversed_agent_row_leaves_agent_activity_when_its_write_landed_before_the_settle_ended() =
        aReversedAgentRowLeaves(before = true)

    @Test
    fun a_reversed_agent_row_leaves_agent_activity_when_its_write_lands_after_the_settle_ended() =
        aReversedAgentRowLeaves(before = false)

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
        engine.deleteTask(id)
        s.refreshLater()
        s.main.next()
        s.io.next()
        // Restored (the engine says so), and the owner reverses the completion before that read is
        // published. The reversal's write waits.
        engine.restoreTask(id)
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
    fun completing_in_agent_activity_keeps_the_agents_row_in_place_until_released() = runTest {
        val engine = fixtureEngine()
        val m = model(engine = engine)
        val agents = RoomKey.Smart(SmartList.AGENT_ACTIVITY)
        m.open(agents)
        advanceUntilIdle()
        val before = m.ui.value.room.tasks.map { it.id }
        // An open task an agent changed, below the latest change (a toggle makes it the latest).
        val row = m.ui.value.room.tasks.drop(1).first { !it.done }
        assertNotNull(row.agent)
        m.setDone(row.id, true)
        advanceUntilIdle()
        // Written, and so the owner's change: the room no longer selects it.
        assertEquals(true, engine.taskDone(row.id))
        assertFalse(engine.room(agents, FIXTURE_NOW).tasks.any { it.id == row.id })
        // But read again while it plays, the row stays where it was, open, with its agent.
        assertEquals(before, m.ui.value.room.tasks.map { it.id })
        m.frame(EXIT_AT + M.ROW_COLLAPSE / 2)
        m.refresh()
        assertEquals(before, m.ui.value.room.tasks.map { it.id })
        val kept = m.ui.value.room.tasks.first { it.id == row.id }
        assertEquals(row.agent, kept.agent)
        assertFalse(kept.done)
        val look = m.motion.rowIn(row.id, Section.ROWS)!!
        assertTrue(look.height > 0f && look.height < 1f)
        // Released: it leaves, and the motion comes to rest.
        m.frame(RELEASE_AT)
        advanceUntilIdle()
        assertEquals(before - row.id, m.ui.value.room.tasks.map { it.id })
        m.frame(time + 5_000)
        assertNull(m.motion.row(row.id))
        assertEquals(Wake.Idle, m.motion.motion.wake(time))
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
        assertEquals(5, unsentCount(SyncStatus(SyncState.OFFLINE, 2u, 3u, null, null)))
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
}
