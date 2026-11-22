package app.todoornot

import app.todoornot.core.RoomKey
import app.todoornot.core.SmartList
import app.todoornot.core.SyncState
import app.todoornot.core.SyncStatus
import app.todoornot.core.TodoEngine
import app.todoornot.model.TodoModel
import app.todoornot.model.UnpairQuestion
import app.todoornot.model.unsentCount
import app.todoornot.ui.unpairWording
import kotlinx.coroutines.CoroutineScope
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
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/** The main screen's model over the real Rust engine (host build, through JNA) with fixture data. */
@OptIn(ExperimentalCoroutinesApi::class)
class TodoModelTest {
    /** Runs every model coroutine eagerly, so each call has taken effect when it returns. */
    private fun TestScope.eager() = CoroutineScope(SupervisorJob() + UnconfinedTestDispatcher(testScheduler))

    private fun TestScope.model(name: String = "world", engine: TodoEngine = fixtureEngine(name)) =
        TodoModel(engine, eager(), UnconfinedTestDispatcher(testScheduler), now = { FIXTURE_NOW })

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
    fun completing_shows_an_undo_that_reopens() = runTest {
        val m = model()
        val task = m.ui.value.room.tasks.first { it.title == "Koupit rohlíky a mléko" }
        m.setDone(task.id, true)
        advanceUntilIdle()
        val toast = m.ui.value.toast!!
        assertEquals(task.id, toast.reopen)
        assertFalse(m.ui.value.room.tasks.any { it.id == task.id })
        assertEquals(4u, m.ui.value.room.done)

        m.undo(toast)
        advanceUntilIdle()
        assertNull(m.ui.value.toast)
        assertTrue(m.ui.value.room.tasks.any { it.id == task.id })
        assertEquals(3u, m.ui.value.room.done)
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
