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
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.CircleShape
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
import androidx.compose.ui.draw.drawBehind
import androidx.compose.ui.geometry.CornerRadius
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.platform.LocalSoftwareKeyboardController
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
import app.todoornot.core.QuickAddDestination
import app.todoornot.core.RoomKey
import app.todoornot.core.QuickAddDraft
import app.todoornot.core.QuickAddMonth
import app.todoornot.core.calendarMoveDays
import app.todoornot.core.calendarRepeatClearNote
import app.todoornot.core.quickAddMonth
import app.todoornot.design.Icon
import app.todoornot.design.LocalDesign
import app.todoornot.design.Tokens
import app.todoornot.design.cubic
import app.todoornot.design.style
import app.todoornot.model.Actions
import app.todoornot.model.MenuAction
import app.todoornot.model.priorityMenu
import app.todoornot.model.priorityName

import app.todoornot.core.ListFeatures
import app.todoornot.core.OpenStage
import app.todoornot.core.WorkflowQuickAddDraft
import app.todoornot.core.WorkflowQuickAddPick
import app.todoornot.core.workflowQuickAddEdit
import app.todoornot.core.workflowQuickAddPick
import app.todoornot.core.workflowQuickAddPreview
import app.todoornot.model.ChecklistFeatures
import app.todoornot.model.label
import app.todoornot.model.openStage
import app.todoornot.model.workflowStage

private val S = Tokens.Space
private val P = Tokens.Metrics.Phone
private val C = Tokens.Metrics.Component
private val M = Tokens.Motion

private val draftSaver = listSaver<WorkflowQuickAddDraft, Any>(
    save = { listOf(it.base.text, it.base.pickedDue.orEmpty(), it.base.clearedDue, it.base.pickedPriority?.toInt() ?: -1,
        it.startDate.orEmpty(), it.status?.name.orEmpty(), it.base.pickedList.orEmpty()) },
    restore = { WorkflowQuickAddDraft(
        QuickAddDraft(it[0] as String, (it[1] as String).ifEmpty { null }, it[2] as Boolean, (it[3] as Int).takeIf { n -> n >= 0 }?.toUByte(),
            (it.getOrNull(6) as? String)?.ifEmpty { null }),
        (it[4] as String).ifEmpty { null }, OpenStage.entries.firstOrNull { stage -> stage.name == it[5] },
    ) },
)

