package app.todoornot.ui

import android.provider.Settings
import androidx.compose.animation.core.Animatable
import androidx.compose.animation.core.tween
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.gestures.awaitEachGesture
import androidx.compose.foundation.gestures.awaitFirstDown
import androidx.compose.foundation.gestures.awaitLongPressOrCancellation
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.runtime.Composable
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
import androidx.compose.ui.draw.drawWithContent
import androidx.compose.ui.geometry.CornerRadius
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
import androidx.compose.ui.layout.positionInRoot
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.platform.LocalHapticFeedback
import androidx.compose.ui.semantics.clearAndSetSemantics
import androidx.compose.ui.unit.IntOffset
import androidx.compose.ui.unit.dp
import app.todoornot.core.NavItem
import app.todoornot.core.RoomKey
import app.todoornot.core.SidebarDrag
import app.todoornot.core.SidebarDrop
import app.todoornot.design.Glyph
import app.todoornot.design.LocalDesign
import app.todoornot.design.Tokens
import app.todoornot.design.cubic
import app.todoornot.model.Actions
import kotlin.math.roundToInt

private val S = Tokens.Space
private val P = Tokens.Metrics.Phone
private val M = Tokens.Motion

// Galleries paint the same indicator and preview without injecting a touch gesture.
internal data class SidebarDragFrame(val item: NavItem, val target: SidebarDrop, val x: Float, val y: Float)
internal val LocalSidebarDragFrame = staticCompositionLocalOf<SidebarDragFrame?> { null }

internal class SidebarInput {
    data class Measured(val item: NavItem, val bounds: Rect, val middle: Float)
    data class Picked(val plan: SidebarDrag, val item: NavItem)
    val rows = mutableMapOf<RoomKey, Measured>()
    var pane: LayoutCoordinates? = null
    var picked by mutableStateOf<Picked?>(null)
    var position by mutableStateOf(Offset.Zero)
    var target by mutableStateOf<SidebarDrop?>(null)
    var moving by mutableStateOf(false)
    var settle by mutableStateOf(0)

    fun hit(at: Offset): Measured? {
        if (pane?.takeIf { it.isAttached }?.boundsInRoot()?.contains(at) != true) return null
        return rows.values.firstOrNull { at in it.bounds }
    }
}

@Composable
internal fun sidebarInputModifier(input: SidebarInput, enabled: Boolean, actions: Actions, generation: Long,
    collapsed: Set<String>, scroll: (Float) -> Unit, menu: (NavItem) -> Unit): Modifier {
    val haptic = LocalHapticFeedback.current
    val density = LocalDensity.current
    val collapsedNow by rememberUpdatedState(collapsed)
    val scrollNow by rememberUpdatedState(scroll)
    val menuNow by rememberUpdatedState(menu)
    fun candidate(): SidebarDrop? {
        val p = input.picked ?: return null
        val hit = input.hit(input.position) ?: return null
        return actions.sidebarDropTarget(p.plan, hit.item.key, input.position.y >= hit.middle, generation)
    }
    LaunchedEffect(input.picked) {
        var dwell = 0L
        var expanded: String? = null
        val edge = with(density) { S.S8.dp.toPx() }
        val step = with(density) { S.S3.dp.toPx() }
        while (input.picked != null) withFrameNanos { now ->
            if (!input.moving) return@withFrameNanos
            input.pane?.takeIf { it.isAttached }?.boundsInRoot()?.let { bounds ->
                if (input.position in bounds) {
                    val delta = edgeScroll(input.position.y - bounds.top, bounds.bottom - input.position.y, edge, step)
                    if (delta != 0f) scrollNow(delta)
                }
            }
            val target = candidate()
            if (target != input.target) {
                dwell = now
                input.target = target
                expanded = null
            }
            if (target is SidebarDrop.Into && target.collection != expanded && target.collection in collapsedNow &&
                now - dwell >= M.SIDEBAR_EXPAND_DWELL * 1_000_000L) {
                expanded = target.collection
                actions.toggleCollection(target.collection)
            }
        }
    }
    // Capture on the stable scroll pane. The source row can scroll out of composition mid-drag.
    return Modifier.onGloballyPositioned { input.pane = it }.pointerInput(input, enabled, actions, generation) {
        if (!enabled) return@pointerInput
        awaitEachGesture {
            val down = awaitFirstDown(requireUnconsumed = false, pass = PointerEventPass.Initial)
            val pane = input.pane?.takeIf { it.isAttached } ?: return@awaitEachGesture
            val start = pane.localToRoot(down.position)
            val source = input.hit(start)?.item ?: return@awaitEachGesture
            val hold = awaitLongPressOrCancellation(down.id) ?: return@awaitEachGesture
            val plan = actions.startSidebarDrag(source.key, generation) ?: return@awaitEachGesture
            val pickup = pane.localToRoot(hold.position)
            input.position = pickup
            input.picked = SidebarInput.Picked(plan, source)
            haptic.performHapticFeedback(HapticFeedbackType.LongPress)
            try {
                while (true) {
                    val change = awaitPointerEvent(PointerEventPass.Initial).changes.firstOrNull { it.id == down.id } ?: break
                    if (!pane.isAttached || change.isConsumed) break
                    input.position = pane.localToRoot(change.position)
                    input.moving = input.moving || (input.position - pickup).getDistance() >= viewConfiguration.touchSlop
                    change.consume()
                    if (!change.pressed) {
                        if (!input.moving) menuNow(source)
                        else candidate()?.let { target ->
                            input.settle++
                            actions.dropSidebarRow(plan, target, generation)
                            haptic.performHapticFeedback(HapticFeedbackType.Confirm)
                        }
                        break
                    }
                }
            } finally {
                input.picked = null
                input.target = null
                input.moving = false
            }
        }
    }
}

