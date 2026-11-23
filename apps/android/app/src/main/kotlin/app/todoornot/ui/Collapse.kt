package app.todoornot.ui

import androidx.compose.animation.core.animate
import androidx.compose.animation.core.tween
import androidx.compose.runtime.Stable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.setValue
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.input.nestedscroll.NestedScrollConnection
import androidx.compose.ui.input.nestedscroll.NestedScrollSource
import androidx.compose.ui.unit.Velocity
import app.todoornot.design.Tokens
import app.todoornot.design.cubic

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
    var reachEnabled = true
    var reducedMotion = false
    private var latched = initialReach > 0f
    private var releasingLatch = false

    val range: Float get() = (body - compact).coerceAtLeast(0).toFloat()
    val hidden: Float get() = maxOf(scrolled.coerceAtMost(range), forced * range)
    val fraction: Float get() = if (range == 0f) forced else (hidden / range).coerceIn(0f, 1f)
    val visibleReach: Float get() = reach.coerceIn(0f, reachLimit)
    val reachFraction: Float get() = if (reachLimit > 0f) visibleReach / reachLimit else 0f
    val reachToText: Float get() = visibleReach * Tokens.Metrics.Phone.HEADER_REACH_TEXT_SHARE
    val reachToScene: Float get() = visibleReach - reachToText

    /** Consume upward drag before the list, and downward drag only after the list reaches its top. */
    fun consume(dy: Float, allowReach: Boolean = true): Float {
        if (forced > 0f) return 0f
        if (dy < 0f && latched) {
            latched = false
            releasingLatch = true
        }
        val before = visibleReach - scrolled.coerceAtMost(range)
        var next = before + dy
        if ((!allowReach || !reachEnabled) && next > 0f) next = minOf(next, visibleReach)
        next = next.coerceIn(-range, reachLimit)
        scrolled = (-next).coerceAtLeast(0f)
        reach = next.coerceAtLeast(0f)
        if (reach == 0f) releasingLatch = false
        return next - before
    }

    suspend fun resetReach() {
        latched = false
        releasingLatch = false
        animateReach(0f)
    }

    internal fun reachRest(): Float = if (reach >= reachLatch && reach > 0f) reachLimit else 0f

    private suspend fun animateReach(to: Float) {
        if (reducedMotion || reach == to) reach = to
        else animate(reach, to, animationSpec = settleSpec()) { value, _ -> reach = value }
    }

    private fun settleSpec() = tween<Float>(Tokens.Motion.PANEL, easing = Tokens.Motion.EASE_STANDARD.cubic())

    val connection = object : NestedScrollConnection {
        override fun onPreScroll(available: Offset, source: NestedScrollSource): Offset =
            if (available.y < 0f) Offset(0f, consume(available.y, allowReach = false)) else Offset.Zero

        override fun onPostScroll(consumed: Offset, available: Offset, source: NestedScrollSource): Offset =
            if (available.y > 0f) Offset(0f, consume(available.y, allowReach = source == NestedScrollSource.UserInput)) else Offset.Zero

        override suspend fun onPreFling(available: Velocity): Velocity {
            // Release while stretched: settle the sky without also sending the task list flying.
            if (reach <= 0f) return Velocity.Zero
            val to = if (releasingLatch || (available.y < 0f && !latched)) 0f else reachRest()
            releasingLatch = false
            latched = to > 0f
            animateReach(to)
            return Velocity(0f, available.y)
        }

        override suspend fun onPostFling(consumed: Velocity, available: Velocity): Velocity {
            if (forced == 0f && scrolled > 0f && scrolled < range) {
                val to = if (fraction < 0.5f) 0f else range
                if (reducedMotion) scrolled = to
                else animate(scrolled, to, animationSpec = settleSpec()) { value, _ -> scrolled = value }
            }
            return Velocity.Zero
        }
    }
}

/** A linear ramp between two points of direct manipulation. */
internal fun ramp(fraction: Float, from: Float, to: Float): Float =
    ((fraction - from) / (to - from)).coerceIn(0f, 1f)

/** The visual hand-off at each scroll fraction, independent of the layout's measured height. */
internal object HeaderChoreography {
    fun detail(fraction: Float) = ramp(fraction, 0f, 0.40f)
    fun title(fraction: Float) = ramp(fraction, 0.20f, 0.55f)
    fun scene(fraction: Float) = ramp(fraction, 0.30f, 0.70f)
    fun compact(fraction: Float) = ramp(fraction, 0.55f, 1f)
    fun sceneScale(fraction: Float, reduced: Boolean) = if (reduced) 1f else 1f - 0.45f * scene(fraction)
    fun reachFade(fraction: Float) = ramp(fraction, 0.15f, 1f)
    fun sunScale(fraction: Float, reduced: Boolean) = if (reduced) 1f else 1f + 0.25f * fraction

    /** The distant layers start more slowly but land on their specified endpoints together. */
    fun parallax(fraction: Float, lag: Float, reduced: Boolean) =
        if (reduced) fraction else fraction * (lag + (1f - lag) * fraction)
}
