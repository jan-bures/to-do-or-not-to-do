package app.todoornot.ui

import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.animation.core.tween
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.gestures.awaitEachGesture
import androidx.compose.foundation.gestures.awaitFirstDown
import androidx.compose.foundation.gestures.awaitLongPressOrCancellation
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
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
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.LazyListState
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.pager.HorizontalPager
import androidx.compose.foundation.pager.PageSize
import androidx.compose.foundation.pager.PagerState
import androidx.compose.foundation.pager.rememberPagerState
import androidx.compose.foundation.selection.selectable
import androidx.compose.foundation.selection.selectableGroup
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.setValue
import androidx.compose.runtime.withFrameNanos
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.drawBehind
import androidx.compose.ui.geometry.CornerRadius
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Rect
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.PathEffect
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.hapticfeedback.HapticFeedbackType
import androidx.compose.ui.input.pointer.PointerEventPass
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.layout.Layout
import androidx.compose.ui.layout.LayoutCoordinates
import androidx.compose.ui.layout.boundsInRoot
import androidx.compose.ui.layout.onGloballyPositioned
import androidx.compose.ui.platform.LocalHapticFeedback
import androidx.compose.ui.semantics.CustomAccessibilityAction
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.customActions
import androidx.compose.ui.semantics.heading
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.IntOffset
import androidx.compose.ui.unit.dp
import app.todoornot.core.BoardSlot
import app.todoornot.core.DueTone
import app.todoornot.core.ListView
import app.todoornot.core.OpenStage
import app.todoornot.core.RoomKey
import app.todoornot.core.TaskRow
import app.todoornot.core.WorkflowStage
import app.todoornot.design.Icon
import app.todoornot.design.LocalDesign
import app.todoornot.design.Tokens
import app.todoornot.design.cubic
import app.todoornot.model.Actions
import app.todoornot.model.UiState
import app.todoornot.model.label
import app.todoornot.model.openStage
import app.todoornot.model.visibleCards
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlin.math.roundToInt

private val B = Tokens.Metrics.Board
private val C = Tokens.Metrics.Component
private val P = Tokens.Metrics.Phone
private val S = Tokens.Space
private val M = Tokens.Motion
private val stages = WorkflowStage.entries
private val openStages = stages.filter { it.openStage() != null }

internal class BoardInput(val pager: PagerState, val scope: CoroutineScope, val reduced: Boolean) {
    data class Card(val task: TaskRow, val stage: WorkflowStage, val bounds: Rect)
    data class Drop(val stage: WorkflowStage, val slot: BoardSlot)
    data class Scroll(val state: LazyListState, val bounds: Rect)
    val scrolls = linkedMapOf<WorkflowStage, Scroll>()
    val cards = linkedMapOf<String, Card>()
    val columns = linkedMapOf<WorkflowStage, Rect>()
    val tabs = linkedMapOf<WorkflowStage, Rect>()
    var root: LayoutCoordinates? = null
    var picked by mutableStateOf<Card?>(null)
    var point by mutableStateOf(Offset.Zero)
    var target by mutableStateOf<Drop?>(null)
    var body by mutableStateOf<Rect?>(null)
    var doneExpanded by mutableStateOf(false)
    var adding by mutableStateOf<OpenStage?>(null)
    var pendingMove by mutableStateOf<TaskRow?>(null)
    val active get() = stages[pager.currentPage]
    val canAdd get() = active.openStage() != null

    fun page(index: Int) {
        if (index !in stages.indices || index == pager.currentPage) return
        scope.launch {
            if (reduced) pager.scrollToPage(index)
            else pager.animateScrollToPage(index, animationSpec = tween(M.PANEL, easing = M.EASE_STANDARD.cubic()))
        }
    }

    fun candidate(): Drop? {
        val source = picked ?: return null
        val tab = tabs.entries.firstOrNull { point in it.value }?.key
        if (tab != null) return if (tab == source.stage) null else Drop(tab, BoardSlot.First)
        val column = columns.entries.firstOrNull { point in it.value }?.key ?: return null
        val card = cards.values.firstOrNull { it.stage == column && point in it.bounds }
        if (card?.task?.id == source.task.id) return null
        val slot = when {
            card == null -> BoardSlot.Last
            point.y < card.bounds.center.y -> BoardSlot.Before(card.task.id)
            else -> BoardSlot.After(card.task.id)
        }
        return Drop(column, slot)
    }
}

