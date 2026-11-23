package app.todoornot.ui

import android.provider.Settings
import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.animation.core.tween
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.gestures.awaitEachGesture
import androidx.compose.foundation.gestures.awaitFirstDown
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.setValue
import androidx.compose.runtime.staticCompositionLocalOf
import androidx.compose.runtime.withFrameNanos
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.draw.drawWithContent
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Rect
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.hapticfeedback.HapticFeedbackType
import androidx.compose.ui.input.pointer.PointerEventPass
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.layout.LayoutCoordinates
import androidx.compose.ui.layout.boundsInRoot
import androidx.compose.ui.layout.onGloballyPositioned
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.platform.LocalHapticFeedback
import androidx.compose.ui.semantics.clearAndSetSemantics
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.IntOffset
import androidx.compose.ui.unit.dp
import app.todoornot.core.RoomKey
import app.todoornot.core.SmartList
import app.todoornot.core.TaskDrag
import app.todoornot.core.TaskDrop
import app.todoornot.core.TaskRow
import app.todoornot.design.Icon
import app.todoornot.design.LocalDesign
import app.todoornot.design.Tokens
import app.todoornot.design.cubic
import app.todoornot.model.Actions
import kotlin.math.roundToInt

private val P = Tokens.Metrics.Phone
private val S = Tokens.Space
private val M = Tokens.Motion

internal val LocalTaskDrag = staticCompositionLocalOf<DragInput?> { null }

/** Painted by the same components as live drag; galleries set it without injecting touch. */
internal data class DragFrame(val title: String, val ids: List<String>, val target: TaskDrop, val x: Float, val y: Float)
internal val LocalTaskDragFrame = staticCompositionLocalOf<DragFrame?> { null }

internal class DragInput {
    data class Grip(val id: String, val title: String, val bounds: Rect)
    data class ScrollPane(val bounds: Rect, val scroll: (Float) -> Unit)
    data class Picked(val drag: TaskDrag, val ids: List<String>, val title: String)

    var root: LayoutCoordinates? = null
    val grips = mutableMapOf<String, Grip>()
    val rows = mutableMapOf<String, Rect>()
    val nav = mutableMapOf<TaskDrop, Rect>()
    var list: ScrollPane? = null
    var drawer: ScrollPane? = null
    var picked by mutableStateOf<Picked?>(null)
    var position by mutableStateOf(Offset.Zero)
    var target by mutableStateOf<TaskDrop?>(null)

    fun candidate(drawerVisible: Boolean): TaskDrop? {
        if (drawerVisible) {
            return if (drawer?.bounds?.contains(position) == true) {
                nav.entries.firstOrNull { position in it.value }?.key
            } else null
        }
        if (list?.bounds?.contains(position) != true) return null
        val entry = rows.entries.firstOrNull { position in it.value } ?: return null
        return TaskDrop.Slot(entry.key, position.y >= entry.value.center.y)
    }

    fun scroll(drawerVisible: Boolean, edge: Float, maxStep: Float) {
        val pane = (if (drawerVisible) drawer else list) ?: return
        if (position !in pane.bounds) return
        val delta = edgeScroll(position.y - pane.bounds.top, pane.bounds.bottom - position.y, edge, maxStep)
        if (delta != 0f) pane.scroll(delta)
    }

    fun cancel() {
        picked = null
        target = null
    }
}

internal fun edgeScroll(top: Float, bottom: Float, edge: Float, maxStep: Float): Float {
    fun strength(gap: Float) = (1f - gap / edge).coerceIn(0f, 1f)
    return (strength(bottom) - strength(top)) * maxStep
}

internal fun RoomKey.taskDrop(): TaskDrop? = when (this) {
    is RoomKey.List -> TaskDrop.List(id)
    RoomKey.Smart(SmartList.MY_DAY) -> TaskDrop.MyDay
    else -> null
}

