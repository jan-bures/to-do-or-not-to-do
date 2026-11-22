package app.todoornot

import androidx.compose.animation.core.Animatable
import androidx.compose.animation.core.LinearEasing
import androidx.compose.animation.core.tween
import androidx.compose.runtime.MonotonicFrameClock
import androidx.compose.ui.MotionDurationScale
import app.todoornot.core.RoomKey
import app.todoornot.core.SmartList
import app.todoornot.design.Tokens
import app.todoornot.model.MotionHost
import app.todoornot.model.Pace
import app.todoornot.model.countdown
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.launch
import kotlinx.coroutines.test.TestScope
import kotlinx.coroutines.test.UnconfinedTestDispatcher
import kotlinx.coroutines.test.advanceTimeBy
import kotlinx.coroutines.test.runTest
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

private val M = Tokens.Motion
private val MY_DAY: RoomKey = RoomKey.Smart(SmartList.MY_DAY)

/**
 * A frame clock the test drives: it remembers that a frame was asked for, and [draw] runs it at a frame
 * time the test chooses, apart from the clock the motion starts transitions on.
 */
private class Frames : MonotonicFrameClock {
    private var waiting: CompletableDeferred<Long>? = null

    /** Whether a frame is asked for. */
    val asked: Boolean get() = waiting != null

    override suspend fun <R> withFrameNanos(onFrame: (Long) -> R): R {
        val d = CompletableDeferred<Long>()
        waiting = d
        return onFrame(d.await() * 1_000_000L)
    }

    /** Draws the frame asked for, at [ms]. */
    fun draw(ms: Long) {
        val d = checkNotNull(waiting) { "no frame was asked for" }
        waiting = null
        d.complete(ms)
    }
}

/** The system's animation scale at 0 ("Remove animations"), as Compose sees it. */
private object NoAnimations : MotionDurationScale {
    override val scaleFactor: Float get() = 0f
}

/** The frame driver ([MotionHost.drive]) with a fake clock and frame times that don't agree, as on a device. */
@OptIn(ExperimentalCoroutinesApi::class)
class MotionHostTest {
    private var clock = 0L

    private fun TestScope.driven(host: MotionHost): Frames {
        val frames = Frames()
        backgroundScope.launch(UnconfinedTestDispatcher(testScheduler) + frames) { host.drive() }
        return frames
    }

    @Test
    fun a_frame_sampled_just_before_dusk_ends_is_followed_by_the_last_one() = runTest {
        val host = MotionHost { clock }
        synchronized(host) {
            host.motion.aimDusk(MY_DAY, false, 0, Pace.FULL)
            host.motion.aimDusk(MY_DAY, true, 0, Pace.FULL)
        }
        host.touched()
        val frames = driven(host)
        assertTrue(frames.asked)
        // A frame samples just before the end, and drawing it runs past the end.
        clock = M.DUSK + 30L
        frames.draw(M.DUSK - 10L)
        assertFalse(host.dusk(MY_DAY)!!.settled)
        assertTrue("the header is still part grown: one more frame", frames.asked)
        clock = M.DUSK + 40L
        frames.draw(M.DUSK + 20L)
        assertTrue(host.dusk(MY_DAY)!!.settled)
        assertFalse("at rest: no frames", frames.asked)
    }

    @Test
    fun a_hold_starts_only_once_its_tick_is_drawn_and_its_timer_asks_for_a_frame() = runTest {
        val host = MotionHost { clock }
        synchronized(host) { host.motion.complete("a", 0, Pace.FULL) }
        host.touched()
        val frames = driven(host)
        val landed = (M.CHECK_TICK_DELAY + M.CHECK_TICK).toLong()
        val exit = landed + M.COMPLETE_HOLD
        // A frame samples just before the tick lands, and drawing it runs into the hold.
        clock = landed + 30
        frames.draw(landed - 10)
        assertTrue(host.row("a")!!.tick < 1f)
        assertTrue("the tick isn't drawn yet: one more frame", frames.asked)
        clock = landed + 40
        frames.draw(landed + 20)
        assertEquals(1f, host.row("a")!!.tick)
        assertFalse("holding still: a timer, no frames", frames.asked)
        // The hold's timer runs out: one frame, though the last one sampled the hold.
        clock = exit
        advanceTimeBy(exit - landed)
        assertTrue(frames.asked)
        clock = exit + 20
        frames.draw(exit + 10)
        assertTrue("leaving: every frame", frames.asked)
        // Released, entering Completed, then at rest.
        clock = RELEASE_AT_HOST + M.ROW_ENTER + 20
        frames.draw(RELEASE_AT_HOST + M.ROW_ENTER + 10)
        assertEquals(null, host.row("a"))
        assertFalse(frames.asked)
    }

    @Test
    fun the_undo_countdown_keeps_its_length_with_animations_removed() = runTest {
        // An animation (what the toast used to count down with) is over at its first frame at scale 0.
        val animFrames = Frames()
        var animated = false
        backgroundScope.launch(UnconfinedTestDispatcher(testScheduler) + animFrames + NoAnimations) {
            Animatable(1f).animateTo(0f, tween(M.UNDO_WINDOW, easing = LinearEasing))
            animated = true
        }
        animFrames.draw(1_000)
        if (animFrames.asked) animFrames.draw(1_016)
        assertTrue(animated)

        // The countdown runs on frame times: the whole window, the bar following it.
        val frames = Frames()
        var left = 1f
        var over = false
        backgroundScope.launch(UnconfinedTestDispatcher(testScheduler) + frames + NoAnimations) {
            countdown(M.UNDO_WINDOW.toLong()) { left = it }
            over = true
        }
        frames.draw(1_000)
        frames.draw(1_016)
        assertFalse(over)
        frames.draw(1_000L + M.UNDO_WINDOW / 2)
        assertEquals(0.5f, left, 1e-3f)
        frames.draw(1_000L + M.UNDO_WINDOW - 1)
        assertFalse(over)
        frames.draw(1_000L + M.UNDO_WINDOW)
        assertTrue(over)
        assertEquals(0f, left)
    }
}

/** Where a full-pace completion is released. */
private val RELEASE_AT_HOST = (M.CHECK_TICK_DELAY + M.CHECK_TICK + M.COMPLETE_HOLD + M.ROW_COLLAPSE).toLong()