@Composable
internal fun rememberBoardInput(initial: WorkflowStage = WorkflowStage.TODO): BoardInput {
    val pager = rememberPagerState(initialPage = initial.ordinal) { stages.size }
    val scope = rememberCoroutineScope()
    val reduced = reducedMotion()
    return remember(pager, scope, reduced) { BoardInput(pager, scope, reduced) }
}

/** Capture the long press in the stationary room, never a lazy card that might scroll out of composition. */
@Composable
internal fun boardDrag(input: BoardInput, state: UiState, actions: Actions, enabled: Boolean, pager: Boolean): Modifier {
    val haptic = LocalHapticFeedback.current
    val active by rememberUpdatedState(enabled)
    val list = (state.room.key as? RoomKey.List)?.id
    val density = androidx.compose.ui.platform.LocalDensity.current
    val edge = with(density) { P.EDGE_GESTURE_INSET.dp.toPx() }
    val point = input.point
    val flip = if (pager && input.picked != null) input.body?.let { b ->
        if (point.y !in b.top..b.bottom) 0 else when {
            point.x <= b.left + edge -> -1
            point.x >= b.right - edge -> 1
            else -> 0
        }
    } ?: 0 else 0
    val dragging = input.picked != null
    LaunchedEffect(input, dragging, density) {
        if (!dragging) return@LaunchedEffect
        val band = with(density) { S.S8.dp.toPx() }
        val step = with(density) { S.S2.dp.toPx() }
        while (input.picked != null) withFrameNanos {
            val finger = input.point
            input.scrolls.values.firstOrNull { finger in it.bounds }?.let { scroll ->
                val dy = when {
                    finger.y < scroll.bounds.top + band -> -step
                    finger.y > scroll.bounds.bottom - band -> step
                    else -> 0f
                }
                if (dy != 0f) scroll.state.dispatchRawDelta(dy)
            }
            val target = input.candidate()
            if (target != input.target && target != null) haptic.performHapticFeedback(HapticFeedbackType.TextHandleMove)
            input.target = target
        }
    }
    LaunchedEffect(input, flip, input.pager.settledPage) {
        if (flip != 0) {
            delay(M.BOARD_EDGE_DWELL.toLong())
            if (input.picked != null) input.page(input.pager.settledPage + flip)
        }
    }
    return Modifier.onGloballyPositioned { input.root = it }.pointerInput(input, list, state.generation, actions) {
        awaitEachGesture {
            val down = awaitFirstDown(requireUnconsumed = false, pass = PointerEventPass.Initial)
            if (!active || list == null) return@awaitEachGesture
            val root = input.root?.takeIf { it.isAttached } ?: return@awaitEachGesture
            val start = root.localToRoot(down.position)
            val card = input.cards.values.firstOrNull { start in it.bounds } ?: return@awaitEachGesture
            val long = awaitLongPressOrCancellation(down.id) ?: return@awaitEachGesture
            if (!active) return@awaitEachGesture
            haptic.performHapticFeedback(HapticFeedbackType.LongPress)
            input.point = root.localToRoot(long.position)
            input.picked = card
            try {
                while (active && root.isAttached) {
                    val change = awaitPointerEvent(PointerEventPass.Initial).changes.firstOrNull { it.id == long.id } ?: break
                    if (change.isConsumed) break
                    input.point = root.localToRoot(change.position)
                    if (!change.pressed) {
                        change.consume()
                        if ((input.point - start).getDistance() > viewConfiguration.touchSlop) {
                            input.candidate()?.let { actions.moveBoardCard(list, card.task.id, it.stage, it.slot, state.generation) }
                        }
                        break
                    }
                    change.consume()
                }
            } finally {
                input.target = null
                input.picked = null
            }
        }
    }
}

