package app.todoornot

import android.app.Application
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.ui.input.key.Key
import androidx.compose.ui.semantics.SemanticsActions
import androidx.compose.ui.test.ExperimentalTestApi
import androidx.compose.ui.test.assertIsFocused
import androidx.compose.ui.test.assertIsSelected
import androidx.compose.ui.test.hasText
import androidx.compose.ui.test.junit4.createComposeRule
import androidx.compose.ui.test.onNodeWithContentDescription
import androidx.compose.ui.test.onNodeWithText
import androidx.compose.ui.test.performClick
import androidx.compose.ui.test.performKeyInput
import androidx.compose.ui.test.performSemanticsAction
import androidx.compose.ui.test.pressKey
import app.todoornot.core.RepeatChoice
import app.todoornot.core.RepeatPreset
import app.todoornot.design.TodoTheme
import app.todoornot.model.TodoModel
import app.todoornot.ui.DetailPane
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.cancel
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
@Config(application = Application::class, sdk = [35], qualifiers = "w412dp-h920dp")
@OptIn(ExperimentalTestApi::class)
class DetailCalendarTest {
    @Suppress("DEPRECATION")
    @get:Rule val compose = createComposeRule()

    @Test
    fun due_opens_on_the_saved_date_and_only_a_choice_edits_it() {
        val engine = fixtureEngine()
        val room = engine.list("Admin")
        val task = engine.taskId(room, "Renew passport appointment")
        engine.setRepeat(task, RepeatChoice.Preset(RepeatPreset.DAILY), FIXTURE_NOW)
        val scope = CoroutineScope(Dispatchers.Unconfined)
        try {
            val model = TodoModel(engine, scope, Dispatchers.Unconfined, now = { FIXTURE_NOW }, start = room, listen = {})
            model.select(task)
            compose.setContent {
                val state by model.ui.collectAsState()
                TodoTheme(false, state.room.hue) { DetailPane(state, model, standalone = true) }
            }

            val due = compose.onNodeWithText("Due", substring = false)
            val original = compose.onNodeWithContentDescription("Sunday, 22 November 2026")
            val clear = compose.onNodeWithText("No date")
            fun open() {
                due.performKeyInput { pressKey(Key.Enter) }
                compose.waitUntilExactlyOneExists(hasText("No date"), timeoutMillis = 5_000)
            }

            due.performSemanticsAction(SemanticsActions.RequestFocus)
            open()
            original.assertIsSelected().assertIsFocused().performKeyInput { pressKey(Key.PageDown) }
            compose.onNodeWithContentDescription("Tuesday, 22 December 2026").assertIsFocused()
                .performKeyInput { pressKey(Key.Escape) }
            clear.assertDoesNotExist()
            due.assertIsFocused()
            assertEquals("2026-11-22", engine.task(task, FIXTURE_NOW)?.due)

            open()
            original.assertIsFocused().performKeyInput { pressKey(Key.DirectionRight) }
            compose.onNodeWithContentDescription("Monday, 23 November 2026").assertIsFocused()
                .performKeyInput { pressKey(Key.Enter) }
            clear.assertDoesNotExist()
            assertEquals("2026-11-23", engine.task(task, FIXTURE_NOW)?.due)
            assertTrue(requireNotNull(engine.task(task, FIXTURE_NOW)).repeats)

            open()
            clear.performClick()
            clear.assertDoesNotExist()
            val cleared = requireNotNull(engine.task(task, FIXTURE_NOW))
            assertNull(cleared.due)
            assertFalse(cleared.repeats)
            open()
            compose.onNodeWithContentDescription("Saturday, 21 November 2026").assertIsFocused()
            clear.assertIsSelected()
        } finally {
            scope.cancel()
        }
    }
}
