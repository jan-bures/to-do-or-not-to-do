package app.todoornot

import android.app.Application
import androidx.activity.ComponentActivity
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.semantics.SemanticsActions
import androidx.compose.ui.semantics.SemanticsProperties
import androidx.compose.ui.test.ExperimentalTestApi
import androidx.compose.ui.test.SemanticsMatcher
import androidx.compose.ui.test.assertIsDisplayed
import androidx.compose.ui.test.assertIsOff
import androidx.compose.ui.test.assertIsOn
import androidx.compose.ui.test.click
import androidx.compose.ui.test.junit4.createAndroidComposeRule
import androidx.compose.ui.test.onNodeWithContentDescription
import androidx.compose.ui.test.onNodeWithText
import androidx.compose.ui.test.performFirstLinkClick
import androidx.compose.ui.test.performSemanticsAction
import androidx.compose.ui.test.performTextReplacement
import androidx.compose.ui.test.performTouchInput
import androidx.compose.ui.test.swipeLeft
import app.todoornot.core.NoteBlock
import app.todoornot.core.NoteDocument
import app.todoornot.core.parseNote
import app.todoornot.design.TodoTheme
import app.todoornot.model.TodoModel
import app.todoornot.ui.DetailPane
import app.todoornot.ui.MarkdownNotes
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.cancel
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config

@RunWith(RobolectricTestRunner::class)
@Config(application = Application::class, sdk = [35], qualifiers = "w412dp-h920dp")
class NotesInteractionTest {
    @Suppress("DEPRECATION")
    @get:Rule val compose = createAndroidComposeRule<ComponentActivity>()

    @OptIn(ExperimentalTestApi::class)
    @Test
    fun `touching a link or a checkbox does not edit the note`() {
        val source = "See [floor plan](https://example.com/plan).\n\n- [ ] Confirm the room\n\nWorkshop setup"
        val document = parseNote(source)
        var edits = 0
        val opened = mutableListOf<String>()
        compose.setContent {
            var note by remember { mutableStateOf(document) }
            TodoTheme(false, "my-day") {
                MarkdownNotes(note, editable = true, onEdit = { edits++ }, onToggle = {
                    val list = note.blocks[1] as NoteBlock.ListBlock
                    val item = list.items.single()
                    val updated = list.copy(items = listOf(item.copy(checkbox = it.copy(checked = true))))
                    note = NoteDocument(listOf(note.blocks[0], updated, note.blocks[2]))
                }, onLink = opened::add)
            }
        }
        val checkbox = compose.onNodeWithContentDescription("Confirm the room")
        checkbox.assertIsOff().performTouchInput { click() }
        checkbox.assertIsOn()
        compose.onNodeWithText("See floor plan.").performFirstLinkClick()
        compose.runOnIdle {
            assertEquals(0, edits)
            assertEquals(listOf("https://example.com/plan"), opened)
        }
        compose.onNodeWithText("Workshop setup").performTouchInput { click() }
        compose.runOnIdle {
            assertEquals(listOf("https://example.com/plan"), opened)
            assertEquals(1, edits)
        }
    }

    @Test
    fun `only the code block scrolls sideways`() {
        val note = parseNote("```text\n${"long-value-".repeat(25)}\n```\n\nAfter the block.")
        var edits = 0
        compose.setContent {
            TodoTheme(false, "my-day") {
                MarkdownNotes(note, editable = true, onEdit = { edits++ }, onToggle = {}, onLink = {})
            }
        }
        val axis = SemanticsProperties.HorizontalScrollAxisRange
        val block = compose.onNode(SemanticsMatcher.keyIsDefined(axis))
        val range = block.fetchSemanticsNode().config[axis]
        assertTrue(range.maxValue() > 0f)
        block.performTouchInput { swipeLeft() }
        compose.runOnIdle {
            assertTrue(range.value() > 0f)
            assertEquals(0, edits)
        }
        compose.onNodeWithText("After the block.").assertIsDisplayed()
    }

    @Test
    fun `losing focus and system Back save the source and restore the preview`() {
        val scope = CoroutineScope(Dispatchers.Unconfined)
        fixtureEngine("notes-code").use { engine ->
            try {
                val room = engine.list("Q4 planning")
                val task = engine.taskId(room, "Prepare the November board update")
                val model = TodoModel(engine, scope, Dispatchers.Unconfined, now = { FIXTURE_NOW }, start = room, listen = {})
                model.select(task)
                compose.setContent {
                    val state by model.ui.collectAsState()
                    TodoTheme(false, state.room.hue) { DetailPane(state, model, standalone = true) }
                }
                val input = compose.onNodeWithContentDescription("Notes")
                compose.onNodeWithText("Export label").performTouchInput { click() }
                input.performTextReplacement("# Revised plan")
                compose.onNodeWithContentDescription("Title").performSemanticsAction(SemanticsActions.RequestFocus)
                input.assertDoesNotExist()
                compose.onNodeWithText("Revised plan").performTouchInput { click() }
                input.performTextReplacement("## Final plan")
                compose.runOnIdle { compose.activity.onBackPressedDispatcher.onBackPressed() }
                input.assertDoesNotExist()
                compose.onNodeWithText("Final plan").assertExists()
                assertEquals("## Final plan", engine.task(task, FIXTURE_NOW)?.notes)
            } finally {
                scope.cancel()
            }
        }
    }
}