/** A horizontal move starting in the system back inset never reaches the pager's gesture detector. */
private fun Modifier.guardPagerEdges() = pointerInput(Unit) {
    val edge = P.EDGE_GESTURE_INSET.dp.toPx()
    awaitEachGesture {
        val down = awaitFirstDown(requireUnconsumed = false, pass = PointerEventPass.Initial)
        if (down.position.x > edge && down.position.x < size.width - edge) return@awaitEachGesture
        do {
            val change = awaitPointerEvent(PointerEventPass.Initial).changes.firstOrNull { it.id == down.id } ?: break
            val travel = change.position - down.position
            if (kotlin.math.abs(travel.x) >= kotlin.math.abs(travel.y) && travel.x != 0f) change.consume()
        } while (change.pressed)
    }
}

@Composable
internal fun BoardBody(state: UiState, actions: Actions, input: BoardInput, wide: Boolean, full: Boolean,
    fold: PaneFold?, modifier: Modifier = Modifier,
) {
    val board = state.board ?: return
    val cards = remember(board, state.room, state.boardReopening) { board.visibleCards(state.room, state.boardReopening) }
    val r = LocalDesign.current.room
    val shown = if (full && input.doneExpanded) stages else openStages
    val startAdd: (WorkflowStage) -> Unit = { stage -> input.adding = stage.openStage() }
    @Composable fun Well(stage: WorkflowStage, m: Modifier) {
        BoardWell(stage, cards[stage].orEmpty(), state, actions, input, m, wide, startAdd)
    }
    Box(modifier.onGloballyPositioned { input.body = it.boundsInRoot() }) {
        if (full) {
            Row(Modifier.fillMaxSize().padding(S.S4.dp), horizontalArrangement = Arrangement.spacedBy(B.COL_GAP.dp)) {
                if (fold != null) {
                    // Keep two narrow columns left of the physical hinge and the remaining well/rail on the right.
                    val left = (fold.start - S.S4.dp - B.COL_GAP.dp).coerceAtLeast(P.TOUCH.dp)
                    Row(Modifier.width(left).fillMaxHeight(), horizontalArrangement = Arrangement.spacedBy(B.COL_GAP.dp)) {
                        openStages.take(2).forEach { Well(it, Modifier.weight(1f)) }
                    }
                    Spacer(Modifier.width(fold.end - fold.start))
                    Well(WorkflowStage.IN_PROGRESS, Modifier.weight(1f))
                    if (input.doneExpanded) Well(WorkflowStage.DONE, Modifier.weight(1f))
                    else DoneRail(cards[WorkflowStage.DONE].orEmpty().size, input)
                } else {
                    shown.forEach { Well(it, Modifier.weight(1f)) }
                    if (!input.doneExpanded) DoneRail(cards[WorkflowStage.DONE].orEmpty().size, input)
                }
            }
        } else BoxWithConstraints(Modifier.fillMaxSize()) {
            val inset = if (wide) S.S4.dp else B.COL_GAP.dp + B.COVER_PEEK.dp
            val pageSize = if (wide && maxWidth - inset * 2 > B.COL_WIDTH.dp) PageSize.Fixed(B.COL_WIDTH.dp) else PageSize.Fill
            HorizontalPager(input.pager, Modifier.fillMaxSize().guardPagerEdges(),
                contentPadding = PaddingValues(horizontal = inset, vertical = S.S3.dp), pageSize = pageSize,
                pageSpacing = B.COL_GAP.dp, beyondViewportPageCount = 1, userScrollEnabled = input.picked == null,
            ) { page -> Well(stages[page], Modifier.fillMaxWidth()) }
        }
        // A peek is decorative, so the pager surface remains the same colour all the way to the glass.
        if (!full && state.room.filtered && cards.values.all { it.isEmpty() }) {
            Label("Nothing matches the filter", Tokens.Type.NOTES, r.ink2, Modifier.align(Alignment.BottomCenter).padding(S.S5.dp))
        }
    }
}

