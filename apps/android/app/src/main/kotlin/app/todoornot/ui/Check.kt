package app.todoornot.ui

import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.animation.core.tween
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.selection.toggleable
import androidx.compose.material3.minimumInteractiveComponentSize
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.graphics.PathMeasure
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.graphics.StrokeJoin
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import app.todoornot.design.LocalDesign
import app.todoornot.design.Tokens
import app.todoornot.design.cubic
import app.todoornot.model.RowLook

private const val TICK_BOX = 24f

/** The tick, in a 24-unit box (Lucide's `check`). */
private val tickPoints = listOf(Offset(20f, 6f), Offset(9f, 17f), Offset(4f, 12f)).reversed()

/**
 * The round checkbox. Checking it fills it with the accent (with a small overshoot) and then draws the
 * tick; an overdue task gets a danger ring. The visual is 20 dp; the touch target is 48.
 *
 * A task row passes its [look] (its choreography's frame, or its rest): the disc and the tick are drawn
 * exactly as that says. Without one (the detail pane) the box animates by itself.
 */
@Composable
fun Check(
    done: Boolean,
    onToggle: (Boolean) -> Unit,
    label: String,
    modifier: Modifier = Modifier,
    overdue: Boolean = false,
    size: Dp = Tokens.Metrics.Component.CHECKBOX.dp,
    enabled: Boolean = true,
    look: RowLook? = null,
    onWash: Boolean = false,
    onSelectionWash: Boolean = false,
) {
    val r = LocalDesign.current.room
    val m = Tokens.Motion
    val target = if (done) 1f else 0f
    val animatedFill by animateFloatAsState(
        target,
        tween(m.CHECK_FILL, easing = m.EASE_POP.cubic()),
        label = "check-fill",
    )
    val animatedTick by animateFloatAsState(
        target,
        tween(m.CHECK_TICK, delayMillis = if (done) m.CHECK_TICK_DELAY else 0, easing = m.EASE_STANDARD.cubic()),
        label = "check-tick",
    )
    // The detail pane's box grows its disc; a row's disc fades in at its (popping) scale.
    val fill = look?.fill ?: 1f
    val scale = look?.scale ?: animatedFill
    val tick = look?.tick ?: animatedTick
    val path = remember { Path() }
    val measure = remember { PathMeasure() }
    val cut = remember { Path() }
    Box(
        modifier
            .minimumInteractiveComponentSize()
            .toggleable(done, enabled = enabled, role = Role.Checkbox, onValueChange = onToggle)
            .semantics { contentDescription = label },
        contentAlignment = Alignment.Center,
    ) {
        Canvas(Modifier.size(size)) {
            val ring = Tokens.Metrics.Component.CHECKBOX_RING.dp.toPx()
            val radius = this.size.minDimension / 2
            if (overdue && !done) drawCircle(r.dangerBg, radius - ring / 2)
            drawCircle(if (overdue && !done) r.danger else r.control, radius - ring / 2, style = Stroke(ring))
            val disc = if (look == null) (radius * scale).coerceAtMost(radius * 1.08f) else radius * scale
            val color = when {
                onSelectionWash -> r.selectionCheck
                onWash -> r.checkSelected
                else -> r.accent
            }
            if (fill > 0f && scale > 0f) drawCircle(color.copy(alpha = fill), disc)
            if (tick > 0f && fill > 0f) {
                // The tick sits in 76% of the box, as in the mockups.
                val inner = this.size.minDimension * 0.76f
                val unit = inner / TICK_BOX
                val o = (this.size.minDimension - inner) / 2
                path.reset()
                tickPoints.forEachIndexed { i, p ->
                    val x = o + p.x * unit
                    val y = o + p.y * unit
                    if (i == 0) path.moveTo(x, y) else path.lineTo(x, y)
                }
                measure.setPath(path, false)
                cut.reset()
                measure.getSegment(0f, measure.length * tick, cut, true)
                drawPath(
                    cut,
                    r.onAccent.copy(alpha = r.onAccent.alpha * fill),
                    style = Stroke(Tokens.Metrics.Component.CHECKBOX_TICK * unit, cap = StrokeCap.Round, join = StrokeJoin.Round),
                )
            }
        }
    }
}
