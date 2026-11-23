package app.todoornot

import app.todoornot.core.ListFeaturesPatch
import app.todoornot.core.ListView
import app.todoornot.core.QuickAddDraft
import app.todoornot.core.RoomKey
import app.todoornot.core.SmartList
import app.todoornot.core.TimelineBar
import app.todoornot.core.TimelineEdit
import app.todoornot.core.TimelineZoom
import app.todoornot.core.WorkflowQuickAddDraft
import app.todoornot.core.timelineColumns
import app.todoornot.core.timelinePreview
import app.todoornot.core.timelineScale
import app.todoornot.model.TodoModel
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class TimelineModelTest {
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Unconfined)
    private val engine = fixtureEngine("blank")
    private val planned = RoomKey.Smart(SmartList.PLANNED)
    private val collection = engine.createCollection("Projects")
    private val list = engine.createList("Project", collection) as RoomKey.List
    private val other = engine.createList("Reference", collection) as RoomKey.List
    private val now = FIXTURE_NOW

    @After fun close() { scope.cancel(); engine.close() }

    private fun enableTimeline() = engine.setListFeatures(list.id, ListFeaturesPatch(null, true, ListView.TIMELINE))
    private fun model() = TodoModel(engine, scope, Dispatchers.Unconfined, now = { now }, start = list)
    private fun add(title: String, due: String? = null, start: String? = null, room: RoomKey = list): String =
        engine.addWorkflowTask(room, WorkflowQuickAddDraft(QuickAddDraft(title, due, false, null), start, null), now)

    @Test fun projectionsRespectRoomMembershipFilterAndCompletedDisclosure() {
        assertFalse(engine.timelineAvailable(collection))
        enableTimeline()
        val startOnly = add("Start design", start = "2026-11-22")
        val dueOnly = add("Buy hardware", due = "2026-11-23", room = other)
        val undated = add("Collect samples")
        val datedDone = add("Submit permits", due = "2026-11-21")
        engine.setDone(datedDone, true)
        engine.setDone(add("Completed without dates"), true)
        val model = model()
        val own = model.read()
        assertEquals(ListView.TIMELINE, own.view)
        assertEquals(listOf(startOnly), own.timeline!!.lanes.map { it.taskId })
        assertEquals(listOf(undated), own.timeline.unscheduled!!.map { it.taskId })
        model.setShowCompleted(true)
        assertEquals(setOf(startOnly, datedDone), model.read().timeline!!.lanes.map { it.taskId }.toSet())

        model.open(collection)
        assertEquals(ListView.LIST, model.read().view)
        model.setRoomView(ListView.TIMELINE, model.read().generation)
        val cross = model.read().timeline!!
        assertEquals(setOf(startOnly, dueOnly), cross.lanes.map { it.taskId }.toSet())
        assertEquals("Reference", cross.lanes.single { it.taskId == dueOnly }.chip?.label)

        model.open(planned)
        assertEquals(listOf(dueOnly), model.read().room.tasks.map { it.id })
        model.setRoomView(ListView.TIMELINE, model.read().generation)
        val dated = model.read().timeline!!
        assertNull(dated.unscheduled)
        assertEquals(listOf(startOnly, dueOnly), dated.lanes.map { it.taskId })
        assertEquals(TimelineBar.OPEN_ENDED, dated.lanes[0].span?.bar)
        assertEquals(TimelineBar.MILESTONE, dated.lanes[1].span?.bar)
        assertEquals("23.11.", dated.lanes[1].span?.label)
        model.setFilter("design")
        assertEquals(listOf(startOnly), model.read().timeline!!.lanes.map { it.taskId })
    }

    @Test fun previewAndTrackedWriteShareInclusiveDateRulesAndGuardStaleGestures() {
        enableTimeline()
        val range = add("Install shelving", due = "2026-11-24", start = "2026-11-20")
        val milestone = add("Pick a finish", due = "2026-11-23")
        val openEnd = add("Prepare supplies", start = "2026-11-22")
        val unscheduled = add("Check stock")
        val model = model()
        fun lane(id: String) = model.read().timeline!!.lanes.single { it.taskId == id }
        val original = lane(range)
        val generation = model.read().generation
        val move = TimelineEdit.Move(2)
        val next = timelinePreview(original.dates, move, now)!!
        assertEquals(1, next.first)
        assertEquals(5, next.last)
        model.scheduleTimeline(original, move, "2026-11-21", generation)
        assertEquals(next, lane(range).span)
        model.scheduleTimeline(original, TimelineEdit.DueEdge(-1), "2026-11-21", generation)
        assertEquals(next, lane(range).span)
        assertFalse(model.read().toast!!.undoable)
        model.scheduleTimeline(lane(range), TimelineEdit.DueEdge(-100), "2026-11-21", generation)
        assertEquals(lane(range).dates.start, lane(range).dates.due)
        model.undo(model.read().toast!!)
        assertEquals(next, lane(range).span)

        assertNull(timelinePreview(lane(milestone).dates, TimelineEdit.StartEdge(1), now))
        model.scheduleTimeline(lane(milestone), TimelineEdit.StartEdge(-2), "2026-11-21", generation)
        assertEquals("2026-11-21", lane(milestone).dates.start)
        model.scheduleTimeline(lane(openEnd), TimelineEdit.DueEdge(2), "2026-11-21", generation)
        assertEquals("2026-11-24", lane(openEnd).dates.due)

        val trayItem = model.read().timeline!!.unscheduled!!.single()
        model.scheduleTimeline(trayItem, TimelineEdit.Place(3), "2026-11-21", generation)
        assertEquals("2026-11-24", lane(unscheduled).dates.due)
        assertNotNull(model.read().toast?.organize)
        model.undo(model.read().toast!!)
        assertEquals(unscheduled, model.read().timeline!!.unscheduled!!.single().taskId)

        model.open(planned)
        model.scheduleTimeline(trayItem, TimelineEdit.Place(5), "2026-11-21", generation)
        assertNull(engine.task(unscheduled, now)?.due)
    }

    @Test fun rulerUsesSharedZoomIsoWeeksAndDatePreference() {
        assertEquals(35, timelineScale(TimelineZoom.MONTH, "2026-11-21").days)
        val q = timelineColumns(TimelineZoom.QUARTER, 0, 14, now)
        assertEquals(listOf(-5, 2, 9), q.map { it.day })
        assertEquals(listOf("W47", "W48", "W49"), q.map { it.week })
        assertTrue(q.all { it.days == 7 })
        assertEquals("23.11.", timelineColumns(TimelineZoom.WEEK, 2, 2, now).single().label)
        assertEquals("2026-11-23", timelineColumns(TimelineZoom.WEEK, 2, 2, now.copy(dateFormat = "iso")).single().label)
    }
}