@Composable
internal fun taskDragInput(
    input: DragInput,
    actions: Actions,
    generation: Long,
    drawerVisible: Boolean,
    openDrawer: () -> Unit,
    closeDrawer: () -> Unit,
): Modifier {
    val haptic = LocalHapticFeedback.current
    val drawer by rememberUpdatedState(drawerVisible)
    val open by rememberUpdatedState(openDrawer)
    val close by rememberUpdatedState(closeDrawer)
    val density = LocalDensity.current
    fun valid(): TaskDrop? = input.picked?.let { p ->
        input.candidate(drawer)?.takeIf { actions.taskDropValid(p.drag, it, generation) }
    }
    LaunchedEffect(input.picked, generation) {
        if (input.picked == null) return@LaunchedEffect
        val edge = with(density) { S.S8.dp.toPx() }
        val step = with(density) { S.S3.dp.toPx() }
        while (input.picked != null) withFrameNanos {
            input.scroll(drawer, edge, step)
            val target = valid()
            if (target != input.target && target is TaskDrop.Slot) {
                haptic.performHapticFeedback(HapticFeedbackType.TextHandleMove)
            }
            input.target = target
        }
    }
    return Modifier.onGloballyPositioned { input.root = it }.pointerInput(input, actions, generation, density) {
        // Capture on the pane, never the lazy row: the source can scroll out of composition mid-drag.
        awaitEachGesture {
            val down = awaitFirstDown(requireUnconsumed = false, pass = PointerEventPass.Initial)
            val root = input.root?.takeIf { it.isAttached } ?: return@awaitEachGesture
            val start = root.localToRoot(down.position)
            val grip = input.grips.values.firstOrNull { start in it.bounds } ?: return@awaitEachGesture
            if (drawer) return@awaitEachGesture
            down.consume()
            var opened = false
            try {
                while (true) {
                    val change = awaitPointerEvent(PointerEventPass.Initial).changes.firstOrNull { it.id == down.id } ?: break
                    if (!root.isAttached || change.isConsumed) break
                    input.position = root.localToRoot(change.position)
                    if (!change.pressed) {
                        change.consume()
                        val p = input.picked
                        val target = valid()
                        if (p != null && target != null) actions.dropTasks(p.drag, target, generation)
                        break
                    }
                    if (input.picked == null && (input.position - start).getDistance() >= viewConfiguration.touchSlop) {
                        val plan = actions.startTaskDrag(grip.id, generation) ?: break
                        input.picked = DragInput.Picked(plan, plan.ids(), grip.title)
                        haptic.performHapticFeedback(HapticFeedbackType.LongPress)
                    }
                    change.consume()
                    if (input.picked != null && !drawer && !opened && input.position.x <= with(density) { S.S5.dp.toPx() }) {
                        opened = true
                        open()
                    }
                }
            } finally {
                input.cancel()
                if (opened) close()
            }
        }
    }
}

@Composable
internal fun DragGrip(task: TaskRow, manual: Boolean) {
    val input = LocalTaskDrag.current
    val r = LocalDesign.current.room
    DisposableEffect(input, task.id) {
        onDispose { input?.grips?.remove(task.id) }
    }
    Box(Modifier.size(P.TOUCH.dp).onGloballyPositioned {
        input?.grips?.set(task.id, DragInput.Grip(task.id, task.title, it.boundsInRoot()))
    }.semantics {
        contentDescription = if (manual) "Drag to reorder, or drag to the left edge for Lists" else "Drag to the left edge for Lists or My Day"
    }, contentAlignment = Alignment.TopCenter) {
        Icon("grip-vertical", r.ink3)
    }
}

