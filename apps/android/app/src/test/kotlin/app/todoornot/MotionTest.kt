package app.todoornot

import app.todoornot.core.RoomKey
import app.todoornot.core.SmartList
import app.todoornot.design.Tokens
import app.todoornot.model.DuskLook
import app.todoornot.model.Keep
import app.todoornot.model.Motion
import app.todoornot.model.Pace
import app.todoornot.model.Release
import app.todoornot.model.RowLook
import app.todoornot.model.Wake
import app.todoornot.model.ease
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import kotlin.math.abs
import kotlin.random.Random

private val M = Tokens.Motion

/** Where a full-pace completion starts to leave, and where it's released. */
private val EXIT_AT = (M.CHECK_TICK_DELAY + M.CHECK_TICK + M.COMPLETE_HOLD).toLong()
private val RELEASE_AT = EXIT_AT + M.ROW_COLLAPSE

private val MY_DAY: RoomKey = RoomKey.Smart(SmartList.MY_DAY)
private val ALL: RoomKey = RoomKey.Smart(SmartList.ALL)

private fun close(a: Float, b: Float) = abs(a - b) < 1e-4f

private fun Motion.look(id: String, t: Long): RowLook = row(id, t) ?: error("$id is at rest at $t")

/**
 * The completion choreography, the sun and the dusk as pure functions of a clock: the same model, and the
 * same cases, as the desktop's `model/motion/tests.rs`.
 */
class MotionTest {
    @Test
    fun the_easings_match_todo_design() {
        for (e in listOf(M.EASE_STANDARD, M.EASE_ENTER, M.EASE_EXIT, M.EASE_POP, M.EASE_SUN)) {
            assertEquals(0f, e.ease(0f))
            assertEquals(1f, e.ease(1f))
        }
        assertTrue(close(M.EASE_SUN.ease(0.5f), 0.5f))
        assertTrue("POP overshoots", (0 until 100).any { M.EASE_POP.ease(it / 100f) > 1f })
    }

    @Test
    fun a_completion_fills_draws_holds_then_collapses_and_is_released() {
        val mo = Motion()
        assertEquals(Keep.HOLDING, mo.complete("a", 0, Pace.FULL))
        assertEquals(setOf("a"), mo.holding())
        assertEquals(false, mo.placedDone("a"))

        // The disc starts small and empty, and pops past its size before it settles.
        val start = mo.look("a", 0)
        assertEquals(0f, start.fill)
        assertEquals(M.CHECK_FILL_FROM_SCALE, start.scale)
        assertTrue((1 until M.CHECK_FILL).any { mo.look("a", it.toLong()).scale > 1f })
        val filled = mo.look("a", M.CHECK_FILL.toLong())
        assertEquals(1f to 1f, filled.fill to filled.scale)
        // The tick waits for its delay, then draws with the strike-through and the dimming title.
        assertEquals(0f, mo.look("a", M.CHECK_TICK_DELAY.toLong()).tick)
        val mid = mo.look("a", (M.CHECK_TICK_DELAY + M.CHECK_TICK / 2).toLong())
        assertTrue(mid.tick > 0f && mid.tick < 1f && mid.strike == mid.tick && mid.dim == mid.tick)

        // Holding: done, struck through and still in place, and nothing to draw until it leaves.
        val landed = (M.CHECK_TICK_DELAY + M.CHECK_TICK).toLong()
        for (t in listOf(landed, landed + M.COMPLETE_HOLD / 2, EXIT_AT - 1)) {
            assertEquals("$t", RowLook.rest(true), mo.look("a", t))
            assertEquals("$t", Wake.At(EXIT_AT), mo.wake(t))
        }
        assertTrue(mo.advance(EXIT_AT).isEmpty())

        // Leaving: fading and collapsing, still held.
        assertEquals(Wake.Frame, mo.wake(EXIT_AT))
        val leaving = mo.look("a", EXIT_AT + M.ROW_COLLAPSE / 2)
        assertTrue(leaving.height > 0f && leaving.height < 1f)
        assertTrue(leaving.opacity > 0f && leaving.opacity < 1f)
        assertTrue(mo.advance(RELEASE_AT - 1).isEmpty())

        // Released once it has gone; then it enters Completed (if that shows it) and comes to rest.
        assertEquals(listOf(Release.Completed("a")), mo.advance(RELEASE_AT))
        assertTrue(mo.holding().isEmpty())
        assertEquals(true, mo.placedDone("a"))
        assertEquals(0f, mo.look("a", RELEASE_AT).height)
        assertTrue(mo.advance(RELEASE_AT + M.ROW_ENTER).isEmpty())
        assertNull(mo.row("a", RELEASE_AT + M.ROW_ENTER))
        assertEquals(Wake.Idle, mo.wake(RELEASE_AT + M.ROW_ENTER))
    }