@Composable
private fun BoardWell(stage: WorkflowStage, cards: List<TaskRow>, state: UiState, actions: Actions, input: BoardInput,
    modifier: Modifier, wide: Boolean, add: (WorkflowStage) -> Unit,
) {
    val r = LocalDesign.current.room
    val shape = RoundedCornerShape(Tokens.Radius.LG.dp)
    val target = input.target?.stage == stage
    val list = rememberLazyListState()
    var shifting by remember(state.reorder) { mutableStateOf(state.reorder?.rows.orEmpty()) }
    LaunchedEffect(state.reorder) {
        if (shifting.isNotEmpty()) {
            delay(M.ROW_ENTER.toLong())
            shifting = emptySet()
        }
    }
    DisposableEffect(input, stage) { onDispose { input.columns.remove(stage); input.scrolls.remove(stage) } }
    Column(modifier.fillMaxHeight().clip(shape).background(if (target) r.wash else r.canvas)
        .border(Tokens.Elevation.HAIRLINE.dp, if (target) r.accentStrong else r.line, shape)
        .onGloballyPositioned { input.columns[stage] = it.boundsInRoot() },
    ) {
        Row(Modifier.fillMaxWidth().heightIn(min = B.HEADER_HEIGHT.dp).hairline(r.line)
            .padding(horizontal = B.CARD_PAD.dp).semantics { heading() },
            verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(S.S2.dp)) {
            Icon(stage.icon, stage.ink(), size = C.ICON_SMALL.dp)
            Label(stage.label, Tokens.Type.BUTTON, if (stage == WorkflowStage.DONE) r.ink else stage.ink(), Modifier.weight(1f))
            val count = if (!wide && stage == WorkflowStage.IN_PROGRESS && !state.room.filtered) "${cards.size} of ${state.room.open} open" else cards.size.toString()
            Label(count, Tokens.Type.COUNT, if (stage == WorkflowStage.IN_PROGRESS) r.accentStrong else r.ink3)
        }
        LazyColumn(Modifier.weight(1f).onGloballyPositioned { input.scrolls[stage] = BoardInput.Scroll(list, it.boundsInRoot()) },
            state = list, contentPadding = PaddingValues(S.S2.dp), verticalArrangement = Arrangement.spacedBy(B.CARD_GAP.dp),
        ) {
            items(cards, key = { it.id }) { card ->
                val animate = !input.reduced && card.id in shifting
                BoardCard(card, stage, state, actions, input, Modifier.animateItem(fadeInSpec = null, fadeOutSpec = null,
                    placementSpec = if (animate) tween(M.ROW_ENTER, easing = M.EASE_STANDARD.cubic()) else null))
            }
            if (cards.isEmpty()) item {
                Box(Modifier.fillMaxWidth().heightIn(min = P.ROW.dp).dashedWell(r.lineStrong).padding(S.S2.dp),
                    contentAlignment = Alignment.Center) {
                    Label(when (stage) {
                        WorkflowStage.BACKLOG -> "Drop future ideas here"
                        WorkflowStage.TODO -> "Nothing to do here"
                        WorkflowStage.IN_PROGRESS -> "Nothing in progress"
                        WorkflowStage.DONE -> "No recent completions"
                    }, Tokens.Type.META_PHONE, r.ink3)
                }
            }
            if (!wide && stage != WorkflowStage.DONE) item {
                BoardFooter("Add to ${stage.label}", "plus", dashed = true, enabled = !state.signedOut) { add(stage) }
            }
            if (stage == WorkflowStage.DONE) item {
                val total = state.board?.totalDone ?: 0u
                if (total > 0u) BoardFooter("View all $total in Completed", "list", dashed = false) {
                    actions.setRoomView(ListView.LIST, state.generation, showCompleted = true)
                }
            }
        }
        if (wide && stage != WorkflowStage.DONE) Box(Modifier.padding(horizontal = S.S2.dp)) {
            BoardFooter("Add to ${stage.label}", "plus", dashed = false, enabled = !state.signedOut) { add(stage) }
        }
        if (wide && stage == WorkflowStage.DONE && input.doneExpanded) Box(Modifier.padding(horizontal = S.S2.dp)) {
            BoardFooter("Collapse Done", "chevron-right", dashed = false) { input.doneExpanded = false }
        }
    }
}

