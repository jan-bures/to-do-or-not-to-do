package app.todoornot.ui

import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.animation.core.snap
import androidx.compose.animation.core.tween
import androidx.compose.foundation.background
import androidx.compose.foundation.gestures.awaitEachGesture
import androidx.compose.foundation.gestures.awaitFirstDown
import androidx.compose.foundation.gestures.awaitHorizontalTouchSlopOrCancellation
import androidx.compose.foundation.gestures.horizontalDrag
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.padding
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.setValue
import androidx.compose.runtime.staticCompositionLocalOf
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clipToBounds
import androidx.compose.ui.hapticfeedback.HapticFeedbackType
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.input.pointer.positionChange
import androidx.compose.ui.layout.onGloballyPositioned
import androidx.compose.ui.layout.positionInWindow
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.platform.LocalHapticFeedback
import androidx.compose.ui.platform.LocalView
import androidx.compose.ui.semantics.CustomAccessibilityAction
import androidx.compose.ui.semantics.clearAndSetSemantics
import androidx.compose.ui.semantics.customActions
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.IntOffset
import androidx.compose.ui.unit.dp
import app.todoornot.design.Icon
import app.todoornot.design.LocalDesign
import app.todoornot.design.Tokens
import app.todoornot.design.cubic
import app.todoornot.model.TaskSwipe
import kotlin.math.roundToInt

internal fun swipeDirection(delta: Float, threshold: Float): TaskSwipe? = when {
    delta >= threshold -> TaskSwipe.COMPLETION
    delta <= -threshold -> TaskSwipe.MY_DAY
    else -> null
}

internal fun insideSwipeEdges(x: Float, windowWidth: Int, inset: Float): Boolean =
    x >= inset && x < windowWidth - inset

internal data class SwipeFrame(val task: String, val offsetDp: Float)
internal val LocalSwipeFrame = staticCompositionLocalOf<SwipeFrame?> { null }

/** Horizontal slop is required before taking a pointer away from the list or row controls. */
@Composable
internal fun SwipeableTaskRow(
    task: String, done: Boolean, myDay: Boolean, enabled: Boolean, onSwipe: (TaskSwipe) -> Unit,
    onSelect: () -> Unit, modifier: Modifier = Modifier, content: @Composable (Modifier) -> Unit,
) {
    val r = LocalDesign.current.room
    val p = Tokens.Metrics.Phone
    val m = Tokens.Motion
    val density = LocalDensity.current
    val threshold = with(density) { p.SWIPE_THRESHOLD.dp.toPx() }
    val edge = with(density) { p.EDGE_GESTURE_INSET.dp.toPx() }
    val view = LocalView.current
    val haptics = LocalHapticFeedback.current
    val commit by rememberUpdatedState(onSwipe)
    val reduced = reducedMotion()
    var left by remember { mutableFloatStateOf(0f) }
    var dragging by remember { mutableStateOf(false) }
    var delta by remember { mutableFloatStateOf(0f) }
    val live by animateFloatAsState(delta, if (dragging || reduced) snap() else tween(m.SWIPE_SETTLE, easing = m.EASE_STANDARD.cubic()), label = "row swipe")
    val fixture = LocalSwipeFrame.current?.takeIf { it.task == task }
    val offset = fixture?.let { with(density) { it.offsetDp.dp.toPx() } } ?: live
    val interaction = if (!enabled) Modifier else Modifier.onGloballyPositioned { left = it.positionInWindow().x }
        .pointerInput(task, view, threshold, edge) {
            awaitEachGesture {
                val down = awaitFirstDown(requireUnconsumed = false)
                if (!insideSwipeEdges(left + down.position.x, view.width, edge)) return@awaitEachGesture
                var armed: TaskSwipe? = null
                fun moveBy(dx: Float) {
                    // The selected action comes from the signed distance at release, not the maximum reached.
                    delta = (delta + dx).coerceIn(-threshold * 2, threshold * 2)
                    val action = swipeDirection(delta, threshold)
                    if (action != null && action != armed) haptics.performHapticFeedback(
                        if (action == TaskSwipe.COMPLETION) HapticFeedbackType.Confirm else HapticFeedbackType.TextHandleMove,
                    )
                    armed = action
                }
                try {
                    val start = awaitHorizontalTouchSlopOrCancellation(down.id) { change, overSlop ->
                        dragging = true
                        // Slop chooses the axis; it is still part of the finger's travel.
                        moveBy(overSlop + (if (overSlop >= 0f) viewConfiguration.touchSlop else -viewConfiguration.touchSlop))
                        change.consume()
                    }
                    if (start != null && horizontalDrag(start.id) { change ->
                            moveBy(change.positionChange().x)
                            change.consume()
                        }) armed?.let(commit)
                } finally {
                    dragging = false
                    delta = 0f
                }
            }
        }
    val completionLabel = if (done) "Reopen" else "Complete"
    val dayLabel = if (myDay) "Remove from My Day" else "Add to My Day"
    Box(modifier.fillMaxWidth().clipToBounds().then(interaction)) {
        if (offset != 0f) {
            val forward = offset > 0f
            val color = if (forward) r.accentStrong else r.sunInk
            val icon = if (forward) (if (done) "undo-2" else "circle-check") else (if (myDay) "sun-filled" else "sun")
            Box(Modifier.matchParentSize().background(r.wash).clearAndSetSemantics {},
                contentAlignment = if (forward) Alignment.CenterStart else Alignment.CenterEnd) {
                Icon(icon, color, modifier = Modifier.padding(horizontal = Tokens.Space.S5.dp))
            }
        }
        content(Modifier.offset { IntOffset(offset.roundToInt(), 0) }.then(if (offset != 0f) Modifier.background(r.pane) else Modifier).then(
            if (enabled) Modifier.semantics {
                customActions = listOf(
                    CustomAccessibilityAction(completionLabel) { onSwipe(TaskSwipe.COMPLETION); true },
                    CustomAccessibilityAction(dayLabel) { onSwipe(TaskSwipe.MY_DAY); true },
                    CustomAccessibilityAction("Select task") { onSelect(); true },
                )
            } else Modifier,
        ))
    }
}