    @Test
    fun reduced_motion_snaps_movement_caps_fades_and_keeps_the_hold() {
        val mo = Motion()
        mo.complete("a", 0, Pace.REDUCED)
        // The checkbox switches at once: no pop, no drawing.
        val first = mo.look("a", 0)
        assertEquals(listOf(1f, 1f, 1f, 1f), listOf(first.fill, first.scale, first.tick, first.strike))
        // The title dims in REDUCED_FADE.
        val half = mo.look("a", (M.REDUCED_FADE / 2).toLong())
        assertTrue(half.dim > 0f && half.dim < 1f)
        // The hold keeps its full length, then the row only fades: it never collapses.
        val exit = (M.REDUCED_FADE + M.COMPLETE_HOLD).toLong()
        assertEquals(Wake.At(exit), mo.wake(M.REDUCED_FADE.toLong()))
        val fading = mo.look("a", exit + M.REDUCED_FADE / 2)
        assertEquals(1f, fading.height)
        assertTrue(fading.opacity > 0f && fading.opacity < 1f)
        val release = exit + M.REDUCED_FADE
        assertTrue(mo.advance(release - 1).isEmpty())
        assertEquals(listOf(Release.Completed("a")), mo.advance(release))
        // It enters Completed without growing.
        assertEquals(1f, mo.look("a", release).height)
    }

    @Test
    fun the_systems_animation_scale_stretches_animations_but_not_the_hold() {
        val mo = Motion()
        val pace = Pace(2f)
        mo.complete("a", 0, pace)
        val exit = 2L * (M.CHECK_TICK_DELAY + M.CHECK_TICK) + M.COMPLETE_HOLD
        assertEquals(Wake.At(exit), mo.wake(2L * (M.CHECK_TICK_DELAY + M.CHECK_TICK)))
        assertEquals(listOf(Release.Completed("a")), mo.advance(exit + 2L * M.ROW_COLLAPSE))
    }

    @Test
    fun toggling_back_during_the_hold_restores_the_row_in_place_without_a_jump() {
        val mo = Motion()
        mo.complete("a", 0, Pace.FULL)
        val at = 400L
        val before = mo.look("a", at)
        assertEquals(Keep.IN_PLACE, mo.reopen("a", at, Pace.FULL))
        assertEquals(before, mo.look("a", at))
        assertTrue(mo.holding().isEmpty() && mo.reopening().isEmpty())
        assertEquals("it stays in the open rows", false, mo.placedDone("a"))
        // The disc clears, the strike draws back, and it's never released.
        val mid = mo.look("a", at + M.OVERLAY_OUT / 2)
        assertTrue(mid.fill < 1f && mid.fill > 0f && mid.strike < 1f)
        assertTrue(mo.advance(5_000).isEmpty())
        assertNull(mo.row("a", 5_000))
        assertEquals(Wake.Idle, mo.wake(5_000))
    }

    @Test
    fun toggling_back_while_collapsing_grows_the_row_back() {
        val mo = Motion()
        mo.complete("a", 0, Pace.FULL)
        val at = EXIT_AT + M.ROW_COLLAPSE / 2
        val before = mo.look("a", at)
        assertTrue(before.height < 1f)
        mo.reopen("a", at, Pace.FULL)
        assertEquals(before, mo.look("a", at))
        val back = mo.look("a", at + M.ROW_ENTER)
        assertEquals(1f to 1f, back.height to back.opacity)
        assertTrue(mo.advance(5_000).isEmpty())
    }

