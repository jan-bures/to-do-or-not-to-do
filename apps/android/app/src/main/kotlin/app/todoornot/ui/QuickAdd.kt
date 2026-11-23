package app.todoornot.ui

import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.core.tween
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
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
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.selection.selectable
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.listSaver
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.paneTitle
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.KeyboardCapitalization
import androidx.compose.ui.unit.dp
import app.todoornot.core.LocalNow
import app.todoornot.core.QuickAddDraft
import app.todoornot.core.QuickAddMonth
import app.todoornot.core.quickAddEdit
import app.todoornot.core.quickAddMonth
import app.todoornot.core.quickAddPickDue
import app.todoornot.core.quickAddPickPriority
import app.todoornot.core.quickAddPreview
import app.todoornot.design.Icon
import app.todoornot.design.LocalDesign
import app.todoornot.design.Tokens
import app.todoornot.design.cubic
import app.todoornot.design.style
import app.todoornot.model.Actions
import app.todoornot.model.MenuAction
import app.todoornot.model.priorityMenu
import app.todoornot.model.priorityName

private val S = Tokens.Space
private val P = Tokens.Metrics.Phone
private val C = Tokens.Metrics.Component
private val M = Tokens.Motion

private val draftSaver = listSaver<QuickAddDraft, Any>(
    save = { listOf(it.text, it.pickedDue.orEmpty(), it.clearedDue, it.pickedPriority?.toInt() ?: -1) },
    restore = { QuickAddDraft(it[0] as String, (it[1] as String).ifEmpty { null }, it[2] as Boolean, (it[3] as Int).takeIf { n -> n >= 0 }?.toUByte()) },
)

/** The room's composer, docked above the keyboard. Only submitting sends a command. */
@Composable
internal fun Composer(placeholder: String, draft: String, keyboard: Boolean, actions: Actions, focus: (() -> Unit)?) {
    val r = LocalDesign.current.room
    val requester = remember { FocusRequester() }
    LaunchedEffect(focus != null) {
        if (focus != null) {
            requester.requestFocus()
            focus()
        }
    }
    var pending by rememberSaveable(draft, stateSaver = draftSaver) { mutableStateOf(QuickAddDraft(draft, null, false, null)) }
    val preview = remember(pending) { quickAddPreview(pending, actions.composerNow()) }
    val priority = preview.priority?.toInt() ?: 0
    val shape = RoundedCornerShape(Tokens.Radius.COMPOSER.dp)
    var calendarNow by remember { mutableStateOf<LocalNow?>(null) }
    var priorityOpen by remember { mutableStateOf(false) }
    fun chooseDate(date: String?) { pending = quickAddPickDue(pending, date, actions.composerNow()) }
    fun choosePriority(level: Int) { pending = quickAddPickPriority(pending, level.toUByte(), actions.composerNow()) }
    fun submit() {
        if (pending.text.isBlank()) return
        actions.add(pending)
        pending = QuickAddDraft("", null, false, null)
    }
    Box(
        Modifier.fillMaxWidth().background(r.pane).hairline(r.line, top = true)
            .navigationBarsPadding().imePadding()
            .padding(start = S.S4.dp, end = S.S4.dp, top = S.S3.dp, bottom = if (keyboard) S.S3.dp else S.S4.dp),
    ) {
        Column(Modifier.fillMaxWidth().background(r.raised, shape).border(Tokens.Elevation.HAIRLINE.dp, r.control, shape)) {
            Row(
                Modifier.fillMaxWidth().heightIn(min = P.COMPOSER.dp).padding(start = S.S3.dp, end = S.S1.dp),
                verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.spacedBy(S.S2.dp),
            ) {
                Icon("plus", r.accentStrong)
                BasicTextField(
                    value = pending.text,
                    onValueChange = { pending = quickAddEdit(pending, it, actions.composerNow()) },
                    modifier = Modifier.weight(1f).focusRequester(requester).semantics { contentDescription = placeholder },
                    textStyle = Tokens.Type.COMPOSER.style(r.ink),
                    singleLine = true,
                    cursorBrush = SolidColor(r.accent),
                    keyboardOptions = KeyboardOptions(capitalization = KeyboardCapitalization.Sentences, imeAction = ImeAction.Done),
                    keyboardActions = KeyboardActions(onDone = { submit() }),
                    decorationBox = { inner ->
                        Box(contentAlignment = Alignment.CenterStart) {
                            if (pending.text.isEmpty()) Label(placeholder, Tokens.Type.COMPOSER, r.ink3)
                            inner()
                        }
                    },
                )
                Row {
                    Box {
                        IconButton("calendar", "Choose due date", if (preview.due != null) r.ink else r.ink3, expanded = calendarNow != null) {
                            calendarNow = actions.composerNow()
                        }
                        calendarNow?.let { opened ->
                            var cursor by remember(opened) { mutableStateOf<String?>(null) }
                            val month = remember(opened, preview.due, cursor) { quickAddMonth(opened, preview.due, cursor) }
                            AnchoredPopup(onDismiss = { calendarNow = null }) { overlay ->
                                QuickAddCalendar(month, preview.due, { day -> overlay.choose { chooseDate(day) } }, { cursor = it }, Modifier.padding(S.S2.dp))
                            }
                        }
                    }
                    Box {
                        IconButton("flag", "Choose priority", if (priority > 0) r.ink else r.ink3, expanded = priorityOpen) { priorityOpen = true }
                        if (priorityOpen) {
                            MenuPopup(priorityMenu(priority) { MenuAction.QuickPriority(it) }, { action ->
                                if (action is MenuAction.QuickPriority) choosePriority(action.marks)
                            }, { priorityOpen = false }, title = "Choose priority")
                        }
                    }
                }
            }
            AnimatedVisibility(
                visible = preview.dueLabel != null || priority > 0,
                enter = fadeIn(tween(M.OVERLAY_IN, easing = M.EASE_ENTER.cubic())),
                exit = fadeOut(tween(M.OVERLAY_OUT, easing = M.EASE_EXIT.cubic())),
            ) {
                Row(Modifier.padding(horizontal = S.S3.dp), horizontalArrangement = Arrangement.spacedBy(S.S2.dp)) {
                    preview.dueLabel?.let { label -> QuickChip(label, "Remove due date: $label", date = true) { chooseDate(null) } }
                    if (priority > 0) QuickChip("!".repeat(priority), "Remove priority: ${priorityName(priority)}", date = false) { choosePriority(0) }
                }
            }
        }
    }
}

