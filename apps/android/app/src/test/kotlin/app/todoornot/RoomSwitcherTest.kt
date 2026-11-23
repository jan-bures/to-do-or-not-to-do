package app.todoornot

import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Rect
import app.todoornot.core.RoomKey
import app.todoornot.core.SmartList
import app.todoornot.core.TaskDrop
import app.todoornot.core.filterRoomSwitcher
import app.todoornot.session.ThumbHand
import app.todoornot.session.ThumbHandPreference
import app.todoornot.ui.DragInput
import org.junit.Assert.*
import org.junit.Test

class RoomSwitcherTest {
    @Test fun room_search_keeps_order_and_parent_and_does_not_confuse_history_with_smart_rooms() {
        val engine = fixtureEngine()
        val home = engine.home(FIXTURE_NOW)
        val match = filterRoomSwitcher(home, " ERRANDS ")
        assertFalse(match.activity)
        assertFalse(match.trash)
        assertTrue(match.home.smart.isEmpty())
        val parent = match.home.groups.single()
        assertEquals("Personal", parent.collection?.name)
        assertEquals(listOf("Errands"), parent.lists.map { it.name })
        assertEquals(home.groups.single { it.collection?.name == "Personal" }, filterRoomSwitcher(home, "person").home.groups.single())
        val activity = filterRoomSwitcher(home, "ACTIVITY")
        assertTrue(activity.activity)
        assertEquals(RoomKey.Smart(SmartList.AGENT_ACTIVITY), activity.home.smart.single().key)
        assertEquals(home.agents, activity.home.agents)
        assertTrue(filterRoomSwitcher(home, "RASH").trash)
        assertEquals(home, filterRoomSwitcher(home, "   ").home)
    }

    @Test fun handedness_is_local_defaults_right_and_does_not_mirror_the_inner_panes() {
        val values = MapValues()
        assertEquals(ThumbHand.RIGHT, ThumbHandPreference(values).load())
        assertTrue(ThumbHandPreference(values).save(ThumbHand.LEFT))
        val remembered = ThumbHandPreference(values).load()
        assertEquals(ThumbHand.LEFT, remembered)
        assertTrue(remembered.mirrors(wide = false))
        assertFalse(remembered.mirrors(wide = true))
    }

    @Test fun drop_uses_the_correct_layer_and_never_passes_through_the_switcher() {
        val input = DragInput()
        input.list = DragInput.ScrollPane(Rect(0f, 0f, 400f, 200f)) { }
        input.rows["task"] = Rect(0f, 64f, 400f, 128f)
        input.pills[TaskDrop.List("peer")] = Rect(80f, 220f, 160f, 270f)
        input.nav[TaskDrop.MyDay] = Rect(216f, 180f, 330f, 236f)
        input.position = Offset(120f, 240f)
        assertEquals(TaskDrop.List("peer"), input.candidate(false))
        assertNull(input.candidate(true))
        input.position = Offset(250f, 200f)
        assertEquals(TaskDrop.MyDay, input.candidate(true))
        input.position = Offset(120f, 100f)
        assertEquals(TaskDrop.Slot("task", true), input.candidate(false))
        assertNull(input.candidate(true))
    }
}
