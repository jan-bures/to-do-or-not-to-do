package app.todoornot

import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.input.nestedscroll.NestedScrollSource
import androidx.compose.ui.unit.dp
import app.todoornot.model.UiState
import app.todoornot.model.headerClockChanged
import app.todoornot.model.isoDay
import app.todoornot.ui.HeaderChoreography
import app.todoornot.ui.HeaderCollapse
import androidx.compose.ui.unit.Velocity
import kotlinx.coroutines.runBlocking
import org.junit.Assert.assertTrue
import app.todoornot.ui.PaneFold
import app.todoornot.ui.listPaneWidth
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
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

    @Test fun the_stretched_height_is_split_without_losing_any_finger_distance() {
        val h = header()
        h.consume(72f)
        assertEquals(0.4f, h.reachFraction, 0.001f)
        assertEquals(18f, h.reachToText, 0f)
        assertEquals(54f, h.reachToScene, 0f)
        h.consume(200f)
        assertEquals(45f, h.reachToText, 0f)
        assertEquals(135f, h.reachToScene, 0f)
        h.reachLimit = 112f
        assertEquals(1f, h.reachFraction, 0f)
        assertEquals(112f, h.reachToText + h.reachToScene, 0f)
    }

    @Test fun reduced_motion_keeps_drag_geometry_and_snaps_the_latch() = runBlocking {
        val h = header().apply { reducedMotion = true }
        assertEquals(108f, h.consume(108f), 0f)
        assertEquals(81f, h.reachToScene, 0f)
        h.connection.onPreFling(Velocity.Zero)
        assertEquals(180f, h.visibleReach, 0f)
        assertEquals(1f, HeaderChoreography.sunScale(h.reachFraction, true), 0f)
        assertEquals(0.4f, HeaderChoreography.parallax(0.4f, 0.35f, true), 0f)
        assertEquals(1f, HeaderChoreography.sceneScale(0.7f, true), 0f)
        h.consume(-16f)
        assertEquals(164f, h.reach, 0f)
        // Even a small upward drag from the latch dismisses reach on release.
        h.connection.onPreFling(Velocity.Zero)
        assertEquals(0f, h.reach, 0f)
        h.consume(-96f)
        h.connection.onPostFling(Velocity.Zero, Velocity.Zero)
        assertEquals(1f, h.fraction, 0f)
    }

    @Test fun collapse_curves_handoff_without_jumps_and_parallax_lands_together() {
        val visual = HeaderChoreography
        assertEquals(0.5f, visual.detail(0.20f), 0.001f)
        assertEquals(0f, visual.title(0.20f), 0f)
        assertEquals(1f, visual.title(0.55f), 0f)
        assertEquals(0f, visual.scene(0.30f), 0f)
        assertEquals(0.775f, visual.sceneScale(0.50f, false), 0.001f)
        assertEquals(0.55f, visual.sceneScale(0.70f, false), 0.001f)
        assertEquals(0f, visual.compact(0.55f), 0f)
        assertEquals(0.5f, visual.compact(0.775f), 0.001f)
        assertEquals(1f, visual.compact(1f), 0f)
        assertEquals(0f, visual.reachFade(0.15f), 0f)
        assertEquals(1f, visual.reachFade(1f), 0f)
        assertTrue(visual.parallax(0.5f, 0.35f, false) < visual.parallax(0.5f, 0.65f, false))
        assertEquals(1f, visual.parallax(1f, 0.35f, false), 0f)
    }

    @Test fun opening_a_sheet_disables_reach_without_jumping_the_visible_height() = runBlocking {
        val h = header().apply { reducedMotion = true }
        h.consume(180f)
        h.reachEnabled = false
        assertEquals(180f, h.visibleReach, 0f)
        h.resetReach()
        assertEquals(0f, h.consume(100f), 0f)
    }

    @Test fun the_sky_clock_refreshes_at_an_hour_or_when_the_timed_cue_expires() {
        val e = fixtureEngine("groceries")
        val state = UiState(e.home(FIXTURE_NOW), e.room(e.list("Groceries"), FIXTURE_NOW), null, e.status(), emptyList(), null,
            today = isoDay(FIXTURE_NOW))
        val inTwentyMinutes = FIXTURE_NOW.copy(epochMs = FIXTURE_NOW.epochMs + 20 * 60_000L)
        assertFalse(state.headerClockChanged(inTwentyMinutes))
        assertTrue(state.headerClockChanged(inTwentyMinutes.copy(epochMs = inTwentyMinutes.epochMs + 5 * 60_000L)))
        val due = state.copy(room = state.room.copy(nextDueToday = "10:55"))
        assertTrue(due.headerClockChanged(inTwentyMinutes))
    }

    @Test fun pane_and_sheet_share_the_left_edge_of_the_hinge() {
        assertNull(listPaneWidth(412.dp, false, null))
        assertEquals(437.5.dp, listPaneWidth(876.dp, true, null))
        assertEquals(421.dp, listPaneWidth(908.dp, true, PaneFold(421.dp, 443.dp)))
        assertEquals(450.dp, listPaneWidth(908.dp, true, PaneFold(908.dp, 910.dp)))
    }
}
