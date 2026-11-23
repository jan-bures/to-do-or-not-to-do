package app.todoornot.ui

import androidx.activity.compose.BackHandler
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.gestures.Orientation
import androidx.compose.foundation.gestures.detectDragGesturesAfterLongPress
import androidx.compose.foundation.gestures.rememberScrollableState
import androidx.compose.foundation.gestures.scrollable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.layout.wrapContentWidth
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.selection.selectable
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.clipToBounds
import androidx.compose.ui.draw.drawWithContent
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Rect
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.PathEffect
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.text.rememberTextMeasurer
import androidx.compose.ui.hapticfeedback.HapticFeedbackType
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.layout.boundsInRoot
import androidx.compose.ui.layout.onGloballyPositioned
import androidx.compose.ui.layout.positionInRoot
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.platform.LocalHapticFeedback
import androidx.compose.ui.platform.LocalLayoutDirection
import androidx.compose.ui.semantics.CustomAccessibilityAction
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.customActions
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.semantics.stateDescription
import androidx.compose.ui.text.style.TextDecoration
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.LayoutDirection
import androidx.compose.ui.unit.dp
import app.todoornot.core.Chip
import app.todoornot.core.DateStyle
import app.todoornot.core.LocalNow
import app.todoornot.core.RoomKey
import app.todoornot.core.TimelineBar
import app.todoornot.core.TimelineColumn
import app.todoornot.core.TimelineEdit
import app.todoornot.core.TimelineSpan
import app.todoornot.core.TimelineTask
import app.todoornot.core.TimelineZoom
import app.todoornot.core.WorkflowStage
import app.todoornot.core.displayDate
import app.todoornot.core.quickAddMonth
import app.todoornot.core.timelineColumns
import app.todoornot.core.timelinePreview
import app.todoornot.core.timelineScale
import app.todoornot.design.Icon
import app.todoornot.design.LocalDesign
import app.todoornot.design.Tokens
import app.todoornot.design.style
import app.todoornot.model.Actions
import app.todoornot.model.MenuAction
import app.todoornot.model.MenuEntry
import app.todoornot.model.UiState
import app.todoornot.model.featuresFor
import app.todoornot.model.label
import app.todoornot.model.roomMenu
import kotlin.math.ceil
import kotlin.math.floor
import kotlin.math.roundToInt

private val S = Tokens.Space
private val P = Tokens.Metrics.Phone
private val T = Tokens.Metrics.Timeline
private val C = Tokens.Metrics.Component
private val E = Tokens.Elevation

/** Deterministic state for the screenshot gallery; production starts on two weeks with a closed tray. */
data class TimelineGallery(val zoom: TimelineZoom, val tray: Boolean = false, val preview: Pair<String, TimelineEdit>? = null)

internal fun timelineRoomMenu(state: UiState): List<MenuEntry> = roomMenu(state.home, state.room.key, state.showCompleted).mapNotNull { entry ->
    if (state.timeline == null || entry !is MenuEntry.Item || entry.action !is MenuAction.ShowCompleted) entry
    else if (state.room.key is RoomKey.Smart) null else entry.copy(label = "${entry.label} on Timeline")
}

private val TimelineZoom.label get() = when (this) {
    TimelineZoom.WEEK -> "Week"
    TimelineZoom.TWO_WEEKS -> "2 weeks"
    TimelineZoom.MONTH -> "Month"
    TimelineZoom.QUARTER -> "Quarter"
}

private val TimelineZoom.minDayWidth get() = when (this) {
    TimelineZoom.WEEK -> P.TOUCH.dp
    TimelineZoom.TWO_WEEKS -> S.S8.dp
    TimelineZoom.MONTH -> S.S3.dp
    TimelineZoom.QUARTER -> S.S2.dp
}

internal data class TimelineCalendarRequest(val task: TimelineTask, val start: Boolean)

internal class TimelineSheets {
    var menu by mutableStateOf<TimelineTask?>(null)
    var picker by mutableStateOf<TimelineCalendarRequest?>(null)
    var resize by mutableStateOf<TimelineResize?>(null)
    val open get() = menu != null || picker != null
}

internal enum class TimelineDragMode { MOVE, START, DUE, PLACE }
internal data class TimelineResize(val task: String, val mode: TimelineDragMode)
private data class Lift(val task: TimelineTask, val edit: TimelineEdit, val span: TimelineSpan)

/** One drag owns the original date pair and a fixed scale; a live sync can repaint the grid but can't
 * silently turn an in-progress move into an edit of the new dates. */
