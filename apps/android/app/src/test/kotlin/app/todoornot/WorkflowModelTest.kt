package app.todoornot

import app.todoornot.core.BoardSlot
import app.todoornot.core.ListFeaturesPatch
import app.todoornot.core.LocalNow
import app.todoornot.core.localNow
import app.todoornot.core.ListView
import app.todoornot.core.OpenStage
import app.todoornot.core.QuickAddDraft
import app.todoornot.core.RoomKey
import app.todoornot.core.SmartList
import app.todoornot.core.WorkflowQuickAddDraft
import app.todoornot.core.WorkflowStage
import app.todoornot.design.Tokens
import app.todoornot.model.TodoModel
import app.todoornot.model.visibleCards
import app.todoornot.model.visibleStage
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

class WorkflowModelTest {
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Unconfined)
    private val engine = fixtureEngine("blank")
    private val myDay = RoomKey.Smart(SmartList.MY_DAY)
    private val list = engine.createList("Project", myDay) as RoomKey.List
    private var clock = 0L

    private fun model(now: LocalNow = FIXTURE_NOW) = TodoModel(engine, scope, Dispatchers.Unconfined, now = { now }, start = list, clock = { clock })

    @After fun close() { scope.cancel(); engine.close() }

    @Test
    fun defaultsAreSyncedButTheViewOverrideBelongsToThisVisit() {
        engine.setListFeatures(list.id, ListFeaturesPatch(true, true, ListView.BOARD))
        val model = model()
        val first = model.read()
        assertEquals(ListView.BOARD, first.view)
        assertNotNull(first.board)
        model.setRoomView(ListView.LIST, first.generation)
        assertEquals(ListView.LIST, model.read().view)
        assertEquals(ListView.BOARD, engine.listFeatures(list.id)?.defaultView)

        model.open(list)
        assertEquals(ListView.BOARD, model.read().view)
        model.setRoomView(ListView.LIST, first.generation)
        assertEquals(ListView.BOARD, model.read().view)
        model.setListFeatures(list.id, ListFeaturesPatch(null, null, ListView.TIMELINE))
        assertEquals(ListView.BOARD, model.read().view)
        assertEquals(ListView.TIMELINE, engine.listFeatures(list.id)?.defaultView)
        model.open(list)
        assertEquals(ListView.TIMELINE, model.read().view)
        assertNotNull(model.read().timeline)
        model.setRoomView(ListView.BOARD, model.read().generation)
        assertEquals(ListView.BOARD, model.read().view)
        model.setListFeatures(list.id, ListFeaturesPatch(false, null, null))
        assertNull(model.read().board)
        assertEquals(ListView.LIST, model.read().view)
    }

    @Test
    fun quickAddStagesTheTaskAndStatusStaysQuietOutsideItsList() {
        val model = model()
        model.setListFeatures(list.id, ListFeaturesPatch(true, true, ListView.BOARD))
        model.open(list)
        model.addWorkflow(WorkflowQuickAddDraft(QuickAddDraft("Prepare samples today..+3d status:wip", null, false, null), null, null))
        model.addWorkflow(WorkflowQuickAddDraft(QuickAddDraft("Collect samples", null, false, null), null, OpenStage.BACKLOG))
        model.add(QuickAddDraft("Count samples", null, false, null))
        var shown = model.read()
        val rows = shown.board!!.visibleCards(shown.room)
        val inProgress = rows.getValue(WorkflowStage.IN_PROGRESS).single()
        assertEquals("Prepare samples", inProgress.title)
        assertEquals("21.–24.11.", inProgress.dateSpan)
        assertEquals("Collect samples", rows.getValue(WorkflowStage.BACKLOG).single().title)
        model.setFilter("status:in_progress")
        shown = model.read()
        assertEquals(listOf(inProgress.id), shown.board!!.visibleCards(shown.room).values.flatten().map { it.id })
        assertEquals(3u, shown.room.open)
        model.open(RoomKey.Smart(SmartList.ALL))
        val all = model.read()
        assertNull(all.board)
        assertEquals(listOf(WorkflowStage.BACKLOG, WorkflowStage.IN_PROGRESS), all.room.tasks.mapNotNull { it.visibleStage() }.sorted())
    }

    @Test
    fun columnMoveUndoCompletionAndReopenUseTheSameWorkflow() {
        engine.setListFeatures(list.id, ListFeaturesPatch(true, true, ListView.BOARD))
        val task = engine.addWorkflowTask(list, WorkflowQuickAddDraft(QuickAddDraft("Build a shelf", "2026-11-22", false, null), null,
            OpenStage.BACKLOG), FIXTURE_NOW)
        // The offline test engine uses the system clock for completion events. Board recency uses it too.
        val model = model(localNow())
        val generation = model.read().generation
        model.moveBoardCard(list.id, task, WorkflowStage.IN_PROGRESS, BoardSlot.First, generation)
        assertEquals(WorkflowStage.IN_PROGRESS, engine.taskWorkflow(task)?.stage)
        model.undo(model.read().toast!!)
        assertEquals(WorkflowStage.BACKLOG, engine.taskWorkflow(task)?.stage)

        model.moveBoardCard(list.id, task, WorkflowStage.DONE, BoardSlot.First, generation)
        assertTrue(engine.taskDone(task) == true)
        val held = model.read()
        assertEquals(listOf(task), held.board!!.visibleCards(held.room).getValue(WorkflowStage.BACKLOG).map { it.id })
        val m = Tokens.Motion
        clock = (m.CHECK_TICK_DELAY + m.CHECK_TICK + m.COMPLETE_HOLD + m.ROW_COLLAPSE + 1).toLong()
        model.motion.freeze(clock)
        val completed = model.read()
        assertEquals(listOf(task), completed.board!!.visibleCards(completed.room).getValue(WorkflowStage.DONE).map { it.id })
        model.moveBoardCard(list.id, task, WorkflowStage.TODO, BoardSlot.First, generation)
        assertTrue(engine.taskDone(task) == false)
        assertEquals(WorkflowStage.TODO, engine.taskWorkflow(task)?.stage)
        model.undo(model.read().toast!!)
        assertEquals(WorkflowStage.DONE, engine.taskWorkflow(task)?.stage)
        model.setTaskStage(list.id, task, WorkflowStage.IN_PROGRESS, generation)
        assertFalse(engine.task(task, FIXTURE_NOW)!!.done)
        assertEquals(WorkflowStage.IN_PROGRESS, engine.taskWorkflow(task)?.stage)

        model.setStartDate(task, "2026-11-26")
        assertEquals("2026-11-26", engine.task(task, FIXTURE_NOW)?.due)
        model.undo(model.read().toast!!)
        assertEquals("2026-11-22", engine.task(task, FIXTURE_NOW)?.due)
        assertNull(engine.taskWorkflow(task)?.startDate)
    }
}
