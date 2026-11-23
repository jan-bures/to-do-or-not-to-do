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
import androidx.compose.ui.focus.focusProperties
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.focus.onFocusChanged
import androidx.compose.ui.input.key.Key
import androidx.compose.ui.input.key.KeyEventType
import androidx.compose.ui.input.key.key
import androidx.compose.ui.input.key.onPreviewKeyEvent
import androidx.compose.ui.input.key.type
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
import app.todoornot.core.calendarMoveDays
import app.todoornot.core.calendarRepeatClearNote
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
                            CalendarPopup(opened, preview.due, onChoose = ::chooseDate, onDismiss = { calendarNow = null })
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

/** The same anchored date picker, whether a choice edits a draft or saves a task. */
@Composable
internal fun CalendarPopup(now: LocalNow, date: String?, onChoose: (String?) -> Unit, onDismiss: () -> Unit, repeats: Boolean = false) {
    val selected = remember { date }
    var cursor by remember { mutableStateOf<String?>(null) }
    val month = remember(now, cursor) { quickAddMonth(now, selected, cursor) }
    AnchoredPopup(onDismiss = onDismiss) { overlay ->
        DateCalendar(month, selected, { day -> overlay.choose { onChoose(day) } }, { cursor = it },
            Modifier.padding(S.S2.dp), dismiss = overlay::close, repeats = repeats)
    }
}

/** The calendar card at rest, also used by the Paparazzi gallery. */
@Composable
internal fun DateCalendar(
    month: QuickAddMonth,
    selectedDate: String?,
    choose: (String?) -> Unit,
    shift: (String) -> Unit,
    modifier: Modifier = Modifier,
    dismiss: (() -> Unit)? = null,
    repeats: Boolean = false,
) {
    val design = LocalDesign.current
    val r = design.room
    val dayFocus = remember { FocusRequester() }
    // A single day is in Tab order. Keyboard navigation follows it even when the grid changes month.
    var focusDay by remember { mutableStateOf(if (dismiss != null) month.cursor else null) }
    LaunchedEffect(month.cursor, focusDay) {
        if (focusDay == month.cursor) {
            dayFocus.requestFocus()
            focusDay = null
        }
    }
    Column(
        modifier.width((7 * P.TOUCH + 2 * S.S2).dp).overlay(RoundedCornerShape(Tokens.Radius.LG.dp), design)
            .verticalScroll(rememberScrollState()).padding(S.S2.dp).semantics { paneTitle = "Choose due date" }
            .onPreviewKeyEvent { event ->
                if (event.key != Key.Escape || dismiss == null) false else {
                    if (event.type == KeyEventType.KeyUp) dismiss()
                    true
                }
            },
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
                    if (cell == null) {
                        Box(Modifier.weight(1f).heightIn(min = P.TOUCH.dp))
                    } else {
                        val active = cell.date == month.cursor
                        val navigation = Modifier.focusProperties { canFocus = active }
                            .then(if (active) Modifier.focusRequester(dayFocus) else Modifier)
                            .onPreviewKeyEvent { event ->
                                val next = when (event.key) {
                                    Key.DirectionLeft -> -1
                                    Key.DirectionRight -> 1
                                    Key.DirectionUp -> -7
                                    Key.DirectionDown -> 7
                                    Key.PageUp, Key.PageDown -> 0
                                    else -> return@onPreviewKeyEvent false
                                }
                                if (event.type == KeyEventType.KeyDown) {
                                    val date = when (event.key) {
                                        Key.PageUp -> month.previous
                                        Key.PageDown -> month.next
                                        else -> calendarMoveDays(cell.date, next)
                                    }
                                    if (date != null && date != cell.date) {
                                        focusDay = date
                                        shift(date)
                                    }
                                }
                                true
                            }
                        CalendarCell(cell.day, cell.label, cell.selected, cell.today, true, Modifier.weight(1f).then(navigation)) { choose(cell.date) }
                    }
                }
            }
        }
        if (repeats) {
            val note = remember { calendarRepeatClearNote() }
            Label(note, Tokens.Type.META_PHONE, r.ink3,
                Modifier.fillMaxWidth().padding(top = S.S1.dp).hairline(r.line, top = true).padding(top = S.S2.dp),
                maxLines = Int.MAX_VALUE)
        }
    }
}

@Composable
private fun CalendarCell(label: String, description: String, selected: Boolean, today: Boolean, day: Boolean, modifier: Modifier, choose: () -> Unit) {
    val r = LocalDesign.current.room
    val shape = RoundedCornerShape(Tokens.Radius.SM.dp)
    var focused by remember { mutableStateOf(false) }
    Box(
        modifier.heightIn(min = P.TOUCH.dp).clip(shape)
            .onFocusChanged { focused = it.isFocused }
            .then(if (selected) Modifier.background(r.wash) else Modifier)
            .then(if (focused || today) Modifier.border(Tokens.Elevation.HAIRLINE.dp, if (focused) r.accent else r.control, shape) else Modifier)
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