/** The room's composer, docked above the keyboard. Only submitting sends a command. */
@Composable
internal fun Composer(placeholder: String, draft: String, keyboard: Boolean, actions: Actions, focus: (() -> Unit)?, docked: Boolean = false,
    features: ListFeatures = ChecklistFeatures, room: RoomKey? = null, replicaRevision: Long = 0, initialStage: OpenStage? = null, onSubmit: () -> Unit = {},
) {
    val r = LocalDesign.current.room
    val requester = remember { FocusRequester() }
    val ime = LocalSoftwareKeyboardController.current
    var fieldFocused by remember { mutableStateOf(false) }
    LaunchedEffect(focus != null) {
        if (focus != null) {
            if (requester.requestFocus()) ime?.show()
            focus()
        }
    }
    var dismissed by rememberSaveable(room, draft, initialStage) { mutableStateOf(false) }
    var pending by rememberSaveable(room, draft, initialStage, stateSaver = draftSaver) {
        mutableStateOf(WorkflowQuickAddDraft(QuickAddDraft(draft, null, false, null), null, initialStage))
    }
    val context = remember(actions, room, pending, dismissed, replicaRevision) {
        room?.let { actions.composerContext(it, pending, !dismissed) }
    }
    val flags = context?.features ?: features
    val destination = context?.destination
    val accepted = pending.base.pickedList != null
    fun clearDestination() {
        dismissed = true
        pending = pending.copy(base = pending.base.copy(pickedList = null))
    }
    LaunchedEffect(accepted, destination) {
        if (accepted && destination == null) clearDestination()
    }
    val preview = remember(pending, flags) { workflowQuickAddPreview(pending, actions.composerNow(), flags) }
    val priority = preview.priority?.toInt() ?: 0
    val stage = preview.status ?: OpenStage.TODO
    val shape = RoundedCornerShape(Tokens.Radius.COMPOSER.dp)
    var calendarNow by remember { mutableStateOf<LocalNow?>(null) }
    var priorityOpen by remember { mutableStateOf(false) }
    var statusOpen by remember { mutableStateOf(false) }
    fun pick(choice: WorkflowQuickAddPick) {
        pending = workflowQuickAddPick(pending, choice, actions.composerNow(), flags)
    }
    fun chooseDate(date: String?) { pick(WorkflowQuickAddPick.Due(date)) }
    fun choosePriority(level: Int) { pick(WorkflowQuickAddPick.Priority(level.toUByte())) }
    fun submit() {
        if (preview.title.isBlank()) return
        if (flags.workflow || flags.timeline) actions.addWorkflow(pending) else actions.add(pending.base)
        dismissed = false
        pending = WorkflowQuickAddDraft(QuickAddDraft("", null, false, null), null, initialStage)
        onSubmit()
    }
    Box(
        Modifier.fillMaxWidth().background(r.pane).hairline(r.line, top = true)
            .then(if (docked) Modifier.padding(horizontal = S.S3.dp, vertical = S.S1.dp)
                else Modifier.navigationBarsPadding().imePadding()
                    .padding(start = S.S4.dp, end = S.S4.dp, top = S.S3.dp, bottom = if (keyboard) S.S3.dp else S.S4.dp)),
    ) {
        val focusRing = if (fieldFocused) Modifier.drawBehind {
            val ring = Tokens.Elevation.FOCUS_RING.dp.toPx()
            drawRoundRect(r.tint, Offset(-ring, -ring), Size(size.width + ring * 2, size.height + ring * 2),
                CornerRadius(Tokens.Radius.COMPOSER.dp.toPx() + ring))
        } else Modifier
        Column(Modifier.fillMaxWidth().then(focusRing).background(r.raised, shape)
            .border(Tokens.Elevation.HAIRLINE.dp, if (fieldFocused) r.accent else r.control, shape)) {
            Row(
                Modifier.fillMaxWidth().heightIn(min = P.COMPOSER.dp).padding(start = S.S3.dp, end = S.S1.dp),
                verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.spacedBy(S.S2.dp),
            ) {
                Icon("plus", r.accentStrong)
                BasicTextField(
                    value = pending.base.text,
                    onValueChange = {
                        if (it.isBlank()) {
                            dismissed = false
                            pending = pending.copy(base = pending.base.copy(pickedList = null))
                        }
                        pending = workflowQuickAddEdit(pending, it, actions.composerNow(), flags)
                    },
                    modifier = Modifier.weight(1f).focusRequester(requester).onFocusChanged { fieldFocused = it.isFocused }
                        .semantics { contentDescription = placeholder },
                    textStyle = Tokens.Type.COMPOSER.style(r.ink),
                    singleLine = true,
                    cursorBrush = SolidColor(r.accent),
                    keyboardOptions = KeyboardOptions(capitalization = KeyboardCapitalization.Sentences, imeAction = ImeAction.Done),
                    keyboardActions = KeyboardActions(onDone = { submit() }),
                    decorationBox = { inner ->
                        Box(Modifier.heightIn(min = P.COMPOSER.dp), contentAlignment = Alignment.CenterStart) {
                            if (pending.base.text.isEmpty()) Label(placeholder, Tokens.Type.COMPOSER, r.ink3)
                            inner()
                        }
                    },
                )
                Row {
                    if (flags.workflow) Box {
                        val mark = stage.workflowStage()
                        IconButton(mark.icon, "Choose status: ${mark.label}", mark.ink(), expanded = statusOpen) { statusOpen = true }
                        if (statusOpen) AnchoredPopup(onDismiss = { statusOpen = false }, above = true) { overlay ->
                            Column(Modifier.padding(S.S2.dp).widthIn(min = P.INNER_LIST.dp / 2)
                                .overlay(RoundedCornerShape(Tokens.Radius.LG.dp), LocalDesign.current).padding(S.S2.dp)) {
                                Caps("Choose status")
                                StatusOptions(mark, includeDone = false) { picked ->
                                    overlay.choose { pick(WorkflowQuickAddPick.Status(picked.openStage())) }
                                }
                            }
                        }
                    }
                    Box {
                        IconButton("calendar", "Choose due date", if (preview.dateLabel != null) r.ink else r.ink3, expanded = calendarNow != null) {
                            calendarNow = actions.composerNow()
                        }
                        calendarNow?.let { opened ->
                            CalendarPopup(opened, preview.due, onChoose = ::chooseDate, onDismiss = { calendarNow = null },
                                start = preview.startDate,
                                onStart = if (flags.timeline) { date -> pick(WorkflowQuickAddPick.Start(date)) } else null)
                        }
                    }
                    Box {
                        IconButton("flag", "Choose priority", if (priority > 0) r.ink else r.ink3, expanded = priorityOpen) { priorityOpen = true }
                        if (priorityOpen) {
                            MenuPopup(priorityMenu(priority) { MenuAction.QuickPriority(it) }, { action ->
                                if (action is MenuAction.QuickPriority) choosePriority(action.marks)
                            }, { priorityOpen = false }, title = "Choose priority", above = true)
                        }
                    }
                }
            }
            AnimatedVisibility(
                visible = preview.dateLabel != null || priority > 0 || stage != OpenStage.TODO || destination != null,
                enter = fadeIn(tween(M.OVERLAY_IN, easing = M.EASE_ENTER.cubic())),
                exit = fadeOut(tween(M.OVERLAY_OUT, easing = M.EASE_EXIT.cubic())),
            ) {
                androidx.compose.foundation.layout.FlowRow(Modifier.padding(horizontal = S.S3.dp), horizontalArrangement = Arrangement.spacedBy(S.S2.dp)) {
                    preview.dateLabel?.let { label -> QuickChip(label, "Remove dates: $label", date = true) {
                        if (flags.timeline && preview.startDate != null) pick(WorkflowQuickAddPick.Start(null))
                        if (preview.due != null) chooseDate(null)
                    } }
                    if (priority > 0) QuickChip("!".repeat(priority), "Remove priority: ${priorityName(priority)}", date = false) { choosePriority(0) }
                    destination?.let { list ->
                        DestinationChip(list, accepted, { pending = pending.copy(base = pending.base.copy(pickedList = list.id)) }, ::clearDestination)
                    }
                    if (stage != OpenStage.TODO) {
                        val mark = stage.workflowStage()
                        QuickChip(mark.label, "Remove status: ${mark.label}", date = false, icon = mark.icon) {
                            pick(WorkflowQuickAddPick.Status(null))
                        }
                    }
                }
            }
        }
    }
}

