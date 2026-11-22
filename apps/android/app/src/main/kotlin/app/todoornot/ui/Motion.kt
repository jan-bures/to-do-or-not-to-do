package app.todoornot.ui

import androidx.compose.animation.core.Animatable
import androidx.compose.animation.core.tween
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.staticCompositionLocalOf
import androidx.compose.ui.layout.onPlaced
import androidx.compose.ui.layout.positionInParent
import app.todoornot.design.Tokens
import app.todoornot.design.cubic
import kotlinx.coroutines.launch
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clipToBounds
import androidx.compose.ui.draw.drawWithContent
import androidx.compose.ui.graphics.drawscope.clipRect
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.layout.layout
import androidx.compose.ui.unit.Constraints
import androidx.compose.ui.unit.dp
import app.todoornot.model.DuskLook
import app.todoornot.model.MotionFrame
import app.todoornot.model.RowLook
import kotlin.math.roundToInt

/** The screen's motion, sampled at the frame being drawn ([MotionFrame.Rest] in the gallery's still shots). */
val LocalMotion = staticCompositionLocalOf<MotionFrame> { MotionFrame.Rest }

/**
 * A row in its choreography: faded to its opacity, and laid out at its share of its height so the rows
 * below close the gap as it collapses (and open it as it enters). At rest ([look] is null) it changes
 * nothing.
 */
fun Modifier.rowMotion(look: RowLook?): Modifier {
    if (look == null) return this
    return graphicsLayer { alpha = look.opacity }
        .clipToBounds()
        .layout { measurable, constraints ->
            val p = measurable.measure(constraints.copy(minHeight = 0, maxHeight = Constraints.Infinity))
            val h = if (look.height >= 1f) p.height else (p.height * look.height).roundToInt()
            layout(p.width, h.coerceIn(constraints.minHeight, constraints.maxHeight)) { p.place(0, 0) }
        }
}

/**
 * Item [i] of tomorrow's hand-over while dusk falls: faded in and risen into place after the doc's cue,
 * the first few staggered ([DuskLook.handover]). Read at draw time; at rest it changes nothing.
 */
fun Modifier.handover(i: Int, dusk: () -> DuskLook): Modifier = graphicsLayer {
    val (shown, rise) = dusk().handover(i)
    alpha = shown
    translationY = rise.dp.toPx()
}

/** Draws only the part of the content from [from] to [to] of its width (shares, 0 to 1). */
fun Modifier.clipShare(from: Float, to: Float): Modifier = drawWithContent {
    clipRect(left = size.width * from, right = size.width * to) { this@drawWithContent.drawContent() }
}

/**
 * A sibling reorder (`docs/design-language.md`, Motion, 13): while [animate] is on (the owner just moved
 * this row, or the one it swaps with), a change of place glides from the old one over `ROW_ENTER`
 * (`STANDARD`), retargeting from where it is on a quick second move. Otherwise it moves at once: a change
 * from elsewhere never slides rows. With the animator scale at 0 it snaps.
 */
@Composable
fun Modifier.slideOnMove(animate: Boolean): Modifier {
    val offset = remember { Animatable(0f) }
    val scope = rememberCoroutineScope()
    val last = remember { floatArrayOf(Float.NaN) }
    val on by rememberUpdatedState(animate)
    return this
        .onPlaced { c ->
            val y = c.positionInParent().y
            val before = last[0]
            last[0] = y
            if (on && !before.isNaN() && before != y) {
                scope.launch {
                    offset.snapTo(offset.value + before - y)
                    offset.animateTo(0f, tween(Tokens.Motion.ROW_ENTER, easing = Tokens.Motion.EASE_STANDARD.cubic()))
                }
            }
        }
        .graphicsLayer { translationY = offset.value }
}