    @Test
    fun reopening_clears_collapses_out_of_completed_and_enters_the_open_rows() {
        val mo = Motion()
        assertEquals(Keep.REOPENING, mo.reopen("a", 0, Pace.FULL))
        assertEquals(setOf("a"), mo.reopening())
        assertEquals(true, mo.placedDone("a"))
        assertEquals(RowLook.rest(true), mo.look("a", 0))
        // The disc clears first; the row stays full height until then.
        val cleared = mo.look("a", M.OVERLAY_OUT.toLong())
        assertEquals(listOf(0f, 0f, 1f), listOf(cleared.fill, cleared.tick, cleared.height))
        val release = (M.OVERLAY_OUT + M.ROW_COLLAPSE).toLong()
        assertTrue(mo.advance(release - 1).isEmpty())
        assertEquals(listOf(Release.Reopened("a")), mo.advance(release))
        assertEquals(false, mo.placedDone("a"))
        val entering = mo.look("a", release + M.ROW_ENTER / 2)
        assertEquals(0f to 0f, entering.fill to entering.strike)
        assertTrue(entering.height > 0f && entering.height < 1f)
    }

    @Test
    fun completing_again_while_reopening_settles_back_done() {
        val mo = Motion()
        mo.reopen("a", 0, Pace.FULL)
        assertEquals(Keep.IN_PLACE, mo.complete("a", 60, Pace.FULL))
        assertTrue(mo.reopening().isEmpty())
        assertEquals(true, mo.placedDone("a"))
        assertTrue(mo.advance(5_000).isEmpty())
        assertNull(mo.row("a", 5_000))
    }

    @Test
    fun a_change_from_elsewhere_settles_or_stops_the_row() {
        val mo = Motion()
        mo.complete("a", 0, Pace.FULL)
        mo.reopen("b", 0, Pace.FULL)
        mo.complete("c", 0, Pace.FULL)
        // A was reopened on another device (or its completion refused), B completed again; C was deleted.
        val state = mapOf("a" to false, "b" to true)
        mo.reconcile(300, Pace.FULL) { state[it] }
        assertTrue(mo.holding().isEmpty() && mo.reopening().isEmpty())
        assertNull(mo.row("c", 300))
        // Settling starts from where each row was: no jump.
        assertNotNull(mo.row("a", 300))
        assertTrue(mo.advance(5_000).isEmpty())
        assertEquals(Wake.Idle, mo.wake(5_000))
    }

    @Test
    fun switching_rooms_puts_everything_at_rest() {
        val mo = Motion()
        mo.complete("a", 0, Pace.FULL)
        mo.reopen("b", 0, Pace.FULL)
        mo.settleAll()
        assertTrue(mo.holding().isEmpty() && mo.reopening().isEmpty())
        assertEquals(Wake.Idle, mo.wake(0))
    }

    @Test
    fun a_row_the_list_no_longer_shows_stops_at_once() {
        val mo = Motion()
        mo.complete("entering", 0, Pace.FULL)
        mo.complete("held", 100, Pace.FULL)
        mo.reopen("reopened", RELEASE_AT - 100, Pace.FULL)
        assertEquals(listOf(Release.Completed("entering")), mo.advance(RELEASE_AT))
        val now = RELEASE_AT
        // The deleted held task stops and is let go, a task reopened where it isn't shown goes straight
        // to entering the open rows, and the released row with no place in a closed Completed stops.
        assertTrue(mo.retainShown(now) { false })
        assertNull(mo.row("held", now))
        assertNull(mo.row("entering", now))
        assertTrue(mo.holding().isEmpty() && mo.reopening().isEmpty())
        val enters = mo.look("reopened", now)
        assertEquals(0f to 0f, enters.height to enters.fill)
        // Not shown there either: it stops, and that lets nothing go.
        assertFalse(mo.retainShown(now) { false })
        assertEquals(Wake.Idle, mo.wake(now))
    }

