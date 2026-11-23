package app.todoornot

import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.input.nestedscroll.NestedScrollSource
import androidx.compose.ui.unit.dp
import app.todoornot.ui.HeaderCollapse
import app.todoornot.ui.PaneFold
import app.todoornot.ui.listPaneWidth
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

class HeaderCollapseTest {
    private fun header(cover: Boolean = true) = HeaderCollapse().apply {
        body = 200
        compact = 36
        reachLimit = if (cover) 180f else 0f
        reachLatch = 96f
    }

    private val finger = NestedScrollSource.UserInput

    @Test fun the_sky_collapses_one_to_one_before_the_list_moves() {
        val h = header()
        assertEquals(-120f, h.connection.onPreScroll(Offset(0f, -120f), finger).y, 0f)
        assertEquals(120f, h.hidden, 0f)
        assertEquals(-44f, h.connection.onPreScroll(Offset(0f, -100f), finger).y, 0f)
        assertEquals(36f, h.body - h.hidden, 0f)
        assertEquals(1f, h.fraction, 0f)
    }

    @Test fun reach_starts_only_after_the_list_and_header_are_back_at_the_top() {
        val h = header()
        h.consume(-164f)
        assertEquals(Offset.Zero, h.connection.onPreScroll(Offset(0f, 100f), finger))
        assertEquals(0f, h.reach, 0f)
        h.connection.onPostScroll(Offset.Zero, Offset(0f, 200f), finger)
        assertEquals(0f, h.fraction, 0f)
        assertEquals(36f, h.reach, 0f)
        assertEquals(0f, h.reachRest(), 0f)
        h.connection.onPostScroll(Offset.Zero, Offset(0f, 60f), finger)
        assertEquals(180f, h.reachRest(), 0f)
        h.consume(500f)
        assertEquals(180f, h.reach, 0f)
        h.consume(-210f)
        assertEquals(0f, h.reach, 0f)
        assertEquals(30f, h.hidden, 0f)
    }

    @Test fun no_reach_on_inner_keyboard_or_fling() {
        val h = header(false)
        assertEquals(0f, h.consume(1000f), 0f)
        h.reachLimit = 180f
        assertEquals(Offset.Zero, h.connection.onPostScroll(Offset.Zero, Offset(0f, 200f), NestedScrollSource.SideEffect))
        h.forced = 1f
        assertEquals(0f, h.consume(200f), 0f)
        assertEquals(1f, h.fraction, 0f)
        h.compact = 0
        assertEquals(200f, h.hidden, 0f)
    }

    @Test fun pane_and_sheet_share_the_left_edge_of_the_hinge() {
        assertNull(listPaneWidth(412.dp, false, null))
        assertEquals(437.5.dp, listPaneWidth(876.dp, true, null))
        assertEquals(421.dp, listPaneWidth(908.dp, true, PaneFold(421.dp, 443.dp)))
        assertEquals(450.dp, listPaneWidth(908.dp, true, PaneFold(908.dp, 910.dp)))
    }
}