private class TimelineGesture(val task: TimelineTask, val mode: TimelineDragMode, val origin: Offset) {
    var pointer = origin
    var edit: TimelineEdit? = null
}

@Composable
internal fun TimelineBody(state: UiState, actions: Actions, blocked: Boolean, sheets: TimelineSheets, gallery: TimelineGallery?) {
    val timeline = state.timeline ?: return
    val now = state.timelineNow ?: return
    val r = LocalDesign.current.room
    val haptic = LocalHapticFeedback.current
    val density = LocalDensity.current
    var zoom by rememberSaveable { mutableStateOf(gallery?.zoom ?: TimelineZoom.TWO_WEEKS) }
    val scale = remember(zoom, state.today) { timelineScale(zoom, state.today) }
    val todayLabel = remember(state.today, now.dateFormat) { displayDate(state.today, now, DateStyle.DATE).orEmpty() }
    val textMeasurer = rememberTextMeasurer()
    val rulerInk = Tokens.Type.META_PHONE.style(r.ink2)
    val dateWidth = with(density) { textMeasurer.measure(todayLabel, rulerInk).size.width.toDp() + S.S3.dp }
    var firstDay by rememberSaveable(state.today) { mutableStateOf(scale.todayOffset.toFloat()) }
    var trayOpen by rememberSaveable { mutableStateOf(gallery?.tray ?: false) }
    var railOpen by rememberSaveable { mutableStateOf(true) }
    var drag by remember { mutableStateOf<TimelineGesture?>(null) }
    var preview by remember { mutableStateOf<Lift?>(null) }
    var dateBounds by remember { mutableStateOf(Rect.Zero) }
    val enabled = !blocked && !state.signedOut
    val cancel = { drag = null; preview = null }
    BackHandler(drag != null || sheets.resize != null) {
        cancel()
        sheets.resize = null
    }
    LaunchedEffect(enabled, state.today, zoom) { cancel() }

    val fixtureLift = remember(gallery, timeline, now) {
        gallery?.preview?.let { (id, edit) ->
            val task = (timeline.lanes + timeline.unscheduled.orEmpty()).firstOrNull { it.taskId == id }
            task?.let { timelinePreview(it.dates, edit, now)?.let { span -> Lift(task, edit, span) } }
        }
    }
    val lift = preview ?: fixtureLift
    val hasTray = timeline.unscheduled != null
    val tray = timeline.unscheduled.orEmpty()

    // Dates increase to the right independent of UI mirroring. Each virtualized lane draws only the
    // visible slice of the same ruler; the task rail and ruler remain pinned during either scroll.
    CompositionLocalProvider(LocalLayoutDirection provides LayoutDirection.Ltr) {
        BoxWithConstraints(Modifier.fillMaxSize()) {
            val rail = if (!railOpen) P.TOUCH.dp else minOf(T.RAIL_WIDTH.dp, maxWidth / 3)
            val gridWidth = (maxWidth - rail).coerceAtLeast(S.S1.dp)
            fun widthFor(zoom: TimelineZoom, days: Int): Dp = maxOf(gridWidth / days, zoom.minDayWidth,
                if (zoom == TimelineZoom.WEEK || zoom == TimelineZoom.TWO_WEEKS) dateWidth else 0.dp)
            val dayWidth = widthFor(zoom, scale.days)
            val dayPx = with(density) { dayWidth.toPx() }
            val visible = gridWidth / dayWidth
            fun clamped(value: Float) = value.coerceIn(scale.minOffset.toFloat(), scale.maxOffset - visible)
            val offset = clamped(firstDay)
            val from = floor(offset).toInt()
            val through = ceil(offset + visible).toInt() - 1
            val columns = remember(zoom, from, through, now) { timelineColumns(zoom, from, through, now) }
            val month = columns.firstOrNull { offset + visible / 2 < it.day + it.days }?.month.orEmpty()
            val pan = rememberScrollableState { delta ->
                val current = clamped(firstDay)
                val next = clamped(current - delta / dayPx)
                firstDay = next
                (current - next) * dayPx
            }
            fun turn(by: Int) { firstDay = clamped(offset + by * maxOf(1, floor(visible).toInt())) }
            fun changeZoom(next: TimelineZoom) {
                val target = timelineScale(next, state.today)
                firstDay = offset + (visible - gridWidth / widthFor(next, target.days)) / 2f
                zoom = next
            }
            fun reveal(span: TimelineSpan) {
                firstDay = clamped(span.first - (visible - minOf(visible, span.last + 1f - span.first)) / 2f)
            }
            val begin: (TimelineTask, TimelineDragMode, Offset) -> Unit = { task, mode, point ->
                if (enabled) {
                    drag = TimelineGesture(task, mode, point)
                    haptic.performHapticFeedback(HapticFeedbackType.LongPress)
                }
            }
            val move: (Offset) -> Unit = { delta ->
                drag?.let { gesture ->
                    gesture.pointer += delta
                    val point = gesture.pointer
                    val by = ((point.x - gesture.origin.x) / dayPx).roundToInt()
                    val edit = when {
                        !dateBounds.contains(point) -> null
                        gesture.mode == TimelineDragMode.PLACE -> TimelineEdit.Place(floor(offset + (point.x - dateBounds.left) / dayPx).toInt())
                        gesture.mode == TimelineDragMode.START -> TimelineEdit.StartEdge(by)
                        gesture.mode == TimelineDragMode.DUE -> TimelineEdit.DueEdge(by)
                        else -> TimelineEdit.Move(by)
                    }
                    if (gesture.edit != edit) {
                        gesture.edit = edit
                        val next = edit?.let { timelinePreview(gesture.task.dates, it, now) }
                        if (next != null && next != preview?.span) haptic.performHapticFeedback(HapticFeedbackType.TextHandleMove)
                        preview = next?.let { Lift(gesture.task, edit, it) }
                    }
                }
            }
            val finish = {
                preview?.let { actions.scheduleTimeline(it.task, it.edit, state.today, state.generation) }
                sheets.resize = null
                cancel()
            }
            val dragEvents = DragEvents(begin, move, finish, cancel)

            Column(Modifier.fillMaxSize()) {
                Box(Modifier.weight(1f).fillMaxWidth().clipToBounds()
                    .scrollable(pan, Orientation.Horizontal, enabled = enabled && drag == null)) {
                    // Use a passive date-area hit target. Unlike a lane, the blank grid can also receive a tray drop.
                    Box(Modifier.matchParentSize().padding(start = rail)
                        .onGloballyPositioned { dateBounds = it.boundsInRoot() }) {
                        GridLines(columns, offset, dayWidth, if (lift?.edit is TimelineEdit.Place) lift.span.first else null)
                    }
                    Column(Modifier.fillMaxSize()) {
                        Row(Modifier.fillMaxWidth().height(P.TOUCH.dp).background(r.canvas).hairline(r.line),
                            verticalAlignment = Alignment.CenterVertically) {
                            Row(Modifier.width(rail), verticalAlignment = Alignment.CenterVertically) {
                                IconButton("list", if (railOpen) "Hide task names" else "Show task names", expanded = railOpen) { railOpen = !railOpen }
                                if (railOpen) Label("Tasks", Tokens.Type.BUTTON_SMALL, r.ink2)
                            }
                            Ruler(columns, offset, dayWidth, gridWidth, zoom, todayLabel, dateWidth,
                                Modifier.weight(1f).fillMaxHeight().clipToBounds())
                        }
                        LazyColumn(Modifier.fillMaxSize(), userScrollEnabled = drag == null) {
                            if (timeline.lanes.isEmpty()) item {
                                Label(if (state.filter.isNotBlank()) "Nothing matches the filter." else "No scheduled tasks.",
                                    Tokens.Type.META_PHONE, r.ink2, Modifier.fillMaxWidth().background(r.pane).padding(S.S3.dp))
                            }
                            items(timeline.lanes, key = { it.taskId }) { task ->
                                val dates = task.span ?: return@items
                                Lane(task, if (lift?.task?.taskId == task.taskId) lift.span else dates, dates, rail, railOpen,
                                    offset, dayWidth, gridWidth, enabled, dragEvents,
                                    workflow = state.home.featuresFor(RoomKey.List(task.listId))?.workflow == true,
                                    resize = sheets.resize?.takeIf { it.task == task.taskId }?.mode,
                                    active = lift?.task?.taskId == task.taskId || state.selected == task.taskId || sheets.resize?.task == task.taskId,
                                    select = { actions.select(task.taskId) }, reveal = { reveal(dates) },
                                    calendar = { start -> sheets.picker = TimelineCalendarRequest(task, start) }, menu = { sheets.menu = task })
                            }
                        }
                    }
                }
                val resize = sheets.resize
                if (lift != null || drag != null || resize != null) {
                    val text = lift?.let { "${it.task.title} · ${it.span.label}" } ?: when (drag?.mode ?: resize?.mode) {
                        TimelineDragMode.PLACE -> "Drop on a date to schedule"
                        TimelineDragMode.START -> "Hold and drag the bar to resize the start"
                        TimelineDragMode.DUE -> "Hold and drag the bar to resize the end"
                        else -> "Drag to move the dates"
                    }
                    Row(Modifier.fillMaxWidth().background(r.tint).hairline(r.line, top = true), verticalAlignment = Alignment.CenterVertically) {
                        Label(text, Tokens.Type.BUTTON_SMALL, r.accentStrong, Modifier.weight(1f)
                            .padding(horizontal = S.S3.dp, vertical = S.S2.dp))
                        if (resize != null) IconButton("x", "Cancel resizing") { sheets.resize = null; cancel() }
                    }
                }
                if (hasTray && trayOpen) UnscheduledTray(tray, enabled, dragEvents, drag != null,
                    select = { actions.select(it.taskId) }, schedule = { sheets.picker = TimelineCalendarRequest(it, false) })
                TimelineControls(zoom, month, ::changeZoom, { turn(-1) }, { firstDay = scale.todayOffset.toFloat() }, { turn(1) },
                    if (hasTray) tray.size else null, trayOpen, !blocked && drag == null) { trayOpen = !trayOpen }
            }
        }
    }
}