@Composable
internal fun sidebarRowProbe(input: SidebarInput, item: NavItem): Modifier {
    DisposableEffect(input, item.key) { onDispose { input.rows.remove(item.key) } }
    return Modifier.onGloballyPositioned {
        input.rows[item.key] = SidebarInput.Measured(item, it.boundsInRoot(), it.positionInRoot().y + it.size.height / 2f)
    }
}

@Composable
internal fun insertionLine(slot: SidebarDrop.Beside?, indent: Boolean): Modifier {
    if (slot == null) return Modifier
    val ink = LocalDesign.current.room.accentStrong
    return Modifier.drawWithContent {
        drawContent()
        val h = Tokens.Elevation.SELECTION_BAR.dp.toPx()
        val left = (if (indent) S.S8 else S.S2).dp.toPx()
        val right = S.S2.dp.toPx()
        // Radius is limited by the line's short edge, not the row corners.
        val radius = Tokens.Radius.BUTTON.dp.toPx().coerceAtMost(h / 2)
        drawRoundRect(ink, Offset(left, if (slot.after) size.height - h else 0f),
            Size((size.width - left - right).coerceAtLeast(0f), h), CornerRadius(radius))
    }
}

@Composable
internal fun SidebarLift(input: SidebarInput, root: LayoutCoordinates?) {
    val fixture = LocalSidebarDragFrame.current
    val item = fixture?.item ?: input.picked?.item ?: return
    if (fixture == null && root == null) return
    val d = LocalDesign.current
    val r = d.room
    val shape = RoundedCornerShape(Tokens.Radius.BUTTON.dp)
    val width = Tokens.Metrics.Desktop.MENU.dp
    val context = LocalContext.current
    val reduced = remember(context) { Settings.Global.getFloat(context.contentResolver, Settings.Global.ANIMATOR_DURATION_SCALE, 1f) == 0f }
    val scale = remember(item.key) { Animatable(if (fixture != null && !reduced) M.DRAG_TOUCH_SCALE else 1f) }
    LaunchedEffect(item.key) {
        if (!reduced && fixture == null) scale.animateTo(M.DRAG_TOUCH_SCALE, tween(M.HOVER, easing = M.EASE_STANDARD.cubic()))
    }
    Row(Modifier.offset {
        if (fixture != null) {
            // The still shot does not depend on whether the sheet's first layout callback ran.
            return@offset IntOffset((fixture.x + S.S3).dp.roundToPx(), (fixture.y + S.S8).dp.roundToPx())
        }
        val origin = root?.takeIf { it.isAttached }?.localToRoot(Offset.Zero) ?: Offset.Zero
        val point = input.position - origin
        val margin = S.S2.dp.toPx()
        val maxX = ((root?.size?.width ?: 0) - width.toPx() - margin).coerceAtLeast(margin)
        val maxY = ((root?.size?.height ?: 0) - P.NAV_ROW.dp.toPx() - margin).coerceAtLeast(margin)
        val x = (point.x + S.S3.dp.toPx()).coerceIn(margin, maxX)
        // Keep the highlighted header and its label visible above the lifted row.
        val below = point.y + S.S8.dp.toPx()
        val y = (if (below <= maxY) below else point.y - S.S8.dp.toPx() - P.NAV_ROW.dp.toPx()).coerceIn(margin, maxY)
        IntOffset(x.roundToInt(), y.roundToInt())
    }.graphicsLayer { scaleX = scale.value; scaleY = scale.value }.widthIn(max = width).height(P.NAV_ROW.dp)
        .overlay(shape, d).background(r.pane, shape)
        .border(Tokens.Elevation.HAIRLINE.dp, if (reduced) r.accent else r.lineStrong, shape)
        .clearAndSetSemantics { }.padding(horizontal = S.S3.dp),
        verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(S.S2.dp)) {
        item.glyph?.let { Glyph(it, d.list(item.hue).glyph) } ?: Box(Modifier.widthIn(min = Tokens.Metrics.Component.GLYPH.dp))
        Label(item.name, Tokens.Type.NAV, r.ink, Modifier.weight(1f, fill = false))
        if (item.open > 0u) Label("${item.open}", Tokens.Type.COUNT, r.ink3)
    }
}
