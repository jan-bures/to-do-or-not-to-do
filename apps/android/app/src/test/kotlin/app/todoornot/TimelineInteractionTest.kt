package app.todoornot

import android.app.Application
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.test.junit4.createComposeRule
import androidx.compose.ui.test.SemanticsNodeInteraction
import androidx.compose.ui.test.onNodeWithContentDescription
import androidx.compose.ui.test.onNodeWithText
import androidx.compose.ui.test.performTouchInput
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