@Composable
internal fun TimelineOverlays(state: UiState, sheets: TimelineSheets, actions: Actions) {
    val now = state.timelineNow ?: return
    val r = LocalDesign.current.room
    sheets.menu?.let { task ->
        BottomSheet("Schedule", { sheets.menu = null }) { overlay ->
            fun choose(start: Boolean) = overlay.choose {
                sheets.menu = null
                sheets.picker = TimelineCalendarRequest(task, start)
            }
            PickerCard("Schedule", inSheet = true) {
                Label(task.title, Tokens.Type.BUTTON, r.ink)
                Button("Start date…", false) { choose(true) }
                Button("Due date…", false) { choose(false) }
                fun arm(mode: TimelineDragMode) = overlay.choose { sheets.resize = TimelineResize(task.taskId, mode) }
                val span = task.span
                if (span != null && span.bar != TimelineBar.OPEN_ENDED)
                    Button("Drag start edge", false) { arm(TimelineDragMode.START) }
                if (span != null && span.bar != TimelineBar.MILESTONE)
                    Button("Drag end edge", false) { arm(TimelineDragMode.DUE) }
                Button("Cancel", false, onClick = overlay::close)
            }
        }
    }
    sheets.picker?.let { request ->
        TimelineCalendar(request, now, { sheets.picker = null }) { day ->
            val before = if (request.start) request.task.dates.start else request.task.dates.due
            if (before != day) actions.scheduleTimeline(request.task,
                if (request.start) TimelineEdit.PickStart(day) else TimelineEdit.PickDue(day), state.today, state.generation)
        }
    }
}