@Composable
internal fun taskDragRow(id: String, reorderable: Boolean): Modifier {
    val input = LocalTaskDrag.current
    val fixture = LocalTaskDragFrame.current
    val target = (fixture?.target ?: input?.target) as? TaskDrop.Slot
    val picked = id in (fixture?.ids ?: input?.picked?.ids.orEmpty())
    val r = LocalDesign.current.room
    DisposableEffect(input, id, reorderable) { onDispose { if (reorderable) input?.rows?.remove(id) } }
    return Modifier
        .then(if (reorderable) Modifier.onGloballyPositioned { input?.rows?.set(id, it.boundsInRoot()) } else Modifier)
        .then(if (target?.row == id) Modifier.drawWithContent {
            drawContent()
            val h = Tokens.Elevation.SELECTION_BAR.dp.toPx()
            val inset = S.S2.dp.toPx()
            drawRect(r.accentStrong, Offset(inset, if (target.after) size.height - h else 0f), Size((size.width - 2 * inset).coerceAtLeast(0f), h))
        } else Modifier)
        .then(if (picked) Modifier.alpha(1f - Tokens.Elevation.DROP_WASH_ALPHA) else Modifier)
}

@Composable
internal fun taskDragNav(target: TaskDrop): Pair<Modifier, Boolean> {
    val input = LocalTaskDrag.current
    val fixture = LocalTaskDragFrame.current
    DisposableEffect(input, target) { onDispose { input?.nav?.remove(target) } }
    return Modifier.onGloballyPositioned { input?.nav?.set(target, it.boundsInRoot()) } to
        ((fixture?.target ?: input?.target) == target)
}

@Composable
internal fun TaskDragSurface(input: DragInput, modifier: Modifier, content: @Composable () -> Unit) {
    CompositionLocalProvider(LocalTaskDrag provides input) {
        Box(Modifier.fillMaxSize().then(modifier)) {
            content()
            DragLift(input)
        }
    }
}

@Composable
private fun DragLift(input: DragInput) {
    val fixture = LocalTaskDragFrame.current
    val p = input.picked
    val context = LocalContext.current
    val active = p != null || fixture != null
    val reduced = remember(context, active) {
        Settings.Global.getFloat(context.contentResolver, Settings.Global.ANIMATOR_DURATION_SCALE, 1f) == 0f
    }
    val size by animateFloatAsState(if (active && !reduced) M.DRAG_TOUCH_SCALE else 1f, tween(M.HOVER, easing = M.EASE_STANDARD.cubic()), label = "drag lift")
    val title = fixture?.title ?: p?.title ?: return
    val d = LocalDesign.current
    val r = d.room
    val width = Tokens.Metrics.Desktop.MENU.dp
    val shape = RoundedCornerShape(Tokens.Radius.BUTTON.dp)
    val count = fixture?.ids?.size ?: p?.ids?.size ?: 1
    Row(
        Modifier.offset {
            val point = fixture?.let { Offset(it.x.dp.toPx(), it.y.dp.toPx()) } ?: input.position
            val margin = S.S2.dp.toPx()
            val root = input.root
            val origin = root?.takeIf { it.isAttached }?.localToRoot(Offset.Zero) ?: Offset.Zero
            val x = (point.x - origin.x + S.S3.dp.toPx()).coerceIn(margin, ((root?.size?.width ?: 0) - width.toPx() - margin).coerceAtLeast(margin))
            val y = (point.y - origin.y - P.TOUCH.dp.toPx() / 2).coerceAtLeast(margin)
            IntOffset(x.roundToInt(), y.roundToInt())
        }.graphicsLayer { scaleX = size; scaleY = size }.widthIn(max = width).heightIn(min = P.TOUCH.dp)
            .overlay(shape, d).background(r.pane).then(if (reduced) Modifier.border(Tokens.Elevation.HAIRLINE.dp, r.accent, shape) else Modifier)
            .clearAndSetSemantics { }.padding(horizontal = S.S3.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(S.S2.dp),
    ) {
        Icon("grip-vertical", r.ink3)
        Label(title, Tokens.Type.TASK_TITLE_PHONE, r.ink, Modifier.weight(1f, fill = false))
        if (count > 1) {
            Box(Modifier.background(r.selectionWash, shape).padding(horizontal = S.S2.dp)) {
                Label("$count", Tokens.Type.COUNT, r.ink)
            }
        }
    }
}
