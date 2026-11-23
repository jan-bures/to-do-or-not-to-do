package app.todoornot

import app.todoornot.core.RoomKey
import app.todoornot.core.SmartList
import app.todoornot.core.StepRow
import app.todoornot.model.MenuAction
import app.todoornot.model.MenuEntry
import app.todoornot.model.drawerMenu
import app.todoornot.model.priorityMenu
import app.todoornot.model.renameTitle
import app.todoornot.model.roomMenu
import app.todoornot.model.siblingPlace
import app.todoornot.model.stepMenu
import app.todoornot.model.swappedWith
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/** The menus as data: what each offers where, and which moves are possible. */
class MenusTest {
    private val engine = fixtureEngine()
    private val home = engine.home(FIXTURE_NOW)

    private fun List<MenuEntry>.labels() = map { if (it is MenuEntry.Item) it.label else "—" }

    private fun List<MenuEntry>.item(label: String) = filterIsInstance<MenuEntry.Item>().first { it.label == label }

    @Test
    fun a_lists_drawer_menu_renames_moves_and_asks_before_deleting() {
        val errands = engine.list("Errands") as RoomKey.List
        val menu = drawerMenu(home, errands)
        assertEquals(listOf("Rename", "Color…", "Move to…", "List settings…", "Move up", "Move down", "—", "Delete list…"), menu.labels())
        assertEquals(MenuAction.Rename(errands), menu.item("Rename").action)
        assertEquals(MenuAction.AskDelete(errands.id), menu.item("Delete list…").action)
        assertTrue(menu.item("Delete list…").danger)
        // Only one end of its collection's lists is off.
        val lists = home.groups.first { g -> g.lists.any { it.key == errands } }.lists.map { it.key }
        val i = lists.indexOf(errands)
        assertEquals(i > 0, menu.item("Move up").enabled)
        assertEquals(i < lists.size - 1, menu.item("Move down").enabled)
        assertEquals(lists.getOrNull(i - 1), swappedWith(home, errands, -1))
        assertEquals(lists.getOrNull(i + 1), swappedWith(home, errands, 1))
        assertEquals("Rename list", renameTitle(errands))
    }

    @Test
    fun a_collections_drawer_menu_renames_and_moves_only() {
        val collections = home.groups.mapNotNull { it.collection?.key }
        val first = collections.first()
        val menu = drawerMenu(home, first)
        assertEquals(listOf("Rename", "Color…", "Move up", "Move down", "—", "Delete collection…"), menu.labels())
        assertFalse(menu.item("Move up").enabled)
        assertTrue(menu.item("Move down").enabled)
        assertFalse(drawerMenu(home, collections.last()).item("Move down").enabled)
        assertEquals(true to false, siblingPlace(home, collections.last()))
        assertNull(swappedWith(home, first, -1))
        assertEquals("Rename collection", renameTitle(first))
        // A smart list, or a list that's gone, has no menu.
        assertTrue(drawerMenu(home, RoomKey.Smart(SmartList.MY_DAY)).isEmpty())
        assertTrue(drawerMenu(home, RoomKey.List("gone")).isEmpty())
    }

    @Test
    fun the_room_menu_offers_only_what_applies() {
        val errands = engine.list("Errands") as RoomKey.List
        assertEquals(
            listOf("Rename list", "Color…", "Move to…", "List settings…", "Move up", "Move down", "Show completed", "—", "Delete list…"),
            roomMenu(home, errands, showCompleted = false).labels(),
        )
        assertEquals(MenuAction.ShowCompleted(false), roomMenu(home, errands, showCompleted = true).item("Hide completed").action)
        assertEquals(
            listOf("Rename collection", "Color…", "Move up", "Move down", "Show completed", "—", "Delete collection…"),
            roomMenu(home, engine.collection("Personal"), showCompleted = false).labels(),
        )
        for (smart in listOf(SmartList.MY_DAY, SmartList.IMPORTANT, SmartList.PLANNED, SmartList.ALL)) {
            assertEquals(listOf("Show completed"), roomMenu(home, RoomKey.Smart(smart), false).labels())
        }
        // These list their done tasks among their rows: no menu, so no button.
        assertTrue(roomMenu(home, RoomKey.Smart(SmartList.COMPLETED), false).isEmpty())
        assertTrue(roomMenu(home, RoomKey.Smart(SmartList.AGENT_ACTIVITY), false).isEmpty())
    }

    @Test
    fun a_steps_menu_moves_it_within_its_task() {
        val steps = listOf(StepRow("a", "Pack", false), StepRow("b", "Go", true), StepRow("c", "Return", false))
        val first = stepMenu(steps, "a")
        assertEquals(listOf("Rename", "Move up", "Move down", "—", "Delete step"), first.labels())
        assertFalse(first.item("Move up").enabled)
        assertTrue(first.item("Move down").enabled)
        assertEquals(MenuAction.MoveStep("a", 1), first.item("Move down").action)
        val last = stepMenu(steps, "c")
        assertTrue(last.item("Move up").enabled)
        assertFalse(last.item("Move down").enabled)
        assertEquals(MenuAction.DeleteStep("b"), stepMenu(steps, "b").item("Delete step").action)
        assertTrue(stepMenu(steps, "gone").isEmpty())
    }

    @Test
    fun the_priority_picker_lists_four_levels_highest_first_with_the_current_ticked() {
        val menu = priorityMenu("t", current = 2).filterIsInstance<MenuEntry.Item>()
        assertEquals(listOf("High", "Normal", "Low", "None"), menu.map { it.label })
        assertEquals(listOf("!!!", "!!", "!", null), menu.map { it.hint })
        assertEquals(listOf(false, true, false, false), menu.map { it.checked })
        assertEquals(listOf(3, 2, 1, 0), menu.map { (it.action as MenuAction.SetPriority).marks })
    }
}