private data class DragEvents(
    val begin: (TimelineTask, TimelineDragMode, Offset) -> Unit,
    val move: (Offset) -> Unit,
    val finish: () -> Unit,
    val cancel: () -> Unit,
)

@Composable
private fun timelineDrag(task: TimelineTask, enabled: Boolean, events: DragEvents, mode: (Offset) -> TimelineDragMode): Modifier {
    val current by rememberUpdatedState(events)
    val source by rememberUpdatedState(task)
    val operation by rememberUpdatedState(mode)
    var origin by remember { mutableStateOf(Offset.Zero) }
    if (!enabled) return Modifier
    return Modifier.onGloballyPositioned { origin = it.positionInRoot() }.pointerInput(task.taskId) {
        detectDragGesturesAfterLongPress(
            onDragStart = { current.begin(source, operation(it), origin + it) },
            onDragEnd = { current.finish() },
            onDragCancel = { current.cancel() },
        ) { change, delta ->
            change.consume()
            current.move(delta)
        }
    }
}

@Composable
private fun GridLines(columns: List<TimelineColumn>, offset: Float, dayWidth: Dp, target: Int?) {
    val r = LocalDesign.current.room
    Canvas(Modifier.fillMaxSize()) {
        val day = dayWidth.toPx()
        val line = E.HAIRLINE.dp.toPx()
        columns.forEach { col ->
            val x = (col.day - offset) * day
            if (col.weekend && col.days == 1) drawRect(r.canvas, Offset(x, 0f), Size(day, size.height))
            drawLine(r.line, Offset(x, 0f), Offset(x, size.height), line)
        }
        target?.let { drawRect(r.tint, Offset((it - offset) * day, 0f), Size(day, size.height)) }
        val x = (0.5f - offset) * day
        drawLine(r.accentStrong, Offset(x, 0f), Offset(x, size.height), T.TODAY_LINE.dp.toPx())
    }
}

