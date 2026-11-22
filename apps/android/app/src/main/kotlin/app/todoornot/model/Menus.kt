package app.todoornot.model

import app.todoornot.core.Home
import app.todoornot.core.RoomKey
import app.todoornot.core.RoomView
import app.todoornot.core.StepRow

/**
 * The menus, as data (as the desktop's `model::menus`): the drawer's long-press menus for a list or a
 * collection, the room's overflow menu, a step's menu and the detail pane's priority picker. The UI draws
 * them ([app.todoornot.ui.MenuCard]) and hands the chosen [MenuAction] back.
 */
sealed interface MenuEntry {
    data class Item(
        val icon: String,
        val label: String,
        val action: MenuAction,
        val danger: Boolean = false,
        /** Ticked (the current priority). */
        val checked: Boolean = false,
        /** Shown at the end, in the priority marks' type (`!!!`). */
        val hint: String? = null,
        /** False at either end of a Move up / Move down. */
        val enabled: Boolean = true,
    ) : MenuEntry

    data object Separator : MenuEntry
}

/** What a menu item does. */
sealed interface MenuAction {
    data class Done(val on: Boolean) : MenuAction
    data class MyDay(val on: Boolean) : MenuAction
    data class Important(val on: Boolean) : MenuAction
    data object PickPriority : MenuAction
    data object PickList : MenuAction
    data object DeleteTask : MenuAction
    data class ReorderTask(val list: String, val neighbour: String?, val by: Int) : MenuAction

    /** Rename a list or a collection (a dialog). */
    data class Rename(val room: RoomKey) : MenuAction

    /** Move a list or a collection one place up (-1) or down (1). */
    data class Move(val room: RoomKey, val by: Int) : MenuAction

    /** Ask before deleting a list. */
    data class AskDelete(val list: String) : MenuAction

    data class ShowCompleted(val on: Boolean) : MenuAction

    /** Rename a step in place. */
    data class RenameStep(val step: String) : MenuAction

    data class MoveStep(val step: String, val by: Int) : MenuAction

    data class DeleteStep(val step: String) : MenuAction

    /** Set the priority by its `!` marks: 3 (high) to 0 (none). */
    data class SetPriority(val task: String, val marks: Int) : MenuAction
}

/** The row and the detail header offer the same actions. Only open rows of a manual list
 * have visible neighbours to swap with (Completed is ordered by completion time).
 */
fun taskMenu(room: RoomView, id: String, done: Boolean, myDay: Boolean, priority: Int): List<MenuEntry> = buildList {
    add(MenuEntry.Item("circle-check", if (done) "Reopen" else "Complete", MenuAction.Done(!done)))
    add(MenuEntry.Item("sun", if (myDay) "Remove from My Day" else "Add to My Day", MenuAction.MyDay(!myDay)))
    add(MenuEntry.Item("star", if (priority == 3) "Remove importance" else "Mark as important", MenuAction.Important(priority != 3)))
    add(MenuEntry.Item("flag", "Priority…", MenuAction.PickPriority, hint = "!".repeat(priority).ifEmpty { null }))
    add(MenuEntry.Item("list", "Move to…", MenuAction.PickList))
    val list = room.key as? RoomKey.List
    if (list != null && !done) {
        val i = room.tasks.indexOfFirst { it.id == id }
        if (i >= 0) for (by in listOf(-1, 1)) {
            val other = room.tasks.getOrNull(i + by)?.id
            add(MenuEntry.Item(if (by < 0) "chevron-up" else "chevron-down", if (by < 0) "Move up" else "Move down",
                MenuAction.ReorderTask(list.id, other, by), enabled = other != null))
        }
    }
    add(MenuEntry.Separator)
    add(MenuEntry.Item("trash-2", "Delete task", MenuAction.DeleteTask, danger = true))
}

/** Where [room] stands among its siblings, for Move up and Move down; null when it isn't in the drawer. */
fun siblingPlace(home: Home, room: RoomKey): Pair<Boolean, Boolean>? {
    val siblings: List<RoomKey> = when (room) {
        is RoomKey.List -> home.groups.firstOrNull { g -> g.lists.any { it.key == room } }?.lists?.map { it.key }
        is RoomKey.Collection -> home.groups.mapNotNull { it.collection?.key }
        is RoomKey.Smart -> null
    } ?: return null
    val i = siblings.indexOf(room)
    if (i < 0) return null
    return (i > 0) to (i < siblings.size - 1)
}

