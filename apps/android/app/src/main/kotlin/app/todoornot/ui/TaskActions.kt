package app.todoornot.ui

import androidx.compose.foundation.background
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
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.verticalScroll
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.platform.LocalFocusManager
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.selected
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.dp
import app.todoornot.core.DestinationGroup
import app.todoornot.core.MergeCandidate
import app.todoornot.core.mergeChangedText
import app.todoornot.core.mergeEmptyText
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
import kotlinx.coroutines.launch

private enum class Picker { ACTIONS, PRIORITY, LIST, MERGE }

@Composable
internal fun TaskActionsSheet(state: UiState, actions: Actions, detail: app.todoornot.core.TaskDetail, close: () -> Unit) {
    val id = detail.id
    val priority = detail.priority.toInt()
    var picker by rememberSaveable(id, detail.list, state.generation) { mutableStateOf(Picker.ACTIONS) }
    val focus = LocalFocusManager.current
    fun back() { focus.clearFocus(); picker = Picker.ACTIONS }
    BottomSheet("Task actions", close) { overlay ->
        androidx.activity.compose.BackHandler(picker != Picker.ACTIONS) { back() }
        SheetPages(picker) { page ->
            when (page) {
                Picker.ACTIONS -> ActionSheetContent(taskMenu(state.room, id, detail.done, detail.myDay, priority), "Task actions") { a ->
                    if (overlay.open) when (a) {
                        MenuAction.PickPriority -> picker = Picker.PRIORITY
                        MenuAction.PickList -> picker = Picker.LIST
                        MenuAction.PickMerge -> picker = Picker.MERGE
                        else -> overlay.choose {
                            when (a) {
                                is MenuAction.Done -> actions.setDone(id, a.on, state.generation, row = false)
                                is MenuAction.MyDay -> actions.setMyDay(id, a.on)
                                is MenuAction.Important -> actions.setImportant(id, a.on)
                                MenuAction.DeleteTask -> actions.deleteTask(id)
                                is MenuAction.ReorderTask -> a.neighbour?.let { actions.reorderTask(a.list, id, it, a.by) }
                                else -> Unit
                            }
                        }
                    }
                }
                Picker.PRIORITY -> ActionSheetContent(priorityMenu(id, priority), "Priority") { a ->
                    if (a is MenuAction.SetPriority) overlay.choose { actions.setPriority(a.task, a.marks) }
                }
                Picker.LIST -> {
                    var filter by rememberSaveable(id) { mutableStateOf("") }
                    var groups by remember(id, filter) { mutableStateOf<List<DestinationGroup>?>(null) }
                    LaunchedEffect(state.home, filter) { groups = actions.moveDestinations(id, filter) }
                    MoveTaskCard(filter, { filter = it }, groups, { to ->
                        overlay.choose { actions.moveTaskTo(id, detail.list, to) }
                    }, ::back, inSheet = true)
                }
                Picker.MERGE -> MergeTaskPages(state, actions, id, overlay, ::back)
            }
        }
    }
}