@Composable
private fun Ruler(columns: List<TimelineColumn>, offset: Float, dayWidth: Dp, grid: Dp,
    zoom: TimelineZoom, today: String, labelWidth: Dp, modifier: Modifier,
) {
    val r = LocalDesign.current.room
    Box(modifier) {
        GridLines(columns, offset, dayWidth, null)
        val weekly = zoom == TimelineZoom.MONTH || zoom == TimelineZoom.QUARTER
        columns.forEachIndexed { i, col ->
            val show = !weekly || i == 0 || columns[i - 1].week != col.week
            if (show) {
                val x = dayWidth * (col.day - offset)
                val right = x + dayWidth * (if (weekly) col.weekRemaining else col.days)
                val left = x.coerceAtLeast(0.dp)
                val width = minOf(grid, right) - left
                // Leave partial date cells unlabelled instead of cutting through their text.
                if ((weekly && width >= S.S8.dp) || (x >= 0.dp && right <= grid && col.day != 0 && !weekly)) {
                    Box(Modifier.offset(x = left).width(width).fillMaxHeight(), contentAlignment = Alignment.TopCenter) {
                        Label(if (weekly) col.week else col.label, Tokens.Type.META_PHONE, r.ink2,
                            Modifier.background(r.canvas).padding(S.S1.dp))
                    }
                }
            }
        }
        val todayX = dayWidth * (0.5f - offset)
        if (todayX in 0.dp..grid && labelWidth <= grid) {
            Box(Modifier.align(if (weekly) Alignment.BottomStart else Alignment.TopStart)
                .offset(x = (todayX - labelWidth / 2).coerceIn(0.dp, grid - labelWidth)).width(labelWidth),
                contentAlignment = Alignment.Center) {
                Label(today, Tokens.Type.META_PHONE, r.accentStrong, Modifier.background(r.tint).padding(S.S1.dp)
                    .semantics { contentDescription = "Today, $today" })
            }
        }
    }
}

