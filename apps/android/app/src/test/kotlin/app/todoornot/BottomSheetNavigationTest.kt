package app.todoornot

import android.app.Application
import androidx.activity.ComponentActivity
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.input.nestedscroll.NestedScrollSource
import androidx.compose.ui.test.ExperimentalTestApi
import androidx.compose.ui.test.hasScrollAction
import androidx.compose.ui.test.hasText
import androidx.compose.ui.test.junit4.createAndroidComposeRule
import androidx.compose.ui.test.onNodeWithContentDescription
import androidx.compose.ui.test.onNodeWithText
import androidx.compose.ui.test.performClick
import androidx.compose.ui.test.performTextInput
import androidx.compose.ui.test.performTouchInput
import androidx.compose.ui.test.swipeDown
import androidx.compose.ui.unit.Velocity
import app.todoornot.core.RoomKey
import app.todoornot.core.SmartList
import app.todoornot.model.Actions
import app.todoornot.model.UiState
import app.todoornot.ui.MainScreen
import app.todoornot.ui.SheetDrag
import kotlinx.coroutines.runBlocking
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config

@RunWith(RobolectricTestRunner::class)
@Config(application = Application::class, sdk = [35], qualifiers = "w412dp-h920dp")
@OptIn(ExperimentalTestApi::class)
class BottomSheetNavigationTest {
    @Suppress("DEPRECATION")
    @get:Rule val compose = createAndroidComposeRule<ComponentActivity>()

    private val finger = NestedScrollSource.UserInput

    private fun sheetDrag(tall: Boolean = true, onDismiss: () -> Unit = {}) = SheetDrag(
        tall = tall,
        roomToGrow = if (tall) 180f else 0f,
        dismissDistance = 44f,
        dismissVelocity = 360f,
        onDismiss = onDismiss,
    )

    @Test fun nested_pull_down_from_top_dismisses_past_threshold_or_on_downward_fling() = runBlocking {
        var dismissed = 0
        val sheet = sheetDrag { dismissed++ }
        val conn = sheet.nestedScroll { if (sheet.dragging) sheet.drag else sheet.rest() }

        // Pull down past threshold and release slowly -> closes and keeps release offset.
        assertEquals(Offset(0f, 60f), conn.onPostScroll(Offset.Zero, Offset(0f, 60f), finger))
        assertEquals(-60f, sheet.drag, 0f)
        assertEquals(Velocity.Zero, conn.onPreFling(Velocity.Zero))
        assertEquals(1, dismissed)
        assertEquals(-60f, sheet.rest(), 0f)

        // Pull down below threshold with a brisk downward fling -> closes too.
        var flingDismissed = 0
        val flung = sheetDrag { flingDismissed++ }
        val flingConn = flung.nestedScroll { if (flung.dragging) flung.drag else flung.rest() }
        assertEquals(Offset(0f, 20f), flingConn.onPostScroll(Offset.Zero, Offset(0f, 20f), finger))
        assertEquals(Velocity(0f, 600f), flingConn.onPreFling(Velocity(0f, 600f)))
        assertEquals(1, flingDismissed)
    }

    @Test fun inertial_fling_and_scrolling_into_top_do_not_accidentally_dismiss() = runBlocking {
        var dismissed = 0
        val sheet = sheetDrag { dismissed++ }
        val conn = sheet.nestedScroll { if (sheet.dragging) sheet.drag else sheet.rest() }

        // Inertial fling hitting the top of a long list must never pull or dismiss the sheet.
        assertEquals(Offset.Zero, conn.onPostScroll(Offset.Zero, Offset(0f, 120f), NestedScrollSource.SideEffect))
        assertEquals(0f, sheet.drag, 0f)
        assertFalse(sheet.dragging)

        // Dragging down while the list scrolls toward top and overshooting must stop at the top
        // instead of closing in the same gesture.
        assertEquals(Offset.Zero, conn.onPostScroll(Offset(0f, 150f), Offset.Zero, finger))
        assertEquals(Offset.Zero, conn.onPostScroll(Offset(0f, 30f), Offset(0f, 80f), finger))
        assertEquals(Velocity.Zero, conn.onPreFling(Velocity(0f, 1200f)))
        assertEquals(0, dismissed)
        assertFalse(sheet.dragging)

        // Now that the list is at the top, a fresh pull down closes the sheet.
        assertEquals(Offset(0f, 60f), conn.onPostScroll(Offset.Zero, Offset(0f, 60f), finger))
        assertEquals(Velocity(0f, 100f), conn.onPreFling(Velocity(0f, 100f)))
        assertEquals(1, dismissed)
    }