/** Filter stays in place above a scrollable list, even with the keyboard open. */
@Composable
fun MoveTaskCard(filter: String, onFilter: (String) -> Unit, groups: List<DestinationGroup>?, onChoose: (String) -> Unit, cancel: () -> Unit, focus: Boolean = false, inSheet: Boolean = false) {
    val design = LocalDesign.current
    val r = design.room
    val s = Tokens.Space
    TaskPickerCard("Move to…", "Filter lists", filter, onFilter, cancel, focus, inSheet) { body ->
        Column(body.verticalScroll(rememberScrollState())) {
            if (groups != null && groups.isEmpty()) Label("No list matches “${filter.trim()}”.", Tokens.Type.BODY, r.ink3, maxLines = 2)
            groups.orEmpty().forEach { group ->
                group.heading?.let { Label(it, Tokens.Type.CAPS_LABEL, r.ink3, Modifier.padding(vertical = s.S2.dp)) }
                group.lists.forEach { dest ->
                    Row(Modifier.fillMaxWidth().heightIn(min = Tokens.Metrics.Phone.TOUCH.dp)
                        .then(if (dest.current) Modifier.background(r.wash) else Modifier)
                        .clickable { onChoose(dest.id) }
                        .semantics { selected = dest.current }.hairline(r.line),
                        horizontalArrangement = Arrangement.spacedBy(s.S3.dp), verticalAlignment = Alignment.CenterVertically) {
                        Glyph(dest.glyph, design.list(dest.hue).glyph)
                        Label(dest.name, Tokens.Type.NAV_SUB, if (dest.current) r.accentStrong else r.ink, Modifier.weight(1f),
                            weight = if (dest.current) Tokens.Type.BUTTON.weight else null)
                        if (dest.current) Icon("check", r.accentStrong, size = Tokens.Metrics.Component.ICON_SMALL.dp)
                    }
                }
            }
        }
    }
}

@Composable
private fun TaskPickerCard(
    title: String, prompt: String, filter: String, onFilter: (String) -> Unit,
    cancel: () -> Unit, focus: Boolean, inSheet: Boolean = false, content: @Composable (Modifier) -> Unit,
) {
    // Open ready to type, as the desktop's picker does. The gallery leaves it unfocused: no cursor.
    val field = remember { FocusRequester() }
    if (focus) LaunchedEffect(Unit) { runCatching { field.requestFocus() } }
    val r = LocalDesign.current.room
    val s = Tokens.Space
    val body: @Composable androidx.compose.foundation.layout.ColumnScope.() -> Unit = {
        Label(title, Tokens.Type.DETAIL_TITLE_PHONE, r.ink)
        if (inSheet) content(Modifier.weight(1f, fill = false))
        BasicTextField(
            value = filter, onValueChange = onFilter, singleLine = true,
            modifier = Modifier.fillMaxWidth().border(Tokens.Elevation.HAIRLINE.dp, r.control, RoundedCornerShape(Tokens.Radius.MD.dp))
                .focusRequester(field).semantics { contentDescription = prompt },
            textStyle = Tokens.Type.BODY.style(r.ink), cursorBrush = SolidColor(r.accent),
            decorationBox = { inner ->
                Box(Modifier.heightIn(min = Tokens.Metrics.Phone.TOUCH.dp).padding(horizontal = s.S3.dp), contentAlignment = Alignment.CenterStart) {
                    if (filter.isEmpty()) Label(prompt, Tokens.Type.BODY, r.ink3)
                    inner()
                }
            },
        )
        if (!inSheet) content(Modifier.weight(1f, fill = false))
        Button("Cancel", primary = false, onClick = cancel)
    }
    if (inSheet) Column(Modifier.fillMaxWidth().padding(s.S4.dp), verticalArrangement = Arrangement.spacedBy(s.S3.dp), content = body)
    else DialogCard(title, Modifier.imePadding(), content = body)
}

@Composable
private fun MergeTaskPages(state: UiState, actions: Actions, source: String, overlay: OverlayState, back: () -> Unit) {
    var filter by rememberSaveable(source) { mutableStateOf("") }
    var candidates by remember(source, filter) { mutableStateOf<List<MergeCandidate>?>(null) }
    var asked by remember(source) { mutableStateOf<Pair<String, String>?>(null) }
    var busy by remember(source) { mutableStateOf(false) }
    var changed by remember(source) { mutableStateOf(false) }
    val scope = rememberCoroutineScope()
    val focus = LocalFocusManager.current
    androidx.activity.compose.BackHandler(asked != null && !busy) { asked = null }
    SheetPages(asked != null) { confirming ->
    val showing = asked
    if (!confirming) {
        LaunchedEffect(state, filter) { candidates = actions.mergeCandidates(source, filter) }
        MergeTaskCard(filter, { filter = it; changed = false }, candidates, { target ->
            if (overlay.open && !busy) {
                busy = true
                scope.launch {
                    val question = actions.mergeQuestion(source, target)
                    if (overlay.open && question != null) {
                        focus.clearFocus()
                        asked = target to question
                        changed = false
                    } else {
                        changed = true
                    }
                    busy = false
                }
            }
        }, back, changed = changed, inSheet = true)
    } else if (showing != null) {
        MergeConfirmCard(showing.second, onMerge = {
            if (overlay.open && !busy) {
                busy = true
                actions.mergeTasks(source, showing.first, showing.second) { again ->
                    if (again == null) overlay.close() else {
                        asked = showing.first to again
                        changed = true
                    }
                    busy = false
                }
            }
        }, cancel = { if (!busy) asked = null }, busy = busy, changed = changed, inSheet = true)
    }
    }
}