@Composable
private fun Lane(
    task: TimelineTask, painted: TimelineSpan, dates: TimelineSpan, rail: Dp, railOpen: Boolean,
    offset: Float, dayWidth: Dp, grid: Dp, enabled: Boolean, events: DragEvents,
    workflow: Boolean, resize: TimelineDragMode?, active: Boolean,
    select: () -> Unit, reveal: () -> Unit, calendar: (Boolean) -> Unit, menu: () -> Unit,
) {
    val density = LocalDensity.current
    val r = LocalDesign.current.room
    val chip = task.chip
    val list = chip?.let { LocalDesign.current.list(it.hue) }
    val done = task.stage == WorkflowStage.DONE
    val overdue = !done && task.dates.due != null && dates.last < 0
    val inProgress = workflow && task.stage == WorkflowStage.IN_PROGRESS
    val description = if (workflow || done) "${task.stage.label}, ${dates.label}" else dates.label
    val cellLeft = dayWidth * (dates.first - offset)
    val cellRight = dayWidth * (dates.last + 1 - offset)
    val onScreen = cellLeft < grid && cellRight > 0.dp
    val touch = P.TOUCH.dp
    val canStart = dates.bar != TimelineBar.OPEN_ENDED && cellLeft >= 0.dp
    val canEnd = dates.bar != TimelineBar.MILESTONE && cellRight <= grid
    val visibleWidth = minOf(cellRight, grid) - maxOf(cellLeft, 0.dp)
    // A short bar has one full-size move target. Its edges can still be dragged using the
    // Schedule sheet's explicit mode rather than making the user hit a tiny resize sliver.
    val directEdges = resize == null && visibleWidth >= touch * (1 + (if (canStart) 1 else 0) + (if (canEnd) 1 else 0))
    val startGrip = directEdges && canStart
    val endGrip = directEdges && canEnd
    val actions = if (!enabled) emptyList() else listOf(
        CustomAccessibilityAction("Change start date") { calendar(true); true },
        CustomAccessibilityAction("Change due date") { calendar(false); true },
    )
    Row(Modifier.fillMaxWidth().height(T.LANE_ROW_PHONE.dp).hairline(r.line)) {
        Row(Modifier.width(rail).fillMaxHeight().background(if (active) r.wash else r.pane)
            .hairline(r.line).combinedClickable(enabled = enabled, role = Role.Button, onLongClickLabel = "Schedule",
                onLongClick = menu, onClick = { if (onScreen) select() else reveal() })
            .semantics {
                contentDescription = task.title
                stateDescription = if (onScreen) description else "Off screen, $description"
                customActions = actions
            }.padding(horizontal = S.S2.dp),
            verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(S.S1.dp)) {
            if (!onScreen) Icon(if (cellRight <= 0.dp) "chevron-left" else "chevron-right", r.ink3, size = C.ICON_SMALL.dp)
            else if (done || !railOpen) Icon(if (done) "check" else "calendar", r.ink3, size = C.ICON_SMALL.dp)
            if (railOpen) Column(Modifier.weight(1f)) {
                Label(task.title, Tokens.Type.BUTTON_SMALL, r.ink, decoration = if (done) TextDecoration.LineThrough else null)
                if (chip == null) Label(painted.label, Tokens.Type.META_PHONE, if (overdue) r.dangerInk else r.ink2)
                else TimelineListPill(chip)
            }
        }
        Box(Modifier.weight(1f).fillMaxHeight().clipToBounds()) {
            val rawLeft = dayWidth * (painted.first - offset)
            val rawRight = dayWidth * (painted.last + 1 - offset)
            val gap = minOf(S.S1.dp, dayWidth / 6)
            val naturalLeft = rawLeft + gap
            val naturalRight = rawRight - gap
            val startVisible = naturalLeft >= 0.dp
            val endVisible = naturalRight <= grid
            // Extend a clipped bar just beyond the viewport so its side stroke is also off-screen.
            val left = naturalLeft.coerceAtLeast(-E.HAIRLINE.dp)
            val right = naturalRight.coerceAtMost(grid + E.HAIRLINE.dp)
            val width = right - left
            if (width > 0.dp && naturalLeft < grid && naturalRight > 0.dp) {
                val color = when {
                    active -> r.accentStrong
                    overdue -> r.dangerInk
                    list != null -> list.ink
                    else -> r.accentStrong
                }
                val fill = when {
                    overdue -> r.dangerBg
                    list != null -> list.tint
                    done -> r.canvas
                    else -> r.tint
                }
                // Only actual endpoints are rounded, never the edge of a clipped viewport.
                val startRadius = if (startVisible) Tokens.Radius.MD.dp else 0.dp
                val endRadius = if (endVisible) Tokens.Radius.MD.dp else 0.dp
                val shape = RoundedCornerShape(topStart = startRadius, bottomStart = startRadius, topEnd = endRadius, bottomEnd = endRadius)
                val railMark = (inProgress || list != null) && startVisible && width >= S.S8.dp
                val showStartGrip = startGrip && startVisible
                val showEndGrip = endGrip && endVisible
                Row(Modifier.align(Alignment.CenterStart).offset(x = left).wrapContentWidth(Alignment.Start, unbounded = true)
                    .width(width).height(T.BAR_HEIGHT_PHONE.dp).clip(shape).background(fill)
                    .then(if (painted.bar == TimelineBar.OPEN_ENDED && endVisible) Modifier.openEnd(fill, color) else Modifier)
                    .then(if (showStartGrip && !railMark || showEndGrip) Modifier.resizeMarks(color, showStartGrip && !railMark, showEndGrip) else Modifier)
                    .border(E.HAIRLINE.dp, color, shape)
                    .then(if (railMark) Modifier.selectionBar(color) else Modifier)
                    .padding(start = if (railMark || showStartGrip) S.S3.dp else S.S1.dp,
                        end = if (showEndGrip) S.S3.dp else S.S1.dp),
                    verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(S.S1.dp)) {
                    if (painted.bar == TimelineBar.MILESTONE && width >= C.ICON_SMALL.dp + S.S2.dp)
                        Icon("flag", color, size = C.ICON_SMALL.dp)
                    if (width >= (if (railOpen) T.INLINE_LABEL_MIN.dp else S.S8.dp))
                        Label(if (railOpen) painted.label else task.title, Tokens.Type.META_PHONE, if (list != null && !overdue) list.ink else r.ink)
                }
                if (!railOpen && width < S.S8.dp) {
                    val after = grid - right - S.S2.dp
                    val before = left - S.S2.dp
                    val useAfter = after >= before
                    val space = maxOf(after, before)
                    if (space >= P.TOUCH.dp) Label(task.title, Tokens.Type.META_PHONE, r.ink,
                        Modifier.align(Alignment.CenterStart).offset(x = if (useAfter) right + S.S2.dp else 0.dp)
                            .widthIn(max = space).background(r.pane).padding(horizontal = S.S1.dp))
                }
            }

            if (onScreen) {
                val hitWidth = maxOf(P.TOUCH.dp, cellRight.coerceAtMost(grid) - cellLeft.coerceAtLeast(0.dp)).coerceAtMost(grid)
                val hitLeft = ((cellLeft + cellRight - hitWidth) / 2).coerceIn(0.dp, grid - hitWidth)
                val move = timelineDrag(task, enabled, events) { point ->
                    val x = hitLeft + with(density) { point.x.toDp() }
                    resize ?: when {
                        startGrip && x < cellLeft + touch -> TimelineDragMode.START
                        endGrip && x >= cellRight - touch -> TimelineDragMode.DUE
                        else -> TimelineDragMode.MOVE
                    }
                }
                Box(Modifier.offset(x = hitLeft).width(hitWidth).fillMaxHeight().then(move)
                    .clickable(enabled = enabled, role = Role.Button, onClick = select).semantics {
                        contentDescription = "Timeline bar: ${task.title}"
                        stateDescription = description
                        customActions = actions
                    })
            }
        }
    }
}