    @Test
    fun the_sun_glides_along_the_sun_curve_and_retargets_from_where_it_is() {
        val mo = Motion()
        mo.aimSun(MY_DAY, 0.25f, 0, Pace.FULL)
        // The first aim places it.
        assertEquals(0.25f, mo.sun(MY_DAY, 0))
        assertEquals(Wake.Idle, mo.wake(0))
        mo.aimSun(MY_DAY, 0.5f, 1_000, Pace.FULL)
        assertEquals(0.25f, mo.sun(MY_DAY, 1_000))
        assertEquals(Wake.Frame, mo.wake(1_000))
        // The symmetric SUN curve is halfway at half time, and slow at both ends.
        val half = mo.sun(MY_DAY, 1_000L + M.SUN_TRAVEL / 2)!!
        assertTrue("$half", close(half, 0.375f))
        val early = mo.sun(MY_DAY, 1_000L + M.SUN_TRAVEL / 10)!!
        assertTrue("$early", early - 0.25f < 0.025f * 0.5f)
        // A quick second change retargets from the current place, without a jump.
        val at = 1_000L + M.SUN_TRAVEL / 2
        mo.aimSun(MY_DAY, 0.75f, at, Pace.FULL)
        assertTrue(close(mo.sun(MY_DAY, at)!!, half))
        assertEquals(0.75f, mo.sun(MY_DAY, at + M.SUN_TRAVEL))
        assertEquals(Wake.Idle, mo.wake(at + M.SUN_TRAVEL))
        // Aiming at the same place again changes nothing; another room's header doesn't read this sun.
        mo.aimSun(MY_DAY, 0.75f, 9_000, Pace.FULL)
        assertEquals(Wake.Idle, mo.wake(9_000))
        assertNull(mo.sun(ALL, 9_000))
    }

    @Test
    fun the_sun_snaps_in_a_new_room_with_reduced_motion_and_when_it_appears() {
        val mo = Motion()
        mo.aimSun(MY_DAY, 0.25f, 0, Pace.FULL)
        mo.aimSun(ALL, 0.8f, 10, Pace.FULL)
        assertEquals(0.8f, mo.sun(ALL, 10))
        mo.aimSun(MY_DAY, 0.25f, 20, Pace.FULL)
        mo.aimSun(MY_DAY, 0.5f, 30, Pace.REDUCED)
        assertEquals(0.5f, mo.sun(MY_DAY, 30))
        assertEquals(Wake.Idle, mo.wake(30))
        mo.aimSun(MY_DAY, null, 40, Pace.FULL)
        assertNull(mo.sun(MY_DAY, 40))
        mo.aimSun(MY_DAY, 0f, 50, Pace.FULL)
        assertEquals(0f, mo.sun(MY_DAY, 50))
    }

    @Test
    fun a_placed_sun_glides_back_from_where_it_was_put() {
        val mo = Motion()
        mo.aimSun(MY_DAY, 11f / 12f, 0, Pace.FULL)
        mo.placeSun(MY_DAY, 1f, 100)
        assertEquals(1f, mo.sun(MY_DAY, 100))
        assertEquals(Wake.Idle, mo.wake(100))
        mo.aimSun(MY_DAY, 11f / 12f, 200, Pace.FULL)
        assertEquals(1f, mo.sun(MY_DAY, 200))
        assertEquals(Wake.Frame, mo.wake(200))
    }