@Composable
internal fun MergeTaskCard(
    filter: String, onFilter: (String) -> Unit, choices: List<MergeCandidate>?,
    onChoose: (String) -> Unit, cancel: () -> Unit, focus: Boolean = false, changed: Boolean = false, inSheet: Boolean = false,
) {
    val r = LocalDesign.current.room
    val s = Tokens.Space
    TaskPickerCard("Merge into…", "Filter open tasks", filter, onFilter, cancel, focus, inSheet) { body ->
        LazyColumn(body) {
            if (changed) item { Label(mergeChangedText(), Tokens.Type.BODY, r.dangerInk, maxLines = 4) }
            if (choices?.isEmpty() == true) item { Label(mergeEmptyText(filter.isNotBlank()), Tokens.Type.BODY, r.ink3) }
            items(choices.orEmpty(), key = { it.id }) { choice ->
                Row(Modifier.fillMaxWidth().heightIn(min = Tokens.Metrics.Phone.TOUCH.dp)
                    .clickable { onChoose(choice.id) }.hairline(r.line)
                    .padding(horizontal = s.S2.dp, vertical = s.S1.dp),
                    horizontalArrangement = Arrangement.spacedBy(s.S3.dp), verticalAlignment = Alignment.CenterVertically) {
                    Icon("chevron-right", r.ink3, size = Tokens.Metrics.Component.ICON_SMALL.dp)
                    Column(Modifier.weight(1f)) {
                        Label(choice.title, Tokens.Type.NAV_SUB, r.ink, maxLines = 1)
                        Label(choice.location, Tokens.Type.META_PHONE, r.ink3, maxLines = 1)
                    }
                }
            }
        }
    }
}

@Composable
internal fun MergeConfirmCard(
    question: String, onMerge: () -> Unit, cancel: () -> Unit,
    focus: Boolean = true, busy: Boolean = false, changed: Boolean = false, inSheet: Boolean = false,
) {
    val r = LocalDesign.current.room
    val initial = remember { FocusRequester() }
    if (focus) LaunchedEffect(question) { runCatching { initial.requestFocus() } }
    val content: @Composable androidx.compose.foundation.layout.ColumnScope.() -> Unit = {
        Label("Merge tasks?", Tokens.Type.DETAIL_TITLE_PHONE, r.ink)
        Column(Modifier.weight(1f, fill = false).verticalScroll(rememberScrollState())) {
            Label(question, Tokens.Type.NOTES, r.ink2, maxLines = Int.MAX_VALUE)
            if (changed) Label(mergeChangedText(), Tokens.Type.META_PHONE, r.dangerInk, maxLines = 4)
        }
        Button("Cancel", primary = false, modifier = Modifier.focusRequester(initial), onClick = cancel)
        Button("Merge tasks", primary = true, enabled = !busy, onClick = onMerge)
    }
    if (inSheet) Column(Modifier.fillMaxWidth().padding(Tokens.Space.S4.dp),
        verticalArrangement = Arrangement.spacedBy(Tokens.Space.S3.dp), content = content)
    else DialogCard("Merge tasks?", content = content)
}