@Composable
private fun BoardCard(task: TaskRow, stage: WorkflowStage, state: UiState, actions: Actions, input: BoardInput, modifier: Modifier) {
    val r = LocalDesign.current.room
    val shape = RoundedCornerShape(Tokens.Radius.LG.dp)
    val selected = state.selected == task.id
    val origin = input.picked?.task?.id == task.id
    val motion = LocalMotion.current
    val look = motion.row(task.id)
    val canEdit = !state.signedOut && !origin && look?.gone != true
    val onMove: () -> Unit = { input.pendingMove = task }
    val slot = input.target?.slot
    val before = (slot as? BoardSlot.Before)?.taskId == task.id
    val after = (slot as? BoardSlot.After)?.taskId == task.id
    DisposableEffect(input, task.id) { onDispose { input.cards.remove(task.id) } }
    Box(modifier.rowMotion(look).fillMaxWidth().onGloballyPositioned {
        input.cards[task.id] = BoardInput.Card(task, stage, it.boundsInRoot())
    }.clip(shape).background(if (selected || origin) r.wash else r.raised)
        .border(Tokens.Elevation.HAIRLINE.dp, if (selected) r.accent else r.line, shape)
        .then(if (selected) Modifier.selectionBar(r.accentStrong) else Modifier)
        .drawBehind {
            val width = Tokens.Elevation.SELECTION_BAR.dp.toPx()
            if (before || after) drawRect(r.accentStrong, Offset(0f, if (before) 0f else size.height - width), Size(size.width, width))
        }
        .clickable(enabled = !origin && look?.gone != true) {
            if (input.pager.currentPage != stage.ordinal) input.pager.requestScrollToPage(stage.ordinal)
            input.adding = null
            actions.select(task.id)
        }
        .semantics {
            if (canEdit) customActions = listOf(CustomAccessibilityAction("Move to column…") { onMove(); true })
        }
        .padding(vertical = S.S2.dp, horizontal = S.S1.dp),
    ) {
        Check(motion.checked(task.id) ?: task.done, { actions.setDone(task.id, it, state.generation) }, task.title,
            modifier = Modifier.align(Alignment.CenterStart),
            overdue = task.due?.tone == DueTone.OVERDUE, enabled = canEdit, look = look, onWash = selected)
        val important = task.priority == 3.toUByte()
        Column(Modifier.fillMaxWidth().padding(start = P.TOUCH.dp - S.S2.dp, top = S.S1.dp, bottom = S.S1.dp, end = S.S2.dp),
            verticalArrangement = Arrangement.spacedBy(S.S2.dp)) {
            Label(task.title, Tokens.Type.NAV, if (task.done || origin) r.ink3 else r.ink,
                Modifier.padding(end = if (important) P.TOUCH.dp - S.S3.dp else 0.dp), maxLines = 2,
                decoration = if (task.done || look?.strike == 1f) androidx.compose.ui.text.style.TextDecoration.LineThrough else null)
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(S.S1.dp)) {
                androidx.compose.foundation.layout.FlowRow(Modifier.weight(1f), horizontalArrangement = Arrangement.spacedBy(S.S2.dp),
                    verticalArrangement = Arrangement.spacedBy(S.S1.dp)) {
                    task.due?.let { TaskDate(task, it) }
                    task.steps?.let { StepMeter(it) }
                    if (task.priority > 0u) Priority(task.priority.toInt())
                    if (task.fromMail) Icon("mail", r.ink3, size = C.ICON_SMALL.dp, label = "From an email")
                    task.tags.take(2).forEach { TagLabel(it.label, it.hue) }
                    if (task.tags.size > 2) Label("+${task.tags.size - 2}", Tokens.Type.COUNT, r.ink3)
                }
                task.agent?.let { AgentBadge(it) }
            }
        }
        if (important) Box(Modifier.align(Alignment.TopEnd)) {
            TaskToggleButton(TaskToggle.IMPORTANT, true, enabled = canEdit,
                label = "Important: ${task.title}", onChange = { actions.setImportant(task.id, it) })
        }
    }
}