    @Test
    fun dusk_falls_with_its_cues_and_lifts_briskly() {
        val mo = Motion()
        mo.aimDusk(MY_DAY, false, 0, Pace.FULL)
        assertEquals(DuskLook.rest(false), mo.dusk(MY_DAY, 0))
        assertEquals(Wake.Idle, mo.wake(0))
        // The last task has left: dusk falls over DUSK.
        mo.aimDusk(MY_DAY, true, 0, Pace.FULL)
        assertEquals(Wake.Frame, mo.wake(0))
        fun at(share: Float) = mo.dusk(MY_DAY, (M.DUSK * share).toLong())!!
        // The ink switches and the night begins at its cue; the hand-over enters after its own, staggered.
        assertFalse(at(M.DUSK_NIGHT_AT - 0.05f).nightInk)
        assertTrue(at(M.DUSK_NIGHT_AT + 0.01f).nightInk)
        assertEquals(0f, at(M.DUSK_NIGHT_AT - 0.05f).night)
        assertEquals(0f to M.DUSK_HANDOVER_RISE, at(M.DUSK_HANDOVER_AT - 0.01f).handover(0))
        assertFalse(at(M.DUSK_HANDOVER_AT - 0.01f).handoverShown(0))
        val (first, rise) = at(M.DUSK_HANDOVER_AT + 0.1f).handover(0)
        val (second, _) = at(M.DUSK_HANDOVER_AT + 0.1f).handover(1)
        assertTrue(first > second && second > 0f && rise > 0f && rise < M.DUSK_HANDOVER_RISE)
        // The stars come out in batches, the later ones a little behind.
        val stars = at(0.7f)
        assertTrue(stars.night(0) > stars.night(4) && stars.night(4) > 0f)
        // The SUN curve: half way at half time.
        val half = at(0.5f)
        assertTrue(close(half.scene, 0.5f) && close(half.height, 0.5f))
        val done = mo.dusk(MY_DAY, M.DUSK.toLong())!!
        assertTrue(done.settled && done.scene == 1f && done.night == 1f && done.night(4) == 1f)
        assertEquals(1f to 0f, done.handover(M.STAGGER_MAX_ITEMS.toInt() + 3))
        assertEquals(Wake.Idle, mo.wake(M.DUSK.toLong()))
        // A task reopens: it turns back from there, over the brisker ROOM_HUE.
        val back = M.DUSK + 1_000L
        mo.aimDusk(MY_DAY, false, back, Pace.FULL)
        assertEquals(1f, mo.dusk(MY_DAY, back)!!.scene)
        assertTrue(mo.dusk(MY_DAY, back + M.ROOM_HUE)!!.settled)
        assertEquals(0f, mo.dusk(MY_DAY, back + M.ROOM_HUE)!!.scene)
    }

    @Test
    fun turning_back_part_way_retargets_from_where_the_dusk_is() {
        val mo = Motion()
        mo.aimDusk(MY_DAY, false, 0, Pace.FULL)
        mo.aimDusk(MY_DAY, true, 0, Pace.FULL)
        val at = M.DUSK / 3L
        val before = mo.dusk(MY_DAY, at)!!.scene
        mo.aimDusk(MY_DAY, false, at, Pace.FULL)
        assertTrue(close(mo.dusk(MY_DAY, at)!!.scene, before))
        assertTrue(mo.dusk(MY_DAY, at + M.ROOM_HUE)!!.settled)
    }

    @Test
    fun reduced_dusk_crossfades_without_growing_or_rising() {
        val mo = Motion()
        mo.aimDusk(MY_DAY, false, 0, Pace.REDUCED)
        mo.aimDusk(MY_DAY, true, 0, Pace.REDUCED)
        val mid = mo.dusk(MY_DAY, M.REDUCED_FADE / 2L)!!
        assertEquals(1f, mid.height)
        assertTrue(mid.scene > 0f && mid.scene < 1f)
        assertEquals(0f, mid.handover(3).second)
        assertTrue(mo.dusk(MY_DAY, M.REDUCED_FADE.toLong())!!.settled)
        // Another room starts at rest.
        mo.aimDusk(ALL, false, 10, Pace.FULL)
        assertEquals(DuskLook.rest(false), mo.dusk(ALL, 10))
        assertNull(mo.dusk(MY_DAY, 10))
    }

    private fun inRange(look: RowLook): Boolean {
        fun unit(v: Float) = v in 0f..1f
        return unit(look.fill) && unit(look.tick) && unit(look.strike) && unit(look.dim) &&
            unit(look.opacity) && unit(look.height) && look.scale >= M.CHECK_FILL_FROM_SCALE && look.scale < 1.2f
    }

    /** What the eye sees of a look: the scale and the tick don't show while the disc is empty. */
    private fun seen(look: RowLook): RowLook = if (look.fill == 0f) look.copy(scale = 1f, tick = 0f) else look

