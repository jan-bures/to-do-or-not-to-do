package app.todoornot

import app.todoornot.core.ProblemKind
import app.todoornot.core.RoomKey
import app.todoornot.core.SmartList
import app.todoornot.core.SyncState
import app.todoornot.design.Tokens
import app.todoornot.model.TodoModel
import app.todoornot.widget.WIDGET_ROW_LIMIT
import app.todoornot.widget.WidgetData
import app.todoornot.widget.WidgetEdit
import app.todoornot.widget.syncForWidget
import app.todoornot.widget.widgetChoices
import app.todoornot.widget.widgetData
import app.todoornot.widget.widgetKey
import app.todoornot.widget.widgetRoom
import app.todoornot.widget.widgetRowHeight
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.launch
import kotlinx.coroutines.test.advanceTimeBy
import kotlinx.coroutines.test.runCurrent
import kotlinx.coroutines.test.runTest
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class WidgetModelTest {
    @Test
    fun choices_follow_drawer_order_and_round_trip_by_id() = fixtureEngine().use { engine ->
        val home = engine.home(FIXTURE_NOW)
        val choices = widgetChoices(home)
        assertEquals(listOf("My Day", "Important", "Planned", "All"), choices.take(4).map { it.item.name })
        assertEquals(home.groups.flatMap { listOfNotNull(it.collection) + it.lists }, choices.drop(4).map { it.item })
        assertEquals(home.groups.flatMap { group -> group.lists.filter { group.collection != null } }, choices.filter { it.indented }.map { it.item })
        choices.forEach { assertEquals(it.item.key, widgetRoom(it.item.key.widgetKey())) }
        assertNull(widgetRoom("smart:${SmartList.COMPLETED.name}"))
        assertNull(widgetRoom("list:"))
        assertNull(widgetRoom("not a room"))
    }

    @Test
    fun collection_preserves_order_due_and_list_labels_and_never_shows_done_rows() = fixtureEngine().use { engine ->
        val choice = engine.collection("Personal")
        val room = engine.room(choice, FIXTURE_NOW)
        val widget = widgetData(engine, choice, FIXTURE_NOW) as WidgetData.Room
        assertEquals(room.tasks.map { it.id }, widget.rows.map { it.id })
        assertEquals(room.tasks.map { it.due }, widget.rows.map { it.due })
        assertEquals(room.tasks.map { it.chip }, widget.rows.map { it.list })
        assertTrue(widget.rows.all { it.list != null })
        assertTrue(widget.rows.none { row -> room.completed.any { it.id == row.id } })
        assertEquals(widgetChoices(engine.home(FIXTURE_NOW)).first { it.item.key == choice }.item, widget.item)
    }

    @Test
    fun missing_rooms_do_not_fall_back_and_empty_uses_shared_copy() = fixtureEngine().use { engine ->
        val reading = engine.list("Reading list") as RoomKey.List
        val empty = widgetData(engine, reading, FIXTURE_NOW) as WidgetData.Room
        assertEquals(engine.room(reading, FIXTURE_NOW).emptyLine, empty.empty)
        assertTrue(empty.rows.isEmpty())
        engine.deleteList(reading.id, engine.deleteListQuestion(reading.id)!!)
        assertEquals(WidgetData.Missing(false), widgetData(engine, reading, FIXTURE_NOW))
        assertEquals(WidgetData.Missing(true), widgetData(engine, RoomKey.Collection(reading.id), FIXTURE_NOW))
        assertEquals(WidgetData.Unconfigured, widgetData(engine, null, FIXTURE_NOW))
    }

    @Test
    fun actions_write_through_the_model_and_repeated_delivery_is_idempotent() = runTest {
        fixtureEngine().use { engine ->
            val room = RoomKey.Smart(SmartList.ALL)
            val widget = widgetData(engine, room, FIXTURE_NOW) as WidgetData.Room
            val task = widget.rows.first { !it.important }.id
            val model = TodoModel(engine, backgroundScope, Dispatchers.Unconfined, now = { FIXTURE_NOW })
            suspend fun edit(action: WidgetEdit) {
                assertTrue(model.applyExternalTaskEdit { action.apply(it, FIXTURE_NOW) })
            }
            val pending = engine.status().pending
            edit(WidgetEdit.Important(task, true))
            edit(WidgetEdit.Important(task, true))
            assertEquals(pending + 1u, engine.status().pending)
            assertTrue((widgetData(engine, room, FIXTURE_NOW) as WidgetData.Room).rows.first { it.id == task }.important)
            edit(WidgetEdit.Important(task, false))
            assertFalse((widgetData(engine, room, FIXTURE_NOW) as WidgetData.Room).rows.first { it.id == task }.important)
            edit(WidgetEdit.Complete(task))
            edit(WidgetEdit.Complete(task))
            assertEquals(pending + 3u, engine.status().pending)
            assertTrue(engine.task(task, FIXTURE_NOW)!!.done)
            assertTrue((widgetData(engine, room, FIXTURE_NOW) as WidgetData.Room).rows.none { it.id == task })
        }
    }

    @OptIn(ExperimentalCoroutinesApi::class)
    @Test
    fun widget_and_reminder_sync_do_not_stop_each_other() = runTest {
        fixtureEngine().use { engine ->
            val model = TodoModel(engine, backgroundScope, Dispatchers.Unconfined, now = { FIXTURE_NOW })
            val longer = launch { model.syncInBackground(2_000) { false } }
            runCurrent()
            val shorter = launch { model.syncInBackground(500) { false } }
            advanceTimeBy(600)
            assertTrue(shorter.isCompleted)
            assertNotEquals(SyncState.PAUSED, engine.status().state)
            longer.cancel()
            runCurrent()
            assertEquals(SyncState.PAUSED, engine.status().state)
        }
    }

    @OptIn(ExperimentalCoroutinesApi::class)
    @Test
    fun background_refresh_waits_for_delayed_reconnect_and_pending_edits_despite_a_stale_send_failure() = runTest {
        fixtureEngine().use { engine ->
            var status = engine.status().copy(state = SyncState.OFFLINE, problem = ProblemKind.NETWORK, pending = 1u)
            val model = TodoModel(engine, backgroundScope, Dispatchers.Unconfined, syncStatus = { status })
            val sync = launch { model.syncForWidget() }
            advanceTimeBy(800) // Reconnect didn't answer on the first poll.
            assertTrue(sync.isActive)
            status = status.copy(state = SyncState.ONLINE, problem = null, caughtUp = true)
            advanceTimeBy(400)
            assertTrue("online but the edit still hasn't been acknowledged", sync.isActive)
            status = status.copy(pending = 0u)
            advanceTimeBy(400)
            assertTrue(sync.isCompleted)
            assertEquals(SyncState.PAUSED, engine.status().state)
        }
    }

    @OptIn(ExperimentalCoroutinesApi::class)
    @Test
    fun online_with_an_empty_outbox_waits_for_delayed_frames_or_the_full_budget() = runTest {
        fixtureEngine().use { engine ->
            var status = engine.status().copy(state = SyncState.ONLINE)
            val model = TodoModel(engine, backgroundScope, Dispatchers.Unconfined, syncStatus = { status })
            val sync = launch { model.syncForWidget() }
            advanceTimeBy(1_300)
            assertTrue("headers arrived but the backlog has not", sync.isActive)
            status = status.copy(caughtUp = true)
            advanceTimeBy(300)
            assertTrue("backlog arrived", sync.isCompleted)

            status = status.copy(caughtUp = false)
            val noFrames = launch { model.syncForWidget() }
            advanceTimeBy(3_999)
            assertTrue(noFrames.isActive)
            advanceTimeBy(2)
            assertTrue(noFrames.isCompleted)
        }
    }

    @Test
    fun rows_honour_font_scale_and_stay_on_the_grid() {
        val type = Tokens.Type
        assertEquals(type.TASK_TITLE.lineHeight + type.COUNT.lineHeight + Tokens.Space.S2, widgetRowHeight(1f))
        // A title-only row uses the compact 40dp target, so a 4x3 widget fits five of them under the header.
        assertEquals(Tokens.Space.S10, widgetRowHeight(1f, hasMeta = false))
        assertTrue(widgetRowHeight(1f, hasMeta = false) < widgetRowHeight(1f))
        assertTrue(widgetRowHeight(1.5f) > widgetRowHeight(1f))
        assertEquals(0f, widgetRowHeight(1.5f) % Tokens.Space.S1)
    }

    @Test
    fun large_rooms_keep_the_first_open_rows_in_order_up_to_the_widget_limit() = fixtureEngine().use { engine ->
        val choice = engine.list("Reading list")
        repeat(WIDGET_ROW_LIMIT + 2) { index -> engine.addTask(choice, "Task ${index + 1}", FIXTURE_NOW) }
        val ids = engine.room(choice, FIXTURE_NOW).tasks.map { it.id }
        engine.setDone(ids[1], true)
        val widget = widgetData(engine, choice, FIXTURE_NOW) as WidgetData.Room
        assertEquals(ids.filterNot { it == ids[1] }.take(WIDGET_ROW_LIMIT), widget.rows.map { it.id })
    }
}