@Composable
private fun DestinationChip(list: QuickAddDestination, accepted: Boolean, accept: () -> Unit, dismiss: () -> Unit) {
    val design = LocalDesign.current
    val r = design.room
    val shape = RoundedCornerShape(Tokens.Radius.MD.dp)
    val remove = if (accepted) "Remove destination: ${list.name}" else "Dismiss list suggestion: ${list.name}"
    Row(verticalAlignment = Alignment.CenterVertically, modifier = Modifier.drawBehind {
        val height = (S.S6.dp).toPx()
        drawRoundRect(if (accepted) r.wash else r.raised, Offset(0f, (size.height - height) / 2),
            Size(size.width, height), CornerRadius(Tokens.Radius.MD.dp.toPx()))
        if (!accepted) {
            val line = Tokens.Elevation.HAIRLINE.dp.toPx()
            drawRoundRect(r.line, Offset(line / 2, (size.height - height + line) / 2),
                Size(size.width - line, height - line), CornerRadius(Tokens.Radius.MD.dp.toPx()),
                style = androidx.compose.ui.graphics.drawscope.Stroke(line))
        }
    }) {
        Row(
            Modifier.heightIn(min = P.TOUCH.dp).clip(shape)
                .then(if (accepted) Modifier else Modifier.widthIn(min = P.TOUCH.dp).clickable(role = Role.Button, onClick = accept))
                .padding(start = S.S2.dp),
            horizontalArrangement = Arrangement.spacedBy(S.S1.dp), verticalAlignment = Alignment.CenterVertically,
        ) {
            Box(Modifier.size(S.S2.dp).background(design.list(list.hue).swatch, CircleShape))
            Label(if (accepted) list.name else "In ${list.name}?", Tokens.Type.COUNT, r.ink,
                modifier = Modifier.widthIn(max = P.CHIP_MAX.dp), weight = Tokens.Type.DUE.weight)
        }
        IconButton("x", remove, r.ink, onClick = dismiss)
    }
}

@Composable
private fun QuickChip(label: String, description: String, date: Boolean, icon: String? = null, onClick: () -> Unit) {
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
            (icon ?: if (date) "calendar" else null)?.let { Icon(it, ink, size = C.ICON_SMALL.dp) }
            Label(label, Tokens.Type.COUNT, ink, weight = Tokens.Type.DUE.weight)
            Icon("x", ink, size = C.ICON_SMALL.dp)
        }
    }
}