@Composable
private fun TimelineListPill(chip: Chip) {
    val list = LocalDesign.current.list(chip.hue)
    Label(chip.label, Tokens.Type.CHIP, list.ink, Modifier.heightIn(min = C.CHIP.dp)
        .background(list.tint, RoundedCornerShape(Tokens.Radius.CHIP.dp)).padding(horizontal = C.CHIP_PAD_X.dp))
}

private fun Modifier.openEnd(fill: Color, ink: Color) = drawWithContent {
    drawContent()
    val stroke = E.HAIRLINE.dp.toPx()
    val inset = Tokens.Radius.MD.dp.toPx()
    val x = size.width - stroke / 2
    val start = Offset(x, inset)
    val end = Offset(x, size.height - inset)
    drawLine(fill, start, end, stroke)
    drawLine(ink, start, end, stroke, pathEffect = PathEffect.dashPathEffect(floatArrayOf(S.S1.dp.toPx(), S.S1.dp.toPx())))
}

private fun Modifier.resizeMarks(ink: Color, start: Boolean, end: Boolean) = drawWithContent {
    drawContent()
    val inset = S.S2.dp.toPx()
    val half = S.S3.dp.toPx() / 2
    val stroke = T.TODAY_LINE.dp.toPx()
    fun mark(x: Float) = drawLine(ink, Offset(x, center.y - half), Offset(x, center.y + half), stroke, cap = StrokeCap.Round)
    if (start) mark(inset)
    if (end) mark(size.width - inset)
}

@Composable
private fun UnscheduledTray(tasks: List<TimelineTask>, enabled: Boolean, events: DragEvents, dragging: Boolean,
    select: (TimelineTask) -> Unit, schedule: (TimelineTask) -> Unit,
) {
    val r = LocalDesign.current.room
    Column(Modifier.fillMaxWidth().background(r.canvas).hairline(r.line, top = true).padding(S.S2.dp)) {
        Label("Unscheduled", Tokens.Type.BUTTON_SMALL, r.ink, Modifier.padding(start = S.S1.dp))
        if (tasks.isEmpty()) {
            Label("Every open task has a date.", Tokens.Type.META_PHONE, r.ink2, Modifier.padding(S.S1.dp))
        } else {
            LazyColumn(Modifier.heightIn(max = T.TRAY_EXPANDED.dp), userScrollEnabled = !dragging) {
                items(tasks, key = { it.taskId }) { task ->
                    Row(Modifier.fillMaxWidth().height(P.TOUCH.dp).hairline(r.line), verticalAlignment = Alignment.CenterVertically) {
                        val gesture = timelineDrag(task, enabled, events) { TimelineDragMode.PLACE }
                        Row(Modifier.weight(1f).fillMaxHeight().then(gesture).clickable(enabled = enabled, role = Role.Button) { select(task) }
                            .padding(horizontal = S.S1.dp), verticalAlignment = Alignment.CenterVertically,
                            horizontalArrangement = Arrangement.spacedBy(S.S2.dp)) {
                            Icon("grip-vertical", r.ink3, size = C.ICON_SMALL.dp)
                            Label(task.title, Tokens.Type.BUTTON_SMALL, r.ink, Modifier.weight(1f, fill = false))
                            task.chip?.let { TimelineListPill(it) }
                        }
                        TimelineButton("Schedule", enabled = enabled, description = "Schedule ${task.title}") { schedule(task) }
                    }
                }
            }
        }
    }
}

