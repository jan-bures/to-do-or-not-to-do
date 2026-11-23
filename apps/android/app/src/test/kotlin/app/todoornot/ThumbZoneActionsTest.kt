package app.todoornot

import android.app.Application
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.width
import androidx.compose.runtime.mutableStateOf
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.semantics.SemanticsActions
import androidx.compose.ui.semantics.SemanticsProperties
import androidx.compose.ui.test.ExperimentalTestApi
import androidx.compose.ui.test.SemanticsMatcher
import androidx.compose.ui.test.assert
import androidx.compose.ui.test.assertIsNotDisplayed
import androidx.compose.ui.test.hasClickAction
import androidx.compose.ui.test.junit4.createComposeRule
import androidx.compose.ui.test.onNodeWithContentDescription
import androidx.compose.ui.test.onNodeWithText
import androidx.compose.ui.test.onRoot
import androidx.compose.ui.test.performClick
import androidx.compose.ui.test.performTouchInput
import androidx.compose.ui.test.performTextInput
import androidx.compose.ui.test.performScrollTo
import androidx.compose.ui.test.swipe
import androidx.compose.ui.unit.dp
import app.todoornot.core.ActivityFooter
import app.todoornot.core.ActivityLoad
import app.todoornot.core.ActivityView
import app.todoornot.core.RoomKey
import app.todoornot.core.SmartList
import app.todoornot.design.TodoTheme
import app.todoornot.design.Tokens
import app.todoornot.model.Actions
import app.todoornot.model.TaskSwipe
import app.todoornot.model.UiState
import app.todoornot.session.ThumbHand
import app.todoornot.ui.HeaderCollapse
import app.todoornot.ui.MainScreen
import app.todoornot.ui.RoomHeader
import app.todoornot.ui.insideSwipeEdges
import app.todoornot.ui.swipeDirection
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config

@RunWith(RobolectricTestRunner::class)
@Config(application = Application::class, sdk = [35], qualifiers = "w876dp-h970dp")
@OptIn(ExperimentalTestApi::class)
class ThumbZoneActionsTest {
    @Suppress("DEPRECATION")
    @get:Rule val compose = createComposeRule()

    @Test fun swipe_threshold_and_system_edges_are_independent_of_handedness() {
        val threshold = Tokens.Metrics.Phone.SWIPE_THRESHOLD
        assertNull(swipeDirection(threshold - 0.1f, threshold))
        assertNull(swipeDirection(-threshold + 0.1f, threshold))
        assertEquals(TaskSwipe.COMPLETION, swipeDirection(threshold, threshold))
        assertEquals(TaskSwipe.MY_DAY, swipeDirection(-threshold, threshold))
        val inset = Tokens.Metrics.Phone.EDGE_GESTURE_INSET
        assertFalse(insideSwipeEdges(inset - 0.1f, 412, inset))
        assertFalse(insideSwipeEdges(412 - inset, 412, inset))
        assertTrue(insideSwipeEdges(inset, 412, inset))
        assertTrue(insideSwipeEdges(412 - inset - 0.1f, 412, inset))
    }

    @Test fun detail_bar_mirrors_on_cover_and_routes_task_actions_with_the_rendered_generation() {
        val engine = fixtureEngine()
        val room = engine.list("Admin")
        val detail = requireNotNull(engine.task(engine.taskId(room, "Renew passport appointment"), FIXTURE_NOW))
        val state = UiState(engine.home(FIXTURE_NOW), engine.room(room, FIXTURE_NOW), detail, engine.status(), emptyList(), null, generation = 7)
        val hand = mutableStateOf(ThumbHand.RIGHT)
        val wide = mutableStateOf(false)
        val changes = mutableListOf<String>()
        val actions = object : Actions by Actions.None {
            override fun select(id: String?) { changes.add("close:$id") }
            override fun setDone(id: String, done: Boolean, generation: Long?, row: Boolean) {
                assertEquals(detail.id, id)
                changes.add("done:$done:$generation:$row")
            }
            override fun setMyDay(id: String, on: Boolean) { assertEquals(detail.id, id); changes.add("day:$on") }
            override fun setImportant(id: String, on: Boolean) { assertEquals(detail.id, id); changes.add("star:$on") }
        }
        compose.setContent { MainScreen(state, actions, false, wide.value, hand = hand.value) }
        val back = compose.onNodeWithContentDescription("Back")
        val more = compose.onNodeWithContentDescription("Task actions")
        val completion = compose.onNodeWithContentDescription("Task completion")
        fun x(node: androidx.compose.ui.test.SemanticsNodeInteraction) = node.fetchSemanticsNode().boundsInRoot.left
        assertTrue(x(back) < x(completion) && x(completion) < x(more))
        completion.performClick()
        compose.onNodeWithContentDescription("My Day").performClick()
        compose.onNodeWithContentDescription("Important").performClick()
        back.performClick()
        compose.runOnIdle { assertEquals(listOf("done:${!detail.done}:7:false", "day:${!detail.myDay}", "star:${detail.priority != 3.toUByte()}", "close:null"), changes) }
        compose.runOnIdle { hand.value = ThumbHand.LEFT }
        assertTrue(x(more) < x(completion) && x(completion) < x(back))
        compose.runOnIdle { wide.value = true }
        val close = compose.onNodeWithContentDescription("Close")
        assertTrue(x(completion) < x(more) && x(more) < x(close))
        more.performClick()
        compose.onNodeWithText("Move to…").performClick()
        compose.onNodeWithContentDescription("Filter lists").assertExists()
        compose.onNodeWithText("Cancel").performClick()
        compose.onNodeWithText("Move to…").assertExists()
    }

