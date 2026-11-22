package app.todoornot

import app.todoornot.core.RoomKey
import app.todoornot.core.SmartList
import app.todoornot.core.SyncState
import app.todoornot.model.TodoModel
import app.todoornot.widget.WidgetData
import app.todoornot.widget.WidgetEdit
import app.todoornot.widget.widgetChoices
import app.todoornot.widget.widgetData
import app.todoornot.widget.widgetFit
import app.todoornot.widget.widgetKey
import app.todoornot.widget.widgetRoom
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

    @Test
    fun resize_reserves_overflow_only_when_needed_and_honours_font_scale() {
        assertEquals(1, widgetFit(132f, 1f, 9).visible)
        assertEquals(8, widgetFit(132f, 1f, 9).more)
        assertEquals(2, widgetFit(144f, 1f, 2).visible)
        assertEquals(0, widgetFit(144f, 1f, 2).more)
        assertTrue(widgetFit(300f, 1.5f, 100).visible < widgetFit(300f, 1f, 100).visible)
    }
}
