package app.todoornot

import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.input.nestedscroll.NestedScrollSource
import app.todoornot.ui.HeaderCollapse
import app.todoornot.ui.HeaderCollapse.Part
import org.junit.Assert.assertEquals
import org.junit.Test

class HeaderCollapseTest {
    private fun header(body: Int = 200, pills: Int = 50) = HeaderCollapse().apply {
        this.body = body
        this.pills = pills
    }

    private val drag = NestedScrollSource.UserInput

    @Test
    fun scrolling_up_collapses_the_header_before_the_list_moves() {
        val h = header()
        val used = h.connection.onPreScroll(Offset(0f, -120f), drag)
        assertEquals(-120f, used.y, 0f)
        assertEquals(120, h.hiddenOf(Part.Body))
        assertEquals(0, h.hiddenOf(Part.Pills))

        // The rest of a long drag: the body goes, then the pills, then the list gets what's left.
        val more = h.connection.onPreScroll(Offset(0f, -200f), drag)
        assertEquals(-130f, more.y, 0f)
        assertEquals(200, h.hiddenOf(Part.Body))
        assertEquals(50, h.hiddenOf(Part.Pills))
        assertEquals(1f, h.fraction, 0f)
        assertEquals(Offset.Zero, h.connection.onPreScroll(Offset(0f, -40f), drag))
    }

    @Test
    fun scrolling_down_expands_only_once_the_list_is_back_at_its_top() {
        val h = header()
        h.connection.onPreScroll(Offset(0f, -250f), drag)
        // Downward scroll goes to the list first: pre-scroll leaves it alone.
        assertEquals(Offset.Zero, h.connection.onPreScroll(Offset(0f, 80f), drag))
        assertEquals(1f, h.fraction, 0f)
        // What the list couldn't use (it's at the top) opens the header, pills first.
        val used = h.connection.onPostScroll(Offset.Zero, Offset(0f, 30f), drag)
        assertEquals(30f, used.y, 0f)
        assertEquals(20, h.hiddenOf(Part.Pills))
        assertEquals(200, h.hiddenOf(Part.Body))
        h.connection.onPostScroll(Offset.Zero, Offset(0f, 1000f), drag)
        assertEquals(0f, h.fraction, 0f)
    }

    @Test
    fun the_keyboard_or_a_short_window_forces_it_closed_whatever_the_scroll() {
        val h = header()
        h.forced = 1f
        assertEquals(1f, h.fraction, 0f)
        assertEquals(200, h.hiddenOf(Part.Body))
        assertEquals(50, h.hiddenOf(Part.Pills))
        // Halfway through the animation, scrolling can still hide more but not less.
        h.forced = 0.5f
        assertEquals(125f, h.hidden, 0f)
        h.connection.onPreScroll(Offset(0f, -200f), drag)
        assertEquals(200f, h.hidden, 0f)
        h.forced = 0f
        assertEquals(200f, h.hidden, 0f)
    }

    @Test
    fun a_smaller_header_never_hides_more_than_it_has() {
        val h = header()
        h.connection.onPreScroll(Offset(0f, -250f), drag)
        h.body = 100
        h.pills = 40
        assertEquals(1f, h.fraction, 0f)
        assertEquals(100, h.hiddenOf(Part.Body))
        assertEquals(40, h.hiddenOf(Part.Pills))
    }
}