@Composable
private fun QuickChip(label: String, description: String, date: Boolean, onClick: () -> Unit) {
    val r = LocalDesign.current.room
    val ink = if (date) r.accentStrong else r.ink
    val shape = RoundedCornerShape(Tokens.Radius.SM.dp)
    Box(
        Modifier.heightIn(min = P.TOUCH.dp).clip(shape).clickable(role = Role.Button, onClick = onClick)
            .semantics { contentDescription = description },
        contentAlignment = Alignment.Center,
    ) {
        Row(
            Modifier.background(r.wash, shape).padding(horizontal = S.S2.dp, vertical = S.S1.dp),
            horizontalArrangement = Arrangement.spacedBy(S.S1.dp), verticalAlignment = Alignment.CenterVertically,
        ) {
            if (date) Icon("calendar", ink, size = C.ICON_SMALL.dp)
            Label(label, Tokens.Type.COUNT, ink, weight = Tokens.Type.DUE.weight)
            Icon("x", ink, size = C.ICON_SMALL.dp)
        }
    }
}

/** The popup card at rest; used by the real composer and the Paparazzi gallery. */
@Composable
internal fun QuickAddCalendar(
    month: QuickAddMonth,
    selectedDate: String?,
    choose: (String?) -> Unit,
    shift: (String) -> Unit,
    modifier: Modifier = Modifier,
) {
    val design = LocalDesign.current
    val r = design.room
    Column(
        modifier.width((7 * P.TOUCH + 2 * S.S2).dp).overlay(RoundedCornerShape(Tokens.Radius.LG.dp), design)
            .verticalScroll(rememberScrollState()).padding(S.S2.dp).semantics { paneTitle = "Choose due date" },
    ) {
        month.presets.chunked(2).forEach { row ->
            Row(Modifier.fillMaxWidth()) {
                row.forEach { preset ->
                    CalendarCell(preset.label, preset.label, preset.date == selectedDate, false, false, Modifier.weight(1f)) { choose(preset.date) }
                }
            }
        }
        Row(Modifier.fillMaxWidth().hairline(r.line, top = true), verticalAlignment = Alignment.CenterVertically) {
            IconButton("chevron-left", "Previous month", enabled = month.previous != null) { month.previous?.let(shift) }
            Box(Modifier.weight(1f), contentAlignment = Alignment.Center) { Label(month.label, Tokens.Type.BUTTON, r.ink) }
            IconButton("chevron-right", "Next month", enabled = month.next != null) { month.next?.let(shift) }
        }
        Row(Modifier.fillMaxWidth()) {
            month.weekdays.forEach { day ->
                Box(Modifier.weight(1f).padding(vertical = S.S1.dp), contentAlignment = Alignment.Center) { Label(day, Tokens.Type.COUNT, r.ink3) }
            }
        }
        month.rows.forEach { week ->
            Row(Modifier.fillMaxWidth()) {
                week.forEach { cell ->
                    if (cell == null) Box(Modifier.weight(1f).heightIn(min = P.TOUCH.dp))
                    else CalendarCell(cell.day, cell.label, cell.selected, cell.today, true, Modifier.weight(1f)) { choose(cell.date) }
                }
            }
        }
    }
}

@Composable
private fun CalendarCell(label: String, description: String, selected: Boolean, today: Boolean, day: Boolean, modifier: Modifier, choose: () -> Unit) {
    val r = LocalDesign.current.room
    val shape = RoundedCornerShape(Tokens.Radius.SM.dp)
    Box(
        modifier.heightIn(min = P.TOUCH.dp).clip(shape)
            .then(if (selected) Modifier.background(r.wash) else Modifier)
            .then(if (today) Modifier.border(Tokens.Elevation.HAIRLINE.dp, r.control, shape) else Modifier)
            .selectable(selected = selected, role = Role.Button, onClick = choose).semantics { contentDescription = description },
        contentAlignment = Alignment.Center,
    ) {
        Label(label, if (day) Tokens.Type.COUNT else Tokens.Type.BUTTON, if (selected) r.accentStrong else r.ink,
            weight = if (selected) Tokens.Type.BUTTON.weight else null)
        if (selected && !day) {
            Box(Modifier.align(Alignment.CenterEnd).padding(end = S.S2.dp)) {
                Icon("check", r.accentStrong, size = C.ICON_SMALL.dp)
            }
        }
    }
}