@Composable
private fun TimelineControls(zoom: TimelineZoom, month: String, changeZoom: (TimelineZoom) -> Unit, previous: () -> Unit, today: () -> Unit,
    next: () -> Unit, unscheduled: Int?, tray: Boolean, enabled: Boolean, toggleTray: () -> Unit,
) {
    val r = LocalDesign.current.room
    var options by remember { mutableStateOf(false) }
    Row(Modifier.fillMaxWidth().background(r.canvas).hairline(r.line, top = true).padding(horizontal = S.S1.dp),
        verticalAlignment = Alignment.CenterVertically) {
        IconButton("chevron-left", "Previous dates", enabled = enabled, onClick = previous)
        TimelineButton("Today", enabled = enabled, onClick = today)
        IconButton("chevron-right", "Next dates", enabled = enabled, onClick = next)
        Box(Modifier.weight(1f), contentAlignment = Alignment.Center) {
            Label(month, Tokens.Type.META_PHONE, r.ink2)
        }
        Box {
            TimelineButton(zoom.label, enabled = enabled, description = "Timeline zoom: ${zoom.label}") { options = true }
            if (options && enabled) AnchoredPopup(onDismiss = { options = false }, above = true) { overlay ->
                Column(Modifier.padding(S.S2.dp).widthIn(min = P.INNER_LIST.dp / 3)
                    .overlay(RoundedCornerShape(Tokens.Radius.LG.dp), LocalDesign.current).padding(S.S1.dp)) {
                    TimelineZoom.entries.forEach { scale ->
                        Box(Modifier.fillMaxWidth().height(P.TOUCH.dp).selectable(scale == zoom, role = Role.RadioButton) {
                            overlay.choose { changeZoom(scale) }
                        }.padding(horizontal = S.S2.dp), contentAlignment = Alignment.CenterStart) {
                            Label(scale.label, Tokens.Type.BUTTON, if (scale == zoom) r.accentStrong else r.ink)
                        }
                    }
                }
            }
        }
        if (unscheduled != null) {
            TimelineButton("Unscheduled $unscheduled", selected = tray, enabled = enabled,
                description = "Unscheduled: $unscheduled", onClick = toggleTray)
        }
    }
}

@Composable
private fun TimelineButton(text: String, enabled: Boolean = true, description: String? = null,
    selected: Boolean? = null, onClick: () -> Unit,
) {
    val r = LocalDesign.current.room
    val shape = RoundedCornerShape(Tokens.Radius.MD.dp)
    Box(Modifier.widthIn(min = P.TOUCH.dp).height(P.TOUCH.dp)
        .then(if (selected == null) Modifier.clickable(enabled, role = Role.Button, onClick = onClick)
            else Modifier.selectable(selected, enabled, role = Role.Button, onClick = onClick))
        .semantics { if (description != null) contentDescription = description }.padding(horizontal = S.S1.dp),
        contentAlignment = Alignment.Center) {
        Label(text, Tokens.Type.BUTTON_SMALL, if (enabled) r.accentStrong else r.ink3,
            Modifier.background(if (selected == true) r.tint else r.pane, shape).border(E.HAIRLINE.dp, r.lineStrong, shape)
                .padding(horizontal = S.S2.dp, vertical = S.S1.dp))
    }
}

@Composable
private fun TimelineCalendar(request: TimelineCalendarRequest, now: LocalNow, close: () -> Unit, choose: (String?) -> Unit) {
    val selected = if (request.start) request.task.dates.start else request.task.dates.due
    var cursor by rememberSaveable { mutableStateOf<String?>(null) }
    val month = remember(now, selected, cursor) { quickAddMonth(now, selected, cursor) }
    BottomSheet(if (request.start) "Start date" else if (request.task.span == null) "Schedule" else "Due date", close) { overlay ->
        DateCalendar(month, selected, { date -> overlay.choose { choose(date) } }, { cursor = it }, starts = request.start,
            dismiss = overlay::close, repeats = !request.start && request.task.repeats,
            inSheet = true, focusCalendar = !LocalSheetAtRest.current)
    }
}