    @Test fun room_heading_and_timed_cue_are_read_only_and_trade_for_one_compact_announcement() {
        val engine = fixtureEngine("groceries")
        val room = engine.room(engine.list("Groceries"), FIXTURE_NOW)
        val collapse = HeaderCollapse()
        compose.setContent {
            TodoTheme(false, room.hue) {
                Box(Modifier.width(412.dp)) {
                    RoomHeader(room, false, collapse, engine.status(), onResetReach = {})
                }
            }
        }
        val isHeading = SemanticsMatcher.keyIsDefined(SemanticsProperties.Heading)
        val title = compose.onNodeWithText(room.title)
        val cue = compose.onNodeWithContentDescription("Next due at 18:00")
        title.assert(isHeading and !hasClickAction())
        cue.assert(!hasClickAction())
        assertTrue(title.fetchSemanticsNode().boundsInRoot.bottom <= cue.fetchSemanticsNode().boundsInRoot.top)
        compose.runOnIdle { collapse.consume(-collapse.range) }
        cue.assertIsNotDisplayed()
        compose.onNodeWithContentDescription("Groceries, 0 of 15 done").assert(isHeading and !hasClickAction())
    }

    @Test fun activity_footer_refresh_and_agent_pill_use_the_active_pager() {
        val e = fixtureEngine()
        val home = e.home(FIXTURE_NOW)
        val agent = home.agents.first()
        val state = UiState(home, e.room(RoomKey.Smart(SmartList.MY_DAY), FIXTURE_NOW), null, e.status(), emptyList(), null,
            activity = "", pagerId = 5L, feed = ActivityView(null, emptyList(), 0u, "Refresh activity", false, ActivityFooter.None))
        val requests = mutableListOf<ActivityLoad>()
        var filter: String? = null
        val actions = object : Actions by Actions.None {
            override fun loadActivity(pager: Long, kind: ActivityLoad) { assertEquals(5L, pager); requests.add(kind) }
            override fun openActivity(agent: String?) { filter = agent }
        }
        compose.setContent { MainScreen(state, actions, false, false) }
        val title = compose.onNodeWithText("Activity")
        val refresh = compose.onNodeWithContentDescription("Refresh activity")
        assertTrue(title.fetchSemanticsNode().boundsInRoot.bottom < refresh.fetchSemanticsNode().boundsInRoot.top)
        refresh.performClick()
        compose.onNodeWithText(agent.mark.name).performClick()
        compose.runOnIdle {
            assertEquals(listOf(ActivityLoad.INITIAL, ActivityLoad.REFRESH), requests)
            assertEquals(agent.id, filter)
        }
    }

    @Test fun tags_keep_the_filter_below_matching_choices_and_restore_it_from_options() {
        val engine = fixtureEngine()
        val room = engine.list("Admin")
        val detail = requireNotNull(engine.task(engine.taskId(room, "Renew passport appointment"), FIXTURE_NOW))
        val state = UiState(engine.home(FIXTURE_NOW), engine.room(room, FIXTURE_NOW), detail, engine.status(), emptyList(), null)
        compose.setContent { MainScreen(state, Actions.None, false, false) }
        compose.onNodeWithText("Tags").performScrollTo().performClick()
        val field = compose.onNodeWithContentDescription("Type to find or create a tag")
        field.performTextInput("admin")
        val options = compose.onNodeWithContentDescription("Options for #admin")
        assertTrue(options.fetchSemanticsNode().boundsInRoot.bottom <= field.fetchSemanticsNode().boundsInRoot.top)
        options.performClick()
        compose.onNodeWithText("Rename tag").assertExists()
        compose.onNodeWithText("All tags").performClick()
        compose.onNodeWithContentDescription("Options for #admin").assertExists()
        compose.onNodeWithText("#domácnost").assertDoesNotExist()
    }

    @Test fun the_row_gestures_and_talkback_route_the_same_actions() {
        val engine = fixtureEngine()
        val room = RoomKey.Smart(SmartList.MY_DAY)
        val read = engine.room(room, FIXTURE_NOW)
        val first = read.tasks.first()
        val state = UiState(engine.home(FIXTURE_NOW), read, null, engine.status(), emptyList(), null, generation = 4)
        val calls = mutableListOf<TaskSwipe>()
        val actions = object : Actions by Actions.None {
            override fun swipeTask(id: String, generation: Long, action: TaskSwipe) {
                assertEquals(first.id, id)
                assertEquals(4L, generation)
                calls.add(action)
            }
        }
        compose.setContent { MainScreen(state, actions, false, false) }
        val row = compose.onNodeWithText(first.title)
        val y = row.fetchSemanticsNode().boundsInRoot.center.y
        val width = compose.onRoot().fetchSemanticsNode().boundsInRoot.width
        compose.onRoot().performTouchInput { swipe(Offset(width * 0.3f, y), Offset(width * 0.65f, y)) }
        compose.onRoot().performTouchInput { swipe(Offset(width * 0.7f, y), Offset(width * 0.35f, y)) }
        compose.onRoot().performTouchInput { swipe(Offset(1f, y), Offset(width * 0.45f, y)) }
        val dayAction = row.fetchSemanticsNode().config[SemanticsActions.CustomActions].single { it.label.endsWith("My Day") }
        compose.runOnIdle {
            assertEquals(listOf(TaskSwipe.COMPLETION, TaskSwipe.MY_DAY), calls)
            assertTrue(dayAction.action())
            assertEquals(TaskSwipe.MY_DAY, calls.last())
        }
    }
}
