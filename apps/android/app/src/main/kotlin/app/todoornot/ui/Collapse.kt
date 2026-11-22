package app.todoornot.ui

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
import kotlin.math.roundToInt

/**
 * How far the room header has collapsed. Two stacked parts give way, top first: the header body (eyebrow,
 * title, landscape), then the room pills. Scrolling the list up hides them before the list moves;
 * scrolling down shows them again once the list is back at its top, like a large top app bar with
 * `exitUntilCollapsed`. [forced] (0 to 1, animated by the caller) collapses the header regardless of
 * scrolling: while the keyboard is open, or in a short window.
 */
@Stable
class HeaderCollapse {
    /** Measured heights of the two parts, in px. */
    var body by mutableIntStateOf(0)
        internal set
    var pills by mutableIntStateOf(0)
        internal set

    /** Pixels hidden by scrolling, 0 to [range]. */
    var scrolled by mutableFloatStateOf(0f)
        private set

    var forced by mutableFloatStateOf(0f)

    val range: Float get() = (body + pills).toFloat()

    /** Pixels hidden now. */
    val hidden: Float get() = maxOf(scrolled.coerceAtMost(range), forced * range)

    /** 0 when the header is fully shown, 1 when only the bar is left. */
    val fraction: Float get() = if (range <= 0f) forced else (hidden / range).coerceIn(0f, 1f)

    /** Pixels of [part] hidden now. */
    fun hiddenOf(part: Part): Int {
        val h = hidden.roundToInt()
        return when (part) {
            Part.Body -> h.coerceIn(0, body)
            Part.Pills -> (h - body).coerceIn(0, pills)
        }
    }

    /** Moves the header by [dy] px of scroll (negative is up) and returns how much of it was used. */
    fun consume(dy: Float): Float {
        val before = scrolled
        scrolled = (scrolled - dy).coerceIn(0f, range)
        return before - scrolled
    }

    val connection: NestedScrollConnection = object : NestedScrollConnection {
        // Scrolling up: collapse first, then let the list scroll.
        override fun onPreScroll(available: Offset, source: NestedScrollSource): Offset =
            if (available.y < 0f) Offset(0f, consume(available.y)) else Offset.Zero

        // Scrolling down: the list goes back to its top first, then the header expands.
        override fun onPostScroll(consumed: Offset, available: Offset, source: NestedScrollSource): Offset =
            if (available.y > 0f) Offset(0f, consume(available.y)) else Offset.Zero
    }

    enum class Part { Body, Pills }
}

/** Lays out [part] at its natural height minus what [state] hides of it, sliding it up under what's above. */
fun Modifier.collapsing(state: HeaderCollapse, part: HeaderCollapse.Part): Modifier =
    clipToBounds().layout { measurable, constraints ->
        val p = measurable.measure(constraints.copy(minHeight = 0, maxHeight = Constraints.Infinity))
        when (part) {
            HeaderCollapse.Part.Body -> if (state.body != p.height) state.body = p.height
            HeaderCollapse.Part.Pills -> if (state.pills != p.height) state.pills = p.height
        }
        val hide = state.hiddenOf(part)
        layout(p.width, p.height - hide) { p.place(0, -hide) }
    }
