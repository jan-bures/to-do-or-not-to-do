package app.todoornot

import android.app.Application
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.test.junit4.createComposeRule
import androidx.compose.ui.test.SemanticsNodeInteraction
import androidx.compose.ui.test.onNodeWithContentDescription
import androidx.compose.ui.test.onNodeWithText
import androidx.compose.ui.test.performClick
import androidx.compose.ui.test.performTouchInput
import androidx.compose.ui.test.onRoot
import androidx.compose.ui.unit.dp
import app.todoornot.core.ListView
import app.todoornot.core.TimelineEdit
import app.todoornot.core.TimelineTask
import app.todoornot.core.TimelineZoom
import app.todoornot.design.TodoTheme
import app.todoornot.model.Actions
import app.todoornot.model.TodoModel
import app.todoornot.ui.TimelineBody
import app.todoornot.ui.TimelineDragMode
import app.todoornot.ui.TimelineResize
import app.todoornot.ui.TimelineGallery
import app.todoornot.ui.TimelineSheets
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config

@RunWith(RobolectricTestRunner::class)
@Config(application = Application::class, sdk = [35], qualifiers = "w412dp-h960dp")
class TimelineInteractionTest {
    @Suppress("DEPRECATION")
    @get:Rule val compose = createComposeRule()

    @Test fun coverShowsOffscreenHintsAndCanPageToEitherSide() {
        val scope = CoroutineScope(SupervisorJob() + Dispatchers.Unconfined)
        val engine = fixtureEngine("workflow")
        try {
            val room = engine.list("Home renovation")
            val model = TodoModel(engine, scope, Dispatchers.Unconfined, now = { FIXTURE_NOW }, start = room)
            model.setRoomView(ListView.TIMELINE, model.read().generation)
            val state = model.read()
            val sheets = TimelineSheets()
            var added = false
            compose.setContent {
                TodoTheme(false, state.room.hue) {
                    TimelineBody(state, Actions.None, blocked = false, sheets, gallery = null, cover = true, onAdd = { added = true })
                }
            }
            compose.onNodeWithText("Tasks").assertDoesNotExist()
            compose.onNodeWithText("IN PROGRESS · 3").assertExists()
            val week = compose.onNodeWithText("W48", useUnmergedTree = true).fetchSemanticsNode().boundsInRoot
            val monday = compose.onNodeWithText("MON", useUnmergedTree = true).fetchSemanticsNode().boundsInRoot
            assertTrue(week.bottom < monday.top)
            val suffix = compose.onNodeWithText("· 1d overdue", useUnmergedTree = true).fetchSemanticsNode().boundsInRoot
            val title = compose.onNodeWithText("Pick grout color for bathroom tiles", useUnmergedTree = true).fetchSemanticsNode().boundsInRoot
            assertTrue(title.right < suffix.left)
            assertTrue(suffix.right < compose.onRoot().fetchSemanticsNode().boundsInRoot.right)
            for (label in listOf("Timeline zoom: Week", "Timeline zoom: 2 weeks", "Timeline zoom: Month", "Unscheduled: 1", "Add task")) {
                val bounds = compose.onNodeWithContentDescription(label).fetchSemanticsNode().boundsInRoot
                with(compose.density) {
                    assertTrue(bounds.width >= 48.dp.roundToPx())
                    assertTrue(bounds.height >= 48.dp.roundToPx())
                }
            }
            compose.onNodeWithContentDescription("Add task").performClick()
            compose.runOnIdle { assertTrue(added); assertTrue(sheets.coverEyebrow.orEmpty().endsWith("W47–W48")) }
            compose.onNodeWithText("Swipe left →").performClick()
            compose.onNodeWithText("← Swipe right").assertExists()
            compose.onNodeWithText("← Swipe right").performClick()
            compose.onNodeWithText("Swipe left →").assertExists()
        } finally {
            scope.cancel()
            engine.close()
        }
    }

    @Test fun touchTargetsAndLongPressMoveResizeAndSchedule() {
        val scope = CoroutineScope(SupervisorJob() + Dispatchers.Unconfined)
        val engine = fixtureEngine("workflow")
        try {
            val room = engine.list("Home renovation")
            val model = TodoModel(engine, scope, Dispatchers.Unconfined, now = { FIXTURE_NOW }, start = room)
            model.setRoomView(ListView.TIMELINE, model.read().generation)
            val state = model.read()
            val edits = mutableListOf<TimelineEdit>()
            val actions = object : Actions by Actions.None {
                override fun scheduleTimeline(task: TimelineTask, edit: TimelineEdit, today: String, generation: Long) {
                    edits += edit
                }
            }
            val sheets = TimelineSheets()
            compose.setContent {
                TodoTheme(false, state.room.hue) {
                    TimelineBody(state, actions, blocked = false, sheets, TimelineGallery(TimelineZoom.WEEK, tray = true))
                }
            }
            val bar = compose.onNodeWithContentDescription("Timeline bar: Pick grout color for bathroom tiles")
            for (target in listOf(bar, compose.onNodeWithContentDescription("Timeline zoom: Week"),
                compose.onNodeWithContentDescription("Schedule Compare kitchen pendant lights"))) {
                val bounds = target.fetchSemanticsNode().boundsInRoot
                with(compose.density) {
                    assertTrue(bounds.width >= 48.dp.roundToPx())
                    assertTrue(bounds.height >= 48.dp.roundToPx())
                }
            }
            val oneDay = with(compose.density) { 48.dp.toPx() }
            fun drag(node: SemanticsNodeInteraction, delta: Offset) {
                node.performTouchInput { down(center) }
                compose.mainClock.advanceTimeBy(650)
                node.performTouchInput { moveBy(delta); up() }
            }
            drag(bar, Offset(oneDay, 0f))
            drag(bar, Offset(-oneDay, 0f)) // Drops on the task rail, outside the date grid.
            compose.runOnIdle { assertEquals(listOf(TimelineEdit.Move(1)), edits) }

            val title = "Confirm electrician visit for kitchen outlets"
            val dueOnly = state.timeline!!.lanes.single { it.title == title }
            compose.runOnIdle { sheets.resize = TimelineResize(dueOnly.taskId, TimelineDragMode.START) }
            drag(compose.onNodeWithContentDescription("Timeline bar: $title"), Offset(-oneDay, 0f))

            val tray = compose.onNodeWithText("Compare kitchen pendant lights")
            val today = compose.onNodeWithContentDescription("Today, 21.11.").fetchSemanticsNode().boundsInRoot.center
            drag(tray, Offset(today.x, bar.fetchSemanticsNode().boundsInRoot.center.y) - tray.fetchSemanticsNode().boundsInRoot.center)
            compose.runOnIdle { assertEquals(listOf(TimelineEdit.Move(1), TimelineEdit.StartEdge(-1), TimelineEdit.Place(0)), edits) }
        } finally {
            scope.cancel()
            engine.close()
        }
    }
}