    @Test fun pulling_back_up_restores_sheet_before_scrolling_the_list() = runBlocking {
        var dismissed = 0
        val sheet = sheetDrag { dismissed++ }
        val conn = sheet.nestedScroll { if (sheet.dragging) sheet.drag else sheet.rest() }

        conn.onPostScroll(Offset.Zero, Offset(0f, 50f), finger)
        assertEquals(-50f, sheet.drag, 0f)
        // Reversing upward consumes only what brings the sheet back to rest, leaving the rest for the list.
        assertEquals(Offset(0f, -50f), conn.onPreScroll(Offset(0f, -80f), finger))
        assertEquals(0f, sheet.drag, 0f)
        assertFalse(sheet.dragging)
        // Because the sheet is back at rest, upward fling passes through to the list.
        assertEquals(Velocity.Zero, conn.onPreFling(Velocity(0f, -900f)))
        assertEquals(0, dismissed)
    }

    @Test fun direct_drag_expands_cancels_on_upward_flick_and_dismisses_downward() {
        var dismissed = 0
        val sheet = sheetDrag(tall = true) { dismissed++ }

        sheet.startDirect(0f)
        sheet.onDirectDrag(-110f)
        sheet.finishDirect(0f)
        assertTrue(sheet.expanded)
        assertEquals(0, dismissed)

        // Pulling down past threshold, then flicking up, cancels dismissal.
        sheet.startDirect(0f)
        sheet.expanded = false
        sheet.onDirectDrag(60f)
        sheet.finishDirect(-500f)
        assertEquals(0, dismissed)

        // Pulling down past threshold and releasing dismisses.
        sheet.startDirect(0f)
        sheet.onDirectDrag(60f)
        sheet.finishDirect(0f)
        assertEquals(1, dismissed)
    }

    @Test fun back_from_list_opens_switcher_and_back_at_switcher_root_closes_app() {
        val engine = fixtureEngine()
        val room = RoomKey.Smart(SmartList.MY_DAY)
        val state = UiState(engine.home(FIXTURE_NOW), engine.room(room, FIXTURE_NOW), null, engine.status(), emptyList(), null)

        compose.setContent { MainScreen(state, Actions.None, false, false, server = "https://todo.example.com") }

        compose.onNodeWithContentDescription("Find a room").assertDoesNotExist()
        assertFalse(compose.activity.isFinishing)

        // Back from the list view opens the room switcher menu.
        compose.runOnIdle { compose.activity.onBackPressedDispatcher.onBackPressed() }
        compose.onNodeWithContentDescription("Find a room").assertExists()
        assertFalse(compose.activity.isFinishing)

        // Back inside a switcher sub-state (active search filter) clears the filter without closing the app.
        compose.onNodeWithContentDescription("Find a room").performTextInput("Errands")
        compose.runOnIdle { compose.activity.onBackPressedDispatcher.onBackPressed() }
        compose.onNodeWithContentDescription("Find a room").assertExists()
        assertFalse(compose.activity.isFinishing)

        // Back inside Settings returns to the room switcher without closing the app.
        compose.onNodeWithContentDescription("Settings").performClick()
        compose.onNodeWithText("Open admin pages").assertExists()
        compose.runOnIdle { compose.activity.onBackPressedDispatcher.onBackPressed() }
        compose.onNodeWithContentDescription("Find a room").assertExists()
        assertFalse(compose.activity.isFinishing)

        // Back at the root of the room switcher is unhandled so Android closes the app.
        compose.runOnIdle {
            assertFalse(compose.activity.onBackPressedDispatcher.hasEnabledCallbacks())
            compose.activity.onBackPressedDispatcher.onBackPressed()
        }
        assertTrue(compose.activity.isFinishing)
    }

