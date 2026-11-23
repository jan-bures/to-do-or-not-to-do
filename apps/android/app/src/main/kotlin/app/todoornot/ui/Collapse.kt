package app.todoornot.ui

import androidx.compose.animation.core.animate
import androidx.compose.animation.core.tween
import androidx.compose.runtime.Stable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clipToBounds
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.input.nestedscroll.NestedScrollConnection
import androidx.compose.ui.input.nestedscroll.NestedScrollSource
import androidx.compose.ui.layout.layout
import androidx.compose.ui.unit.Constraints
import androidx.compose.ui.unit.Velocity
import app.todoornot.design.Tokens
import app.todoornot.design.cubic
import kotlin.math.roundToInt

/** Scroll and reach distances are in pixels, and are mutually exclusive. */
@Stable
class HeaderCollapse(initialReach: Float = 0f) {
    var body by mutableIntStateOf(0)
        internal set
    var compact by mutableIntStateOf(0)
    var scrolled by mutableFloatStateOf(0f)
        private set
    var forced by mutableFloatStateOf(0f)
    var reach by mutableFloatStateOf(initialReach)
        private set
    var reachLimit by mutableFloatStateOf(0f)
    var reachLatch by mutableFloatStateOf(0f)
    var reducedMotion = false
    private var latched = initialReach > 0f

    val range: Float get() = (body - compact).coerceAtLeast(0).toFloat()
    val hidden: Float get() = maxOf(scrolled.coerceAtMost(range), forced * range)
    val fraction: Float get() = if (range == 0f) forced else (hidden / range).coerceIn(0f, 1f)

    /** Consume upward drag before the list, and downward drag only after the list reaches its top. */
    fun consume(dy: Float, allowReach: Boolean = true): Float {
        if (forced > 0f) return 0f
        if (dy < 0f && reach > 0f) latched = false
        val before = reach - scrolled.coerceAtMost(range)
        var next = before + dy
        if (!allowReach && next > 0f) next = minOf(next, reach)
        next = next.coerceIn(-range, reachLimit)
        scrolled = (-next).coerceAtLeast(0f)
        reach = next.coerceAtLeast(0f)
        return next - before
    }

    suspend fun resetReach() {
        latched = false
        animateReach(0f)
    }

    internal fun reachRest(): Float = if (reach >= reachLatch && reach > 0f) reachLimit else 0f

    private suspend fun animateReach(to: Float) {
        animate(reach, to, animationSpec = settleSpec()) { value, _ -> reach = value }
    }

    private fun settleSpec() = tween<Float>(if (reducedMotion) 0 else Tokens.Motion.PANEL, easing = Tokens.Motion.EASE_STANDARD.cubic())

    val connection = object : NestedScrollConnection {
        override fun onPreScroll(available: Offset, source: NestedScrollSource): Offset =
            if (available.y < 0f) Offset(0f, consume(available.y, allowReach = false)) else Offset.Zero

        override fun onPostScroll(consumed: Offset, available: Offset, source: NestedScrollSource): Offset =
            if (available.y > 0f) Offset(0f, consume(available.y, allowReach = source == NestedScrollSource.UserInput)) else Offset.Zero

        override suspend fun onPreFling(available: Velocity): Velocity {
            // Release while stretched: settle the sky without also sending the task list flying.
            if (reach <= 0f) return Velocity.Zero
            val to = if (available.y < 0f && !latched) 0f else reachRest()
            latched = to > 0f
            animateReach(to)
            return Velocity(0f, available.y)
        }

        override suspend fun onPostFling(consumed: Velocity, available: Velocity): Velocity {
            if (forced == 0f && scrolled > 0f && scrolled < range) {
                val to = if (fraction < 0.5f) 0f else range
                animate(scrolled, to, animationSpec = settleSpec()) { value, _ -> scrolled = value }
            }
            return Velocity.Zero
        }
    }
}

/** The expanded scene slides behind a compact title overlay, not behind a second bar. */
fun Modifier.collapsing(state: HeaderCollapse, compactPixels: Int): Modifier =
    clipToBounds().layout { measurable, constraints ->
        val p = measurable.measure(constraints.copy(minHeight = 0, maxHeight = Constraints.Infinity))
        if (state.compact != compactPixels) state.compact = compactPixels
        if (state.body != p.height) state.body = p.height
        val hide = state.hidden.roundToInt().coerceIn(0, p.height)
        layout(p.width, p.height - hide) { p.place(0, -hide) }
    }
