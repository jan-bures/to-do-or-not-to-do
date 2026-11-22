package app.todoornot.ui

import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.verticalScroll
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.platform.LocalFocusManager
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.selected
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.dp
import app.todoornot.core.DestinationGroup
import app.todoornot.design.Glyph
import app.todoornot.design.Icon
import app.todoornot.design.LocalDesign
import app.todoornot.design.Tokens
import app.todoornot.design.style
import app.todoornot.model.Actions
import app.todoornot.model.MenuAction
import app.todoornot.model.UiState
import app.todoornot.model.priorityMenu
import app.todoornot.model.taskMenu

private enum class Picker { ACTIONS, PRIORITY, LIST }

/** Anchor shared by the task row's long press and the detail header's overflow button. */
@Composable
fun TaskActionsAnchor(
    state: UiState, actions: Actions, id: String, done: Boolean, myDay: Boolean, priority: Int,
    row: Boolean, modifier: Modifier = Modifier, enabled: Boolean = true, content: @Composable (open: () -> Unit) -> Unit,
) {
    var picker by remember(id, state.generation, enabled) { mutableStateOf<Picker?>(null) }
    val focus = LocalFocusManager.current
    Box(modifier) {
        content {
            if (enabled) {
                focus.clearFocus()
                picker = Picker.ACTIONS
            }
        }
        when (picker) {
            Picker.ACTIONS -> MenuPopup(taskMenu(state.room, id, done, myDay, priority), { a ->
                when (a) {
                    is MenuAction.Done -> actions.setDone(id, a.on, state.generation, row = row)
                    is MenuAction.MyDay -> actions.setMyDay(id, a.on)
                    is MenuAction.Important -> actions.setImportant(id, a.on)
                    MenuAction.PickPriority -> picker = Picker.PRIORITY
                    MenuAction.PickList -> picker = Picker.LIST
                    MenuAction.DeleteTask -> actions.deleteTask(id)
                    is MenuAction.ReorderTask -> a.neighbour?.let { actions.reorderTask(a.list, id, it, a.by) }
                    else -> Unit
                }
            }, onDismiss = { picker = null }, start = row, title = "Task actions")
            Picker.PRIORITY -> MenuPopup(priorityMenu(id, priority), { a ->
                if (a is MenuAction.SetPriority) actions.setPriority(a.task, a.marks)
            }, onDismiss = { picker = null }, start = row, title = "Priority")
            Picker.LIST -> OverlayDialog(onDismiss = { picker = null }) { overlay ->
                var filter by rememberSaveable(id) { mutableStateOf("") }
                // Never offer another query's results. Re-read on sync too: deleted lists go away,
                // and a renamed destination appears under its new name.
                var groups by remember(id, filter) { mutableStateOf<List<DestinationGroup>?>(null) }
                LaunchedEffect(state, filter) { groups = actions.moveDestinations(id, filter) }
                MoveTaskCard(filter, { filter = it }, groups, { list -> overlay.choose { actions.moveTaskTo(id, list) } }, overlay::close)
            }
            null -> Unit
        }
    }
}

/** Filter stays in place above a scrollable list, even with the keyboard open. */
@Composable
fun MoveTaskCard(filter: String, onFilter: (String) -> Unit, groups: List<DestinationGroup>?, onChoose: (String) -> Unit, cancel: () -> Unit) {
    val design = LocalDesign.current
    val r = design.room
    val s = Tokens.Space
    DialogCard("Move to…", Modifier.imePadding()) {
        Label("Move to…", Tokens.Type.DETAIL_TITLE_PHONE, r.ink)
        BasicTextField(
            value = filter, onValueChange = onFilter, singleLine = true,
            modifier = Modifier.fillMaxWidth().border(Tokens.Elevation.HAIRLINE.dp, r.control, RoundedCornerShape(Tokens.Radius.MD.dp))
                .semantics { contentDescription = "Filter lists" },
            textStyle = Tokens.Type.BODY.style(r.ink), cursorBrush = SolidColor(r.accent),
            decorationBox = { inner ->
                Box(Modifier.heightIn(min = Tokens.Metrics.Phone.TOUCH.dp).padding(horizontal = s.S3.dp), contentAlignment = Alignment.CenterStart) {
                    if (filter.isEmpty()) Label("Filter lists", Tokens.Type.BODY, r.ink3)
                    inner()
                }
            },
        )
        Column(Modifier.weight(1f, fill = false).verticalScroll(rememberScrollState())) {
            if (groups != null && groups.isEmpty()) Label("No list matches “${filter.trim()}”.", Tokens.Type.BODY, r.ink3, maxLines = 2)
            groups.orEmpty().forEach { group ->
                group.heading?.let { Label(it, Tokens.Type.CAPS_LABEL, r.ink3, Modifier.padding(vertical = s.S2.dp)) }
                group.lists.forEach { dest ->
                    Row(Modifier.fillMaxWidth().heightIn(min = Tokens.Metrics.Phone.TOUCH.dp).clickable { onChoose(dest.id) }
                        .semantics { selected = dest.current }.hairline(r.line),
                        horizontalArrangement = Arrangement.spacedBy(s.S3.dp), verticalAlignment = Alignment.CenterVertically) {
                        Glyph(dest.glyph, design.list(dest.hue).glyph)
                        Label(dest.name, Tokens.Type.NAV_SUB, r.ink, Modifier.weight(1f))
                        if (dest.current) Icon("check", r.accentStrong, size = Tokens.Metrics.Component.ICON_SMALL.dp)
                    }
                }
            }
        }
        Button("Cancel", primary = false, onClick = cancel)
    }
}