    /**
     * Any sequence of toggles, changes from elsewhere, deletions and room switches, at any speed and pace:
     * every row stays in range and toggles never jump; a task is held or reopening only while its state
     * says it was just completed or reopened; releases always agree with the state; and once things go
     * quiet, everything converges to rest with no frames or timers left.
     */
    @Test
    fun the_choreography_converges_to_rest_and_stays_consistent() {
        val ids = listOf("a", "b", "c")
        repeat(400) { case ->
            val rnd = Random(case)
            val pace = listOf(Pace.FULL, Pace.REDUCED, Pace(0.5f))[case % 3]
            val mo = Motion()
            var now = 0L
            val done = ids.associateWith { false }.toMutableMap()
            val placed = done.toMutableMap()
            val deleted = HashSet<String>()
            fun check(where: String) {
                val holding = mo.holding()
                val reopening = mo.reopening()
                for (id in ids) {
                    mo.row(id, now)?.let { assertTrue("$case $where: $it", inRange(it)) }
                    when {
                        id in deleted -> {
                            assertNull("$case $where", mo.row(id, now))
                            assertTrue(id !in holding && id !in reopening)
                        }
                        id in holding -> assertTrue("$case $where", done.getValue(id) && !placed.getValue(id))
                        id in reopening -> assertTrue("$case $where", !done.getValue(id) && placed.getValue(id))
                        else -> assertEquals("$case $where $id", done.getValue(id), placed.getValue(id))
                    }
                }
            }
            repeat(1 + rnd.nextInt(40)) { step ->
                now += if (rnd.nextBoolean()) rnd.nextLong(60) else rnd.nextLong(1_200)
                for (r in mo.advance(now)) {
                    when (r) {
                        is Release.Completed -> assertTrue(done[r.id] == true && placed[r.id] == false)
                        is Release.Reopened -> assertTrue(done[r.id] == false && placed[r.id] == true)
                    }
                    placed[r.id] = r is Release.Completed
                }
                check("after advance $step")
                val id = ids[rnd.nextInt(ids.size)]
                when (rnd.nextInt(10)) {
                    in 0..5 -> if (id !in deleted) {
                        val d = done.getValue(id)
                        val before = mo.row(id, now) ?: RowLook.rest(placed.getValue(id))
                        done[id] = !d
                        when (if (d) mo.reopen(id, now, pace) else mo.complete(id, now, pace)) {
                            Keep.HOLDING -> assertFalse(placed.getValue(id))
                            Keep.REOPENING -> assertTrue(placed.getValue(id))
                            Keep.IN_PLACE -> assertEquals(!d, placed.getValue(id))
                        }
                        val after = mo.row(id, now) ?: error("a toggle starts motion")
                        if (!pace.reduced) assertEquals("$case: a toggle jumped", seen(before), seen(after))
                    }
                    in 6..7 -> if (id !in deleted) done[id] = !done.getValue(id)
                    8 -> deleted += id
                    else -> mo.settleAll()
                }
                // As the model does: the state answers (a deleted task is gone), and the list doesn't show
                // a deleted task.
                mo.reconcile(now, pace) { if (it in deleted) null else done[it] }
                while (mo.retainShown(now) { it !in deleted }) {
                    // Letting a task go reads again; the list still doesn't show it.
                }
                val holding = mo.holding()
                val reopening = mo.reopening()
                for (t in ids) if (t !in holding && t !in reopening) placed[t] = done.getValue(t)
                check("after step $step")
            }
            // Quiet: everything is released and at rest, with no frames and no timers.
            now += 10_000
            for (r in mo.advance(now)) placed[r.id] = r is Release.Completed
            assertEquals("$case", Wake.Idle, mo.wake(now))
            assertTrue(mo.holding().isEmpty() && mo.reopening().isEmpty())
            for (t in ids) {
                if (t !in deleted) assertEquals("$case $t", done.getValue(t), placed.getValue(t))
                assertNull(mo.row(t, now))
            }
        }
    }

    /** However often the count changes, the sun stays on the arc and comes to rest where the last change put it. */
    @Test
    fun the_sun_converges_to_its_last_target() {
        repeat(200) { case ->
            val rnd = Random(case)
            val mo = Motion()
            var now = 0L
            var last = 0f
            repeat(1 + rnd.nextInt(20)) {
                now += rnd.nextLong(900)
                last = rnd.nextInt(13) / 12f
                mo.aimSun(MY_DAY, last, now, Pace.FULL)
                assertTrue(mo.sun(MY_DAY, now)!! in 0f..1f)
            }
            now += M.SUN_TRAVEL
            assertEquals(last, mo.sun(MY_DAY, now))
            assertEquals(Wake.Idle, mo.wake(now))
        }
    }
}