/** The list or collection [room] swaps places with when it moves [by] one place; null at either end. */
fun swappedWith(home: Home, room: RoomKey, by: Int): RoomKey? {
    val siblings: List<RoomKey> = when (room) {
        is RoomKey.List -> home.groups.firstOrNull { g -> g.lists.any { it.key == room } }?.lists?.map { it.key }
        is RoomKey.Collection -> home.groups.mapNotNull { it.collection?.key }
        is RoomKey.Smart -> null
    } ?: return null
    val i = siblings.indexOf(room)
    return if (i < 0) null else siblings.getOrNull(i + by)
}

private fun moves(home: Home, room: RoomKey): List<MenuEntry> {
    val (up, down) = siblingPlace(home, room) ?: return emptyList()
    return listOf(
        MenuEntry.Item("chevron-up", "Move up", MenuAction.Move(room, -1), enabled = up),
        MenuEntry.Item("chevron-down", "Move down", MenuAction.Move(room, 1), enabled = down),
    )
}

/** A list's or a collection's menu in the drawer (long-press): rename, move, and for a list, delete. */
fun drawerMenu(home: Home, room: RoomKey): List<MenuEntry> = when (room) {
    is RoomKey.List -> if (siblingPlace(home, room) == null) {
        emptyList()
    } else {
        listOf(MenuEntry.Item("pencil", "Rename", MenuAction.Rename(room))) + moves(home, room) + listOf(
            MenuEntry.Separator,
            MenuEntry.Item("trash-2", "Delete list…", MenuAction.AskDelete(room.id), danger = true),
        )
    }
    is RoomKey.Collection -> if (siblingPlace(home, room) == null) {
        emptyList()
    } else {
        listOf(MenuEntry.Item("pencil", "Rename", MenuAction.Rename(room))) + moves(home, room)
    }
    is RoomKey.Smart -> emptyList()
}

/**
 * The room's overflow menu: only what applies to the room. A list: rename, move, the Completed toggle and
 * delete; a collection: rename, move and the toggle; My Day, Important, Planned and All: the toggle.
 * Completed and Agent activity list their done tasks among the rows, so they have no menu (and no button).
 */
fun roomMenu(home: Home, room: RoomKey, showCompleted: Boolean): List<MenuEntry> {
    val completed = MenuEntry.Item(
        if (showCompleted) "eye-off" else "eye",
        if (showCompleted) "Hide completed" else "Show completed",
        MenuAction.ShowCompleted(!showCompleted),
    )
    return when (room) {
        is RoomKey.List -> if (siblingPlace(home, room) == null) {
            emptyList()
        } else {
            listOf(MenuEntry.Item("pencil", "Rename list", MenuAction.Rename(room))) + moves(home, room) + listOf(
                completed,
                MenuEntry.Separator,
                MenuEntry.Item("trash-2", "Delete list…", MenuAction.AskDelete(room.id), danger = true),
            )
        }
        is RoomKey.Collection -> if (siblingPlace(home, room) == null) {
            emptyList()
        } else {
            listOf(MenuEntry.Item("pencil", "Rename collection", MenuAction.Rename(room))) + moves(home, room) + completed
        }
        is RoomKey.Smart -> if (listsDoneAmongRows(room)) emptyList() else listOf(completed)
    }
}

/** A step's menu: rename it in place, move it, or delete it (Undo brings it back). */
fun stepMenu(steps: List<StepRow>, step: String): List<MenuEntry> {
    val i = steps.indexOfFirst { it.id == step }
    if (i < 0) return emptyList()
    return listOf(
        MenuEntry.Item("pencil", "Rename", MenuAction.RenameStep(step)),
        MenuEntry.Item("chevron-up", "Move up", MenuAction.MoveStep(step, -1), enabled = i > 0),
        MenuEntry.Item("chevron-down", "Move down", MenuAction.MoveStep(step, 1), enabled = i < steps.size - 1),
        MenuEntry.Separator,
        MenuEntry.Item("trash-2", "Delete step", MenuAction.DeleteStep(step), danger = true),
    )
}

/** The priority's name for its `!` marks, as the desktop's picker words it. */
fun priorityName(marks: Int): String = when (marks) {
    3 -> "High"
    2 -> "Normal"
    1 -> "Low"
    else -> "None"
}

/** The detail pane's priority picker: all four levels, highest first, the current one ticked. */
fun priorityMenu(task: String, current: Int): List<MenuEntry> = listOf(3, 2, 1, 0).map { level ->
    MenuEntry.Item(
        "flag",
        priorityName(level),
        MenuAction.SetPriority(task, level),
        checked = level == current,
        hint = if (level > 0) "!".repeat(level) else null,
    )
}

/** The rename dialog's title for a room. */
fun renameTitle(room: RoomKey): String = when (room) {
    is RoomKey.Collection -> "Rename collection"
    else -> "Rename list"
}