/** Quick-add calendar opens above its button, with the shared presets nearest the dock. */
@Composable
internal fun CalendarPopup(now: LocalNow, date: String?, onChoose: (String?) -> Unit, onDismiss: () -> Unit, repeats: Boolean = false,
    start: String? = null, onStart: ((String?) -> Unit)? = null,
) {
    var pickingStart by remember { mutableStateOf(false) }
    val selected = if (pickingStart) start else date
    var cursor by remember(pickingStart) { mutableStateOf<String?>(null) }
    val month = remember(now, selected, cursor) { quickAddMonth(now, selected, cursor) }
    AnchoredPopup(onDismiss = onDismiss, above = true) { overlay ->
        DateCalendar(month, selected, { day -> overlay.choose { if (pickingStart) onStart?.invoke(day) else onChoose(day) } }, { cursor = it },
            Modifier.padding(S.S2.dp), dismiss = overlay::close, repeats = repeats, starts = pickingStart,
            dateMode = if (onStart != null) {{
                Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.SpaceEvenly) {
                    Button("Starts", pickingStart, Modifier.weight(1f)) { pickingStart = true }
                    Button("Due", !pickingStart, Modifier.weight(1f)) { pickingStart = false }
                }
            }} else null)
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
    inSheet: Boolean = false,
    focusCalendar: Boolean = dismiss != null,
    starts: Boolean = false,
    dateMode: (@Composable () -> Unit)? = null,
) {
    val design = LocalDesign.current
    val r = design.room
    val dayFocus = remember { FocusRequester() }
    // A single day is in Tab order. Keyboard navigation follows it even when the grid changes month.
    var focusDay by remember { mutableStateOf(if (focusCalendar) month.cursor else null) }
    LaunchedEffect(month.cursor, focusDay) {
        if (focusDay == month.cursor) {
            dayFocus.requestFocus()
            focusDay = null
        }
    }
    Column(
        modifier.then(if (inSheet) Modifier.fillMaxWidth() else Modifier.width((7 * P.TOUCH + 2 * S.S2).dp)
            .overlay(RoundedCornerShape(Tokens.Radius.LG.dp), design))
            .padding(S.S2.dp).semantics { paneTitle = if (starts) "Choose start date" else "Choose due date" }
            .onPreviewKeyEvent { event ->
                if (event.key != Key.Escape || dismiss == null) false else {
                    if (event.type == KeyEventType.KeyUp) dismiss()
                    true
                }
            },
    ) {
        if (inSheet) Label(if (starts) "Start date" else "Due date", Tokens.Type.CAPS_LABEL, r.ink3, Modifier.padding(horizontal = S.S2.dp))
        Column(Modifier.weight(1f, fill = false).verticalScroll(rememberScrollState())) {
        Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) {
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
        }
        Column(Modifier.fillMaxWidth().hairline(r.line, top = true).padding(top = S.S1.dp)) {
            if (repeats) Label(remember { calendarRepeatClearNote() }, Tokens.Type.META_PHONE, r.ink3,
                Modifier.padding(horizontal = S.S2.dp, vertical = S.S1.dp), maxLines = Int.MAX_VALUE)
            dateMode?.invoke()
            month.presets.chunked(2).forEach { row ->
                Row(Modifier.fillMaxWidth()) {
                    row.forEach { preset ->
                        val label = if (starts && preset.date == null) "Clear" else preset.label
                        CalendarCell(label, label, preset.date == selectedDate, false, false, Modifier.weight(1f)) { choose(preset.date) }
                    }
                }
            }
            if (inSheet && dismiss != null) Button("Cancel", false, onClick = dismiss)
        }
    }
}

@Composable
internal fun DueDateSheet(detail: app.todoornot.core.TaskDetail, actions: Actions, close: () -> Unit) {
    val now = remember { actions.composerNow() }
    val selected = rememberSaveable { detail.due.orEmpty() }.ifEmpty { null }
    var cursor by rememberSaveable { mutableStateOf<String?>(null) }
    val month = remember(now, selected, cursor) { quickAddMonth(now, selected, cursor) }
    BottomSheet("Due date", close) { overlay ->
        DateCalendar(month, selected, { day -> overlay.choose { actions.setDue(detail.id, day) } }, { cursor = it },
            dismiss = overlay::close, repeats = detail.repeats, inSheet = true, focusCalendar = !LocalSheetAtRest.current)
    }
}

@Composable
internal fun StartDateSheet(detail: app.todoornot.core.TaskDetail, actions: Actions, close: () -> Unit) {
    val now = remember { actions.composerNow() }
    val selected = rememberSaveable { detail.workflow.startDate.orEmpty() }.ifEmpty { null }
    var cursor by rememberSaveable { mutableStateOf<String?>(null) }
    val month = remember(now, selected, cursor) { quickAddMonth(now, selected, cursor) }
    BottomSheet("Start date", close) { overlay ->
        DateCalendar(month, selected, { day -> overlay.choose { actions.setStartDate(detail.id, day) } }, { cursor = it },
            dismiss = overlay::close, inSheet = true, focusCalendar = !LocalSheetAtRest.current, starts = true)
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
