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
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.layout.wrapContentWidth
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.selection.selectable
import androidx.compose.foundation.selection.selectableGroup
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.SideEffect
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
import androidx.compose.ui.platform.LocalViewConfiguration
import androidx.compose.ui.semantics.CustomAccessibilityAction
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.clearAndSetSemantics
import androidx.compose.ui.semantics.heading
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
    var coverEyebrow by mutableStateOf<String?>(null)
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
    var moved = false
}

@Composable
internal fun TimelineBody(state: UiState, actions: Actions, blocked: Boolean, sheets: TimelineSheets, gallery: TimelineGallery?,
    cover: Boolean = false, onAdd: (() -> Unit)? = null,
) {
    val timeline = state.timeline ?: return
    val now = state.timelineNow ?: return
    val r = LocalDesign.current.room
    val haptic = LocalHapticFeedback.current
    val density = LocalDensity.current
    val touchSlop = LocalViewConfiguration.current.touchSlop
    var zoom by rememberSaveable(cover) {
        mutableStateOf(gallery?.zoom ?: if (cover) TimelineZoom.WEEK else TimelineZoom.TWO_WEEKS)
    }
    val scale = remember(zoom, state.today) { timelineScale(zoom, state.today) }
    val todayLabel = remember(state.today, now.dateFormat) { displayDate(state.today, now, DateStyle.DATE).orEmpty() }
    val textMeasurer = rememberTextMeasurer()
    val rulerInk = Tokens.Type.META_PHONE.style(r.ink2)
    val dateWidth = with(density) { textMeasurer.measure(todayLabel, rulerInk).size.width.toDp() + S.S3.dp }
    val todayOffset = if (cover && zoom == TimelineZoom.WEEK) (scale.todayOffset - 1).coerceAtLeast(scale.minOffset) else scale.todayOffset
    var firstDay by rememberSaveable(state.today, cover) { mutableStateOf(todayOffset.toFloat()) }
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
    LaunchedEffect(enabled, state.today, zoom, cover) { cancel() }

    val fixtureLift = remember(gallery, timeline, now) {
        gallery?.preview?.let { (id, edit) ->
            val task = (timeline.lanes + timeline.unscheduled.orEmpty()).firstOrNull { it.taskId == id }
            task?.let { timelinePreview(it.dates, edit, now)?.let { span -> Lift(task, edit, span) } }
        }
    }
    val lift = preview ?: fixtureLift
    val hasTray = timeline.unscheduled != null
    val tray = timeline.unscheduled.orEmpty()
    val steps = if (cover) remember(state.room.tasks, state.room.completed) {
        (state.room.tasks + state.room.completed).mapNotNull { row ->
            row.steps?.let { row.id to "${it.done}/${it.total}" }
        }.toMap()
    } else emptyMap()

    // Dates increase to the right independent of UI mirroring. Each virtualized lane draws only the
    // visible slice of the same ruler; the task rail and ruler remain pinned during either scroll.
    CompositionLocalProvider(LocalLayoutDirection provides LayoutDirection.Ltr) {
        BoxWithConstraints(Modifier.fillMaxSize()) {
            val rail = if (cover) 0.dp else if (!railOpen) P.TOUCH.dp else minOf(T.RAIL_WIDTH.dp, maxWidth / 3)
            val gridWidth = (maxWidth - rail).coerceAtLeast(S.S1.dp)
            fun widthFor(zoom: TimelineZoom, days: Int): Dp {
                // A cover Week shows six touch-sized dates, leaving the rest within a swipe.
                val count = if (cover && zoom == TimelineZoom.WEEK) days - 1 else days
                val labelWidth = if (!cover && (zoom == TimelineZoom.WEEK || zoom == TimelineZoom.TWO_WEEKS)) dateWidth else 0.dp
                return maxOf(gridWidth / count, zoom.minDayWidth, labelWidth)
            }
            val dayWidth = widthFor(zoom, scale.days)
            val dayPx = with(density) { dayWidth.toPx() }
            val visible = gridWidth / dayWidth
            fun clamped(value: Float) = value.coerceIn(scale.minOffset.toFloat(), scale.maxOffset - visible)
            val offset = clamped(firstDay)
            val from = floor(offset).toInt()
            val through = ceil(offset + visible).toInt() - 1
            val columns = remember(zoom, from, through, now) { timelineColumns(zoom, from, through, now) }
            val month = columns.firstOrNull { offset + visible / 2 < it.day + it.days }?.month.orEmpty()
            val coverWindow = if (cover) remember(timeline, from, through) { coverWindow(timeline.lanes, from, through) } else null
            if (cover) {
                val weeks = columns.firstOrNull()?.let { weekRange(it.week, columns.last().week) }
                val scheduled = timeline.lanes.count { it.stage != WorkflowStage.DONE }
                SideEffect { sheets.coverEyebrow = if (weeks == null) "$scheduled scheduled" else "$scheduled scheduled · $weeks" }
            }
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
                    gesture.moved = gesture.moved || (gesture.pointer - gesture.origin).getDistance() >= touchSlop
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
                val gesture = drag
                preview?.let { actions.scheduleTimeline(it.task, it.edit, state.today, state.generation) }
                if (cover && preview == null && gesture != null && !gesture.moved &&
                    gesture.mode != TimelineDragMode.PLACE && sheets.resize == null) {
                    sheets.menu = gesture.task
                }
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
                            if (!cover) Row(Modifier.width(rail), verticalAlignment = Alignment.CenterVertically) {
                                TimelineSquareIcon("list", if (railOpen) "Hide task names" else "Show task names", false) { railOpen = !railOpen }
                                if (railOpen) Label("Tasks", Tokens.Type.BUTTON_SMALL, r.ink2)
                            }
                            val ruler = Modifier.weight(1f).fillMaxHeight().clipToBounds()
                            if (cover) CoverRuler(columns, offset, dayWidth, gridWidth, zoom, todayLabel, dateWidth,
                                ruler, useFullDates = !now.dateFormat.isNullOrEmpty() && now.dateFormat != "day_month")
                            else Ruler(columns, offset, dayWidth, gridWidth, zoom, todayLabel, dateWidth, ruler)
                        }
                        LazyColumn(Modifier.fillMaxSize(), userScrollEnabled = drag == null) {
                            if (timeline.lanes.isEmpty()) item {
                                Label(if (state.filter.isNotBlank()) "Nothing matches the filter." else "No scheduled tasks.",
                                    Tokens.Type.META_PHONE, r.ink2, Modifier.fillMaxWidth().background(r.pane).padding(S.S3.dp))
                            }
                            if (coverWindow != null && coverWindow.earlier.isNotEmpty()) item("earlier") {
                                OffscreenHint(coverWindow.earlier, earlier = true, enabled) { reveal(it) }
                            }
                            val visibleTasks = coverWindow?.visible ?: timeline.lanes
                            val grouped = cover && state.listFeatures?.workflow == true
                            val sections = if (grouped) coverStages.associateWith { stage -> visibleTasks.filter { it.stage == stage } }
                                .filterValues { it.isNotEmpty() } else mapOf(null to visibleTasks)
                            sections.forEach { (stage, tasks) ->
                                if (stage != null) item("stage-${stage.name}") { CoverSection(stage, tasks.size) }
                                items(tasks, key = { it.taskId }) { task ->
                                    val dates = task.span ?: return@items
                                    Lane(task, if (lift?.task?.taskId == task.taskId) lift.span else dates, dates, rail, railOpen,
                                        offset, dayWidth, gridWidth, enabled, dragEvents, cover = cover,
                                        progress = steps[task.taskId]?.takeUnless { lift?.task?.taskId == task.taskId },
                                        workflow = state.home.featuresFor(RoomKey.List(task.listId))?.workflow == true,
                                        resize = sheets.resize?.takeIf { it.task == task.taskId }?.mode,
                                        active = lift?.task?.taskId == task.taskId || state.selected == task.taskId || sheets.resize?.task == task.taskId,
                                        select = { actions.select(task.taskId) }, reveal = { reveal(dates) },
                                        calendar = { start -> sheets.picker = TimelineCalendarRequest(task, start) }, menu = { sheets.menu = task })
                                }
                            }
                            if (coverWindow != null && coverWindow.later.isNotEmpty()) item("later") {
                                OffscreenHint(coverWindow.later, earlier = false, enabled) { reveal(it) }
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
                if (cover) CoverControls(zoom, ::changeZoom, { firstDay = todayOffset.toFloat() },
                    if (hasTray) tray.size else null, trayOpen, !blocked && drag == null, onAdd.takeUnless { state.signedOut }) { trayOpen = !trayOpen }
                else TimelineControls(zoom, month, ::changeZoom, { turn(-1) }, { firstDay = todayOffset.toFloat() }, { turn(1) },
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

private val coverStages = listOf(WorkflowStage.IN_PROGRESS, WorkflowStage.TODO, WorkflowStage.BACKLOG, WorkflowStage.DONE)

private data class CoverWindow(val visible: List<TimelineTask>, val earlier: List<TimelineSpan>, val later: List<TimelineSpan>)

private fun coverWindow(tasks: List<TimelineTask>, from: Int, through: Int): CoverWindow {
    val visible = mutableListOf<TimelineTask>()
    val earlier = mutableListOf<TimelineSpan>()
    val later = mutableListOf<TimelineSpan>()
    for (task in tasks) {
        val span = task.span ?: continue
        when {
            span.last < from -> earlier += span
            span.first > through -> later += span
            else -> visible += task
        }
    }
    return CoverWindow(visible, earlier, later)
}

private fun weekRange(first: String, last: String) = if (first == last) first else "$first–$last"

@Composable
private fun CoverSection(stage: WorkflowStage, count: Int) {
    val r = LocalDesign.current.room
    val ink = if (stage == WorkflowStage.IN_PROGRESS) r.accentStrong else r.ink2
    Row(Modifier.fillMaxWidth().background(r.canvas).hairline(r.line)
        .padding(horizontal = S.S4.dp, vertical = S.S1.dp).semantics { heading() },
        horizontalArrangement = Arrangement.spacedBy(S.S2.dp), verticalAlignment = Alignment.CenterVertically) {
        Icon(stage.icon, ink, size = C.ICON_SMALL.dp)
        Label("${stage.label} · $count", Tokens.Type.CAPS_LABEL, ink)
    }
}

@Composable
private fun OffscreenHint(spans: List<TimelineSpan>, earlier: Boolean, enabled: Boolean, reveal: (TimelineSpan) -> Unit) {
    val r = LocalDesign.current.room
    val weeks = if (earlier) weekRange(spans.minBy { it.last }.lastWeek, spans.maxBy { it.last }.lastWeek)
        else weekRange(spans.minBy { it.first }.firstWeek, spans.maxBy { it.first }.firstWeek)
    val tasks = if (spans.size == 1) "task" else "tasks"
    val direction = if (earlier) "earlier" else "later"
    val shape = RoundedCornerShape(Tokens.Radius.MD.dp)
    Row(Modifier.fillMaxWidth().padding(horizontal = S.S4.dp, vertical = S.S1.dp)
        .heightIn(min = P.TOUCH.dp).clip(shape).background(r.pane).border(E.HAIRLINE.dp, r.line, shape)
        .clickable(enabled = enabled, role = Role.Button, onClickLabel = "Show $direction tasks") {
            reveal(if (earlier) spans.maxBy { it.last } else spans.minBy { it.first })
        }
        .padding(horizontal = S.S3.dp), verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(S.S2.dp)) {
        Label("${spans.size} more $tasks $direction in $weeks", Tokens.Type.META_PHONE, r.ink2,
            Modifier.weight(1f), maxLines = 2)
        Label(if (earlier) "← Swipe right" else "Swipe left →", Tokens.Type.META_PHONE, r.accentStrong)
    }
}

@Composable
private fun GridLines(columns: List<TimelineColumn>, offset: Float, dayWidth: Dp, target: Int?, todayLine: Boolean = true) {
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
        if (todayLine) {
            val x = (0.5f - offset) * day
            drawLine(r.accentStrong, Offset(x, 0f), Offset(x, size.height), T.TODAY_LINE.dp.toPx())
        }
    }
}

@Composable
private fun CoverRuler(columns: List<TimelineColumn>, offset: Float, dayWidth: Dp, grid: Dp,
    zoom: TimelineZoom, today: String, labelWidth: Dp, modifier: Modifier, useFullDates: Boolean,
) {
    if (zoom == TimelineZoom.MONTH || zoom == TimelineZoom.QUARTER) {
        Ruler(columns, offset, dayWidth, grid, zoom, today, labelWidth, modifier, todayDot = true)
        return
    }
    val r = LocalDesign.current.room
    val roomy = dayWidth >= P.TOUCH.dp
    Box(modifier) {
        GridLines(columns, offset, dayWidth, null, todayLine = false)
        columns.forEachIndexed { index, col ->
            val x = dayWidth * (col.day - offset)
            val current = col.day == 0
            val full = x >= 0.dp && x + dayWidth <= grid + E.HAIRLINE.dp
            if (!full && (!current || x + dayWidth / 2 !in 0.dp..grid)) return@forEachIndexed
            Box(Modifier.offset(x = x).width(dayWidth).fillMaxHeight().then(if (current) Modifier.background(r.tint) else Modifier)
                .clearAndSetSemantics { contentDescription = if (current) "Today, ${col.label}" else "${col.weekday}, ${col.label}" }) {
                if (index == 0 || col.weekRemaining == 7) {
                    Label(col.week, Tokens.Type.COUNT, r.ink2, Modifier.align(Alignment.TopStart).padding(start = S.S1.dp))
                }
                Column(Modifier.align(Alignment.Center).padding(top = if (roomy) 0.dp else S.S2.dp),
                    horizontalAlignment = Alignment.CenterHorizontally) {
                    val ink = if (current) r.accentStrong else r.ink2
                    Label(col.weekday, Tokens.Type.CAPS_LABEL, ink)
                    // A day number is an axis coordinate. Prefer the selected date format where there is room for it.
                    val date = if (useFullDates && dayWidth >= labelWidth) col.label else col.dayNumber
                    Label(date, Tokens.Type.COUNT, if (current) r.accentStrong else r.ink, weight = Tokens.Type.BUTTON.weight)
                }
                if (current) Box(Modifier.align(Alignment.BottomCenter).size(T.TODAY_DOT.dp).background(r.accentStrong, CircleShape))
            }
        }
    }
}

@Composable
private fun Ruler(columns: List<TimelineColumn>, offset: Float, dayWidth: Dp, grid: Dp,
    zoom: TimelineZoom, today: String, labelWidth: Dp, modifier: Modifier, todayDot: Boolean = false,
) {
    val r = LocalDesign.current.room
    Box(modifier) {
        GridLines(columns, offset, dayWidth, null, todayLine = !todayDot)
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
                .offset(x = (todayX - labelWidth / 2).coerceIn(0.dp, grid - labelWidth)).width(labelWidth)
                .padding(bottom = if (todayDot) T.TODAY_DOT.dp else 0.dp),
                contentAlignment = Alignment.Center) {
                Label(today, Tokens.Type.META_PHONE, r.accentStrong, Modifier.background(r.tint).padding(S.S1.dp)
                    .semantics { contentDescription = "Today, $today" })
            }
            if (todayDot) Box(Modifier.align(Alignment.BottomStart).offset(x = todayX - T.TODAY_DOT.dp / 2)
                .size(T.TODAY_DOT.dp).background(r.accentStrong, CircleShape))
        }
    }
}

@Composable
private fun Lane(
    task: TimelineTask, painted: TimelineSpan, dates: TimelineSpan, rail: Dp, railOpen: Boolean,
    offset: Float, dayWidth: Dp, grid: Dp, enabled: Boolean, events: DragEvents,
    cover: Boolean, progress: String?, workflow: Boolean, resize: TimelineDragMode?, active: Boolean,
    select: () -> Unit, reveal: () -> Unit, calendar: (Boolean) -> Unit, menu: () -> Unit,
) {
    val density = LocalDensity.current
    val r = LocalDesign.current.room
    val chip = task.chip
    val list = chip?.let { LocalDesign.current.list(it.hue) }
    val done = task.stage == WorkflowStage.DONE
    val overdueLabel = painted.overdueLabel?.takeUnless { done }
    val overdue = overdueLabel != null
    val inProgress = workflow && task.stage == WorkflowStage.IN_PROGRESS
    val description = listOfNotNull(chip?.label, task.stage.label.takeIf { workflow || done }, dates.label, overdueLabel)
        .joinToString(", ")
    val measurer = rememberTextMeasurer()
    val barTitleStyle = Tokens.Type.BUTTON.style(r.ink)
    val barMetaStyle = Tokens.Type.META_PHONE.style(r.ink2)
    val titleWidth = if (cover) with(density) { measurer.measure(task.title, barTitleStyle).size.width.toDp() } else 0.dp
    val meta = overdueLabel ?: progress ?: painted.label
    val metaWidth = if (cover) with(density) { measurer.measure(meta, barMetaStyle).size.width.toDp() } else 0.dp
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
        CustomAccessibilityAction("Schedule options") { menu(); true },
    )
    Row(Modifier.fillMaxWidth().height(T.LANE_ROW_PHONE.dp).hairline(r.line)) {
        if (!cover) Row(Modifier.width(rail).fillMaxHeight().background(if (active) r.wash else r.pane)
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
                if (overdueLabel != null) Label(overdueLabel, Tokens.Type.META_PHONE, r.dangerInk)
                else if (chip == null) Label(painted.label, Tokens.Type.META_PHONE, r.ink2)
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
                    overdue -> r.dangerInk
                    active -> r.accentStrong
                    list != null -> list.ink
                    cover && workflow && !inProgress && painted.bar == TimelineBar.RANGE -> r.control
                    else -> r.accentStrong
                }
                val fill = when {
                    overdue -> r.dangerBg
                    list != null -> list.tint
                    done -> r.canvas
                    cover && active -> r.tint
                    cover && workflow && !inProgress && painted.bar == TimelineBar.RANGE -> r.pane
                    else -> r.tint
                }
                val ink = when {
                    overdue -> r.dangerInk
                    list != null -> list.ink
                    else -> r.ink
                }
                // Only actual endpoints are rounded, never the edge of a clipped viewport.
                val startRadius = if (startVisible) Tokens.Radius.MD.dp else 0.dp
                val endRadius = if (endVisible) Tokens.Radius.MD.dp else 0.dp
                val shape = RoundedCornerShape(topStart = startRadius, bottomStart = startRadius, topEnd = endRadius, bottomEnd = endRadius)
                val marker = overdue || painted.bar == TimelineBar.MILESTONE
                val railMark = !marker && (inProgress || list != null) && startVisible && width >= S.S8.dp
                val showStartGrip = startGrip && startVisible
                val showEndGrip = endGrip && endVisible
                val pad = if (marker && width < C.ICON_SMALL.dp + S.S2.dp) 0.dp else S.S1.dp
                val padStart = if (railMark || showStartGrip) S.S3.dp else pad
                val padEnd = if (showEndGrip) S.S3.dp else pad
                val inner = (width - padStart - padEnd).coerceAtLeast(0.dp)
                val after = grid - right - S.S2.dp
                val before = left - S.S2.dp
                val useAfter = after >= before
                val space = maxOf(after, before)
                val markerWidth = if (marker) C.ICON_SMALL.dp + S.S1.dp else 0.dp
                val titleInside = cover && (!marker || width >= T.INLINE_LABEL_MIN.dp) &&
                    (inner - markerWidth >= titleWidth || inner - markerWidth >= maxOf(space, P.TOUCH.dp))
                val metaInside = titleInside && inner - markerWidth >= metaWidth + S.S2.dp + minOf(titleWidth, T.INLINE_LABEL_MIN.dp)
                Row(Modifier.align(Alignment.CenterStart).offset(x = left).wrapContentWidth(Alignment.Start, unbounded = true)
                    .width(width).height(T.BAR_HEIGHT_PHONE.dp).clip(shape).background(fill)
                    .then(if (painted.bar == TimelineBar.OPEN_ENDED && endVisible) Modifier.openEnd(fill, color) else Modifier)
                    .then(if (showStartGrip && !railMark || showEndGrip) Modifier.resizeMarks(color, showStartGrip && !railMark, showEndGrip) else Modifier)
                    .border(E.HAIRLINE.dp, color, shape)
                    .then(if (railMark) Modifier.selectionBar(color) else Modifier)
                    .padding(start = padStart, end = padEnd), verticalAlignment = Alignment.CenterVertically,
                    horizontalArrangement = if (marker && !titleInside && (cover || width < T.INLINE_LABEL_MIN.dp))
                        Arrangement.Center else Arrangement.spacedBy(S.S1.dp)) {
                    if (overdue) OverdueDiamond(ink, inner)
                    else if (painted.bar == TimelineBar.MILESTONE && inner >= C.ICON_SMALL.dp)
                        Icon("flag", color, size = C.ICON_SMALL.dp)
                    if (titleInside) {
                        Label(task.title, Tokens.Type.BUTTON, ink, Modifier.weight(1f),
                            decoration = TextDecoration.LineThrough.takeIf { done })
                        if (metaInside || (overdue && inner >= metaWidth + T.INLINE_LABEL_MIN.dp))
                            Label(meta, Tokens.Type.META_PHONE, if (overdue) ink else list?.ink ?: r.ink2)
                    } else if (!cover && width >= (if (railOpen) T.INLINE_LABEL_MIN.dp else S.S8.dp)) {
                        Label(overdueLabel ?: if (railOpen) painted.label else task.title, Tokens.Type.META_PHONE, ink)
                    }
                }
                val outside = if (cover) !titleInside else !railOpen && width < S.S8.dp
                if (outside && space >= P.TOUCH.dp) {
                    Box(Modifier.align(Alignment.CenterStart).offset(x = if (useAfter) right + S.S2.dp else 0.dp)
                        .width(space).fillMaxHeight().then(if (cover) Modifier.combinedClickable(enabled = enabled,
                            role = Role.Button, onLongClickLabel = "Schedule", onLongClick = menu, onClick = select) else Modifier),
                        contentAlignment = if (useAfter) Alignment.CenterStart else Alignment.CenterEnd) {
                        Column(Modifier.background(r.pane, RoundedCornerShape(Tokens.Radius.SM.dp)).padding(horizontal = S.S1.dp),
                            horizontalAlignment = if (useAfter) Alignment.Start else Alignment.End) {
                            Row(horizontalArrangement = Arrangement.spacedBy(S.S1.dp), verticalAlignment = Alignment.CenterVertically) {
                                Label(task.title, if (cover) Tokens.Type.BUTTON else Tokens.Type.META_PHONE, if (overdue) ink else r.ink,
                                    Modifier.weight(1f, fill = false), decoration = TextDecoration.LineThrough.takeIf { done })
                                if (overdueLabel != null && space >= metaWidth + P.TOUCH.dp)
                                    Label("· $overdueLabel", Tokens.Type.META_PHONE, ink)
                            }
                            if (cover && chip != null) TimelineListPill(chip)
                        }
                    }
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
private fun OverdueDiamond(ink: Color, width: Dp) {
    if (width > 0.dp) Canvas(Modifier.size(minOf(S.S2.dp, width))) {
        val diamond = androidx.compose.ui.graphics.Path().apply {
            moveTo(center.x, 0f)
            lineTo(size.width, center.y)
            lineTo(center.x, size.height)
            lineTo(0f, center.y)
            close()
        }
        drawPath(diamond, ink)
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
internal fun TimelineSquareIcon(icon: String, label: String, selected: Boolean, onClick: () -> Unit) {
    val r = LocalDesign.current.room
    val shape = RoundedCornerShape(Tokens.Radius.MD.dp)
    Box(Modifier.size(P.TOUCH.dp).clickable(role = Role.Button, onClick = onClick)
        .clearAndSetSemantics { contentDescription = label }, contentAlignment = Alignment.Center) {
        Box(Modifier.size(Tokens.Metrics.Board.HEADER_HEIGHT.dp)
            .then(if (selected) Modifier.background(r.raised, shape).border(E.HAIRLINE.dp, r.lineStrong, shape) else Modifier),
            contentAlignment = Alignment.Center) {
            Icon(icon, if (selected) r.ink else r.ink2)
        }
    }
}

@Composable
private fun CoverControls(zoom: TimelineZoom, changeZoom: (TimelineZoom) -> Unit, today: () -> Unit,
    unscheduled: Int?, tray: Boolean, enabled: Boolean, add: (() -> Unit)?, toggleTray: () -> Unit,
) {
    val r = LocalDesign.current.room
    val shape = RoundedCornerShape(Tokens.Radius.MD.dp)
    Row(Modifier.fillMaxWidth().background(r.canvas).hairline(r.line, top = true).padding(horizontal = S.S1.dp),
        verticalAlignment = Alignment.CenterVertically) {
        TimelineButton("Today", enabled = enabled, onClick = today)
        Row(Modifier.height(P.TOUCH.dp).drawWithContent {
            val inset = S.S1.dp.toPx()
            val radius = Tokens.Radius.MD.dp.toPx()
            drawRoundRect(r.wash, topLeft = Offset(0f, inset), size = Size(size.width, size.height - 2 * inset),
                cornerRadius = androidx.compose.ui.geometry.CornerRadius(radius))
            drawContent()
        }.selectableGroup(), verticalAlignment = Alignment.CenterVertically) {
            listOf(TimelineZoom.WEEK to "1W", TimelineZoom.TWO_WEEKS to "2W", TimelineZoom.MONTH to "M").forEach { (scale, short) ->
                Box(Modifier.size(P.TOUCH.dp).selectable(scale == zoom, enabled, role = Role.Tab) { changeZoom(scale) }
                    .semantics { contentDescription = "Timeline zoom: ${scale.label}" }, contentAlignment = Alignment.Center) {
                    Box(Modifier.size(Tokens.Metrics.Board.HEADER_HEIGHT.dp)
                        .then(if (scale == zoom) Modifier.background(r.raised, shape).border(E.HAIRLINE.dp, r.lineStrong, shape) else Modifier),
                        contentAlignment = Alignment.Center) {
                        Label(short, Tokens.Type.BUTTON_SMALL, if (enabled) r.ink else r.ink3)
                    }
                }
            }
        }
        if (unscheduled == null) Spacer(Modifier.weight(1f))
        else BoxWithConstraints(Modifier.weight(1f), contentAlignment = Alignment.CenterEnd) {
            val full = "Unscheduled $unscheduled"
            val fit = maxWidth >= P.CHIP_MAX.dp && LocalDensity.current.fontScale < 1.3f
            TimelineButton(if (fit) full else "Unsched $unscheduled", enabled, description = "Unscheduled: $unscheduled",
                selected = tray, icon = "calendar-range", onClick = toggleTray)
        }
        if (add != null) {
            Box(Modifier.size(P.TOUCH.dp).clickable(enabled = enabled, role = Role.Button, onClick = add)
                .semantics { contentDescription = "Add task" }, contentAlignment = Alignment.Center) {
                Box(Modifier.size(S.S10.dp).background(if (enabled) r.accentFill else r.wash, shape), contentAlignment = Alignment.Center) {
                    Icon("plus", if (enabled) r.onAccent else r.ink3)
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
    selected: Boolean? = null, icon: String? = null, onClick: () -> Unit,
) {
    val r = LocalDesign.current.room
    val shape = RoundedCornerShape(Tokens.Radius.MD.dp)
    Box(Modifier.widthIn(min = P.TOUCH.dp).height(P.TOUCH.dp)
        .then(if (selected == null) Modifier.clickable(enabled, role = Role.Button, onClick = onClick)
            else Modifier.selectable(selected, enabled, role = Role.Button, onClick = onClick))
        .semantics { if (description != null) contentDescription = description }.padding(horizontal = S.S1.dp),
        contentAlignment = Alignment.Center) {
        Row(Modifier.background(if (selected == true) r.tint else r.pane, shape).border(E.HAIRLINE.dp, r.lineStrong, shape)
            .padding(horizontal = S.S2.dp, vertical = S.S1.dp), verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(S.S1.dp)) {
            val ink = if (enabled) r.accentStrong else r.ink3
            icon?.let { Icon(it, ink, size = C.ICON_SMALL.dp) }
            Label(text, Tokens.Type.BUTTON_SMALL, ink)
        }
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