@Composable
private fun DoneRail(count: Int, input: BoardInput) {
    val r = LocalDesign.current.room
    val stage = WorkflowStage.DONE
    val target = input.target?.stage == stage
    val shape = RoundedCornerShape(Tokens.Radius.LG.dp)
    DisposableEffect(input) { onDispose { input.columns.remove(stage) } }
    Column(Modifier.width(B.COL_COLLAPSED.dp).fillMaxHeight().clip(shape)
        .background(if (target) r.wash else r.canvas)
        .border(Tokens.Elevation.HAIRLINE.dp, if (target) r.accentStrong else r.line, shape)
        .onGloballyPositioned { input.columns[stage] = it.boundsInRoot() }
        .clickable(role = Role.Button) { input.doneExpanded = true }
        .semantics { contentDescription = "Expand Done, $count recent tasks" }
        .padding(top = S.S3.dp), horizontalAlignment = Alignment.CenterHorizontally,
        verticalArrangement = Arrangement.spacedBy(S.S2.dp)) {
        Icon(stage.icon, stage.ink())
        Layout(content = { Label("Done · $count", Tokens.Type.BUTTON, r.ink2) }) { children, constraints ->
            val label = children.single().measure(androidx.compose.ui.unit.Constraints(maxWidth = constraints.maxHeight))
            layout(label.height, label.width) {
                label.placeWithLayer((label.height - label.width) / 2, (label.width - label.height) / 2) { rotationZ = -90f }
            }
        }
    }
}

@Composable
private fun BoardFooter(label: String, icon: String, dashed: Boolean, enabled: Boolean = true, click: () -> Unit) {
    val r = LocalDesign.current.room
    Row(Modifier.fillMaxWidth().heightIn(min = P.TOUCH.dp)
        .then(if (dashed) Modifier.dashedWell(r.lineStrong) else Modifier)
        .clickable(enabled = enabled, role = Role.Button, onClick = click).padding(horizontal = S.S2.dp),
        verticalAlignment = Alignment.CenterVertically, horizontalArrangement = if (dashed) Arrangement.Center else Arrangement.Start) {
        Icon(icon, r.accentStrong, size = C.ICON_SMALL.dp)
        Spacer(Modifier.width(S.S2.dp))
        Label(label, Tokens.Type.BUTTON_SMALL, r.ink2)
    }
}

private fun Modifier.dashedWell(color: androidx.compose.ui.graphics.Color) = drawBehind {
    val line = Tokens.Elevation.HAIRLINE.dp.toPx()
    val radius = CornerRadius(Tokens.Radius.MD.dp.toPx())
    val dash = S.S1.dp.toPx()
    drawRoundRect(color, topLeft = Offset(line / 2, line / 2), size = Size(size.width - line, size.height - line),
        cornerRadius = radius, style = Stroke(line, pathEffect = PathEffect.dashPathEffect(floatArrayOf(dash, dash))))
}