    @Test fun back_closes_detail_and_other_sheets_before_opening_the_switcher() {
        val engine = fixtureEngine()
        val room = engine.list("Admin")
        val taskId = engine.taskId(room, "Renew passport appointment")
        val detail = requireNotNull(engine.task(taskId, FIXTURE_NOW))
        val selected = androidx.compose.runtime.mutableStateOf<app.todoornot.core.TaskDetail?>(detail)
        val actions = object : Actions by Actions.None {
            override fun select(id: String?) { if (id == null) selected.value = null }
        }
        compose.setContent {
            val state = UiState(engine.home(FIXTURE_NOW), engine.room(room, FIXTURE_NOW), selected.value, engine.status(), emptyList(), null)
            MainScreen(state, actions, false, false)
        }

        // First Back closes task detail back to the list view (not the switcher).
        compose.runOnIdle { compose.activity.onBackPressedDispatcher.onBackPressed() }
        compose.runOnIdle { assertEquals(null, selected.value) }
        compose.onNodeWithContentDescription("Find a room").assertDoesNotExist()
        assertFalse(compose.activity.isFinishing)

        // Opening Room menu sheet and pressing Back closes the sheet back to the list view.
        compose.onNodeWithContentDescription("Room menu").performClick()
        compose.onNodeWithText("Rename list").assertExists()
        compose.runOnIdle { compose.activity.onBackPressedDispatcher.onBackPressed() }
        compose.onNodeWithText("Rename list").assertDoesNotExist()
        compose.onNodeWithContentDescription("Find a room").assertDoesNotExist()
        assertFalse(compose.activity.isFinishing)

        // Next Back from the list view opens the switcher.
        compose.runOnIdle { compose.activity.onBackPressedDispatcher.onBackPressed() }
        compose.onNodeWithContentDescription("Find a room").assertExists()
        assertFalse(compose.activity.isFinishing)
    }

    @Test fun pulling_down_on_scrolled_to_top_room_list_or_smart_tiles_closes_switcher() {
        val engine = fixtureEngine()
        val room = RoomKey.Smart(SmartList.MY_DAY)
        val state = UiState(engine.home(FIXTURE_NOW), engine.room(room, FIXTURE_NOW), null, engine.status(), emptyList(), null)
        var opened: RoomKey? = null
        val actions = object : Actions by Actions.None {
            override fun open(room: RoomKey) { opened = room }
        }
        val firstName = state.home.groups.first().let { it.collection?.name ?: it.lists.first().name }

        compose.setContent { MainScreen(state, actions, false, false) }

        // Open switcher via Back, scroll the room list to the top if needed, then pull down to close.
        compose.runOnIdle { compose.activity.onBackPressedDispatcher.onBackPressed() }
        compose.onNodeWithContentDescription("Find a room").assertExists()
        val scrollPane = compose.onNode(hasScrollAction())
        for (i in 0 until 6) {
            if (compose.onAllNodes(hasText(firstName)).fetchSemanticsNodes().isNotEmpty()) break
            scrollPane.performTouchInput {
                swipeDown(startY = top + height * 0.25f, endY = top + height * 0.75f, durationMillis = 300)
            }
            compose.onNodeWithContentDescription("Find a room").assertExists()
        }
        compose.onNodeWithText(firstName).assertExists()
        scrollPane.performTouchInput {
            swipeDown(startY = top + height * 0.2f, endY = top + height * 0.9f, durationMillis = 250)
        }
        compose.onNodeWithContentDescription("Find a room").assertDoesNotExist()
        compose.runOnIdle { assertEquals(null, opened) }

        // Open switcher again, then pull down on a non-scrollable Smart Room tile without activating it.
        compose.runOnIdle { compose.activity.onBackPressedDispatcher.onBackPressed() }
        compose.onNodeWithContentDescription("Find a room").assertExists()
        compose.onNodeWithContentDescription("Important").performTouchInput {
            swipeDown(startY = centerY, endY = centerY + 300f, durationMillis = 250)
        }
        compose.onNodeWithContentDescription("Find a room").assertDoesNotExist()
        compose.runOnIdle { assertEquals(null, opened) }
    }
}