@Composable
internal fun BoardColumnStrip(state: UiState, input: BoardInput, left: Boolean) {
    val r = LocalDesign.current.room
    val rows = remember(state.board, state.room, state.boardReopening) {
        state.board?.visibleCards(state.room, state.boardReopening).orEmpty()
    }
    Row(Modifier.fillMaxWidth().heightIn(min = P.BOTTOM_BAR.dp).background(r.pane).hairline(r.line, top = true)
        .padding(horizontal = S.S1.dp).selectableGroup(), verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(S.S1.dp)) {
        if (left && input.canAdd) BoardAdd(input, !state.signedOut)
        stages.forEach { stage ->
            val selected = stage == input.active
            val target = stage == input.target?.stage
            val count = rows[stage].orEmpty().size
            val shape = RoundedCornerShape(Tokens.Radius.MD.dp)
            val label = if (stage == WorkflowStage.DONE) "" else stage.label
            DisposableEffect(input, stage) { onDispose { input.tabs.remove(stage) } }
            val weight = when (stage) {
                WorkflowStage.BACKLOG -> 1.2f
                WorkflowStage.TODO -> 1f
                WorkflowStage.IN_PROGRESS -> 1.55f
                WorkflowStage.DONE -> 0.65f
            }
            Box(Modifier.weight(weight)
                .height(P.TOUCH.dp).onGloballyPositioned { input.tabs[stage] = it.boundsInRoot() }
                .selectable(selected, role = Role.Tab) { input.page(stage.ordinal) }
                .semantics { contentDescription = "${stage.label}, $count tasks" }, contentAlignment = Alignment.Center) {
                Row(Modifier.fillMaxWidth().heightIn(min = B.HEADER_HEIGHT.dp).background(if (selected || target) r.wash else r.canvas, shape)
                    .border(Tokens.Elevation.HAIRLINE.dp, if (selected || target) r.accentStrong else r.line, shape)
                    .padding(horizontal = S.S1.dp), verticalAlignment = Alignment.CenterVertically,
                    horizontalArrangement = Arrangement.spacedBy(S.S1.dp, Alignment.CenterHorizontally)) {
                    Icon(stage.icon, stage.ink(selected || target), size = C.ICON_SMALL.dp)
                    if (label.isNotEmpty()) Label(label, Tokens.Type.BUTTON_SMALL, if (selected) r.accentStrong else r.ink, Modifier.weight(1f, fill = false))
                    Label(count.toString(), Tokens.Type.COUNT, r.ink2)
                }
            }
        }
        if (!left && input.canAdd) BoardAdd(input, !state.signedOut)
    }
}

@Composable
private fun BoardAdd(input: BoardInput, enabled: Boolean) {
    val r = LocalDesign.current.room
    val shape = RoundedCornerShape(Tokens.Radius.LG.dp)
    Box(Modifier.size(P.TOUCH.dp).clip(shape).background(if (enabled) r.accentFill else r.wash)
        .clickable(enabled = enabled, role = Role.Button) { input.adding = input.active.openStage() }
        .semantics { contentDescription = "Add to ${input.active.label}" }, contentAlignment = Alignment.Center) {
        Icon("plus", if (enabled) r.onAccent else r.ink3)
    }
}

@Composable
internal fun BoardAddLabel(add: () -> Unit) {
    val r = LocalDesign.current.room
    val shape = RoundedCornerShape(Tokens.Radius.MD.dp)
    Box(Modifier.heightIn(min = P.TOUCH.dp).clickable(role = Role.Button, onClick = add), contentAlignment = Alignment.Center) {
        Row(Modifier.heightIn(min = B.HEADER_HEIGHT.dp).background(r.accentFill, shape).padding(horizontal = S.S3.dp),
            verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(S.S2.dp)) {
            Icon("plus", r.onAccent, size = C.ICON_SMALL.dp)
            Label("Add task", Tokens.Type.BUTTON, r.onAccent)
        }
    }
}

@Composable
internal fun BoardLift(input: BoardInput) {
    val card = input.picked
    val scale by animateFloatAsState(if (card != null && !input.reduced) M.DRAG_TOUCH_SCALE else 1f,
        tween(if (input.reduced) 0 else M.HOVER, easing = M.EASE_STANDARD.cubic()), label = "board lift")
    if (card == null) return
    val d = LocalDesign.current
    val shape = RoundedCornerShape(Tokens.Radius.LG.dp)
    Box(Modifier.offset {
        val root = input.root?.takeIf { it.isAttached }
        val origin = root?.localToRoot(Offset.Zero) ?: Offset.Zero
        val max = ((root?.size?.width ?: 0) - (P.INNER_LIST.dp / 2).toPx()).coerceAtLeast(0f)
        IntOffset((input.point.x - origin.x + S.S3.dp.toPx()).coerceIn(0f, max).roundToInt(),
            (input.point.y - origin.y - P.TOUCH.dp.toPx()).coerceAtLeast(0f).roundToInt())
    }.graphicsLayer { scaleX = scale; scaleY = scale }.widthIn(max = P.INNER_LIST.dp / 2).overlay(shape, d).padding(S.S3.dp)) {
        Label(card.task.title, Tokens.Type.TASK_TITLE_PHONE, d.room.ink, maxLines = 2)
    }
}
