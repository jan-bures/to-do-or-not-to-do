package app.todoornot.model

import app.todoornot.core.RoomKey
import app.todoornot.design.Easing
import app.todoornot.design.Tokens
import kotlin.math.abs
import kotlin.math.max
import kotlin.math.min
import kotlin.math.roundToLong

/*
 * The completion choreography, the sun's travel and the dusk, as plain functions of a clock (milliseconds
 * on a monotonic clock). The same model as the desktop's `apps/desktop/src/model/motion.rs`, so both
 * clients move alike: `docs/design-language.md`, Motion, moments 1 and 2.
 *
 * Completing a task:
 *
 * 1. the disc fills and pops (CHECK_FILL, POP); the tick draws, the title dims and is struck through
 *    (CHECK_TICK, STANDARD, after CHECK_TICK_DELAY);
 * 2. the row holds still (COMPLETE_HOLD);
 * 3. it fades and collapses (ROW_COLLAPSE, EXIT), and only then is it released: it leaves the open rows
 *    (entering an open Completed section, ROW_ENTER), the counts change and the sun travels (SUN_TRAVEL,
 *    SUN).
 *
 * Reopening a completed task mirrors it: the disc clears (OVERLAY_OUT, EXIT), the row collapses out of its
 * completed place and is released into the open rows, where it enters. Toggling a row again in the middle
 * reverses it from wherever it is, in place. With reduced motion, movement is instant, fades take at most
 * REDUCED_FADE, and the hold keeps its full length ([Pace]).
 *
 * Every duration, curve and cue is a `todo-design` token (the generated [Tokens]). Nothing here sleeps or
 * touches Compose: the screen samples it at each frame's time, and [Motion.wake] says when it needs the
 * next frame (every frame while something moves, one timer while a row only holds still, nothing at rest).
 */

private val M = Tokens.Motion

/** Eased progress for linear progress [t] (clamped to 0–1): the CSS cubic Bézier, as `todo_design` does it. */
fun Easing.ease(t: Float): Float {
    val x = t.coerceIn(0f, 1f).toDouble()
    if (x == 0.0 || x == 1.0) return x.toFloat()
    fun bez(a: Double, b: Double, s: Double): Double {
        val u = 1.0 - s
        return 3.0 * u * u * s * a + 3.0 * u * s * s * b + s * s * s
    }
    fun slope(a: Double, b: Double, s: Double): Double {
        val u = 1.0 - s
        return 3.0 * u * u * a + 6.0 * u * s * (b - a) + 3.0 * s * s * (1.0 - b)
    }
    val (ax, ay, bx, by) = listOf(x1.toDouble(), y1.toDouble(), x2.toDouble(), y2.toDouble())
    // Newton's method on x(s) = t, falling back to bisection where the slope is flat.
    var s = x
    var n = 0
    while (n++ < 8) {
        val err = bez(ax, bx, s) - x
        if (abs(err) < 1e-7) return bez(ay, by, s).toFloat()
        val d = slope(ax, bx, s)
        if (abs(d) < 1e-6) break
        s = (s - err / d).coerceIn(0.0, 1.0)
    }
    var lo = 0.0
    var hi = 1.0
    s = x
    repeat(40) {
        if (bez(ax, bx, s) < x) lo = s else hi = s
        s = (lo + hi) / 2.0
    }
    return bez(ay, by, s).toFloat()
}

/**
 * How fast transitions starting now run: the system's animator duration scale (0 is reduced motion,
 * `Settings.Global.ANIMATOR_DURATION_SCALE`), read whenever one starts. Intermediate scales stretch or
 * shrink the animations as they do every other animation on the phone; timers keep their length.
 */
data class Pace(val scale: Float = 1f) {
    val reduced: Boolean get() = scale <= 0f

    /** A movement (a scale, a slide, a stroke drawing itself): instant with reduced motion. */
    fun movement(ms: Int): Long = if (reduced) 0L else (ms * scale).roundToLong()

    /** A pure opacity or colour fade: at most REDUCED_FADE with reduced motion. */
    fun fade(ms: Int): Long = if (reduced) min(ms, M.REDUCED_FADE).toLong() else (ms * scale).roundToLong()

    companion object {
        val FULL = Pace(1f)
        val REDUCED = Pace(0f)
    }
}

/**
 * Linear progress, 0 to 1, of the stretch that starts [start] into the elapsed time [e] and lasts
 * [span]. An instant stretch is complete as soon as it starts.
 */
private fun stretch(e: Long, start: Long, span: Long): Float = when {
    e < start -> 0f
    span <= 0L -> 1f
    else -> min((e - start).toFloat() / span, 1f)
}

/** [a] to [b] at [t], landing exactly on [b] (an overshooting [t] goes past it). */
fun mix(a: Float, b: Float, t: Float): Float = if (t == 1f) b else a + (b - a) * t

/**
 * What a task row looks like at a moment. Every field runs from 0 to 1 except [scale], which overshoots a
 * little while the disc pops.
 */
data class RowLook(
    /** The checkbox disc's opacity: 0 is the empty ring, 1 the filled disc (with its tick). */
    val fill: Float,
    /** The disc's scale. */
    val scale: Float,
    /** How much of the tick is drawn, from the left. */
    val tick: Float,
    /** How much of the title's strike-through is drawn, from the left. */
    val strike: Float,
    /** How far the title has dimmed to `ink-3`. */
    val dim: Float,
    val opacity: Float,
    /** The row's share of its height: 1 in place, 0 collapsed (the rows below have closed the gap). */
    val height: Float,
) {
    /** Nothing of it shows: it takes no taps. */
    val gone: Boolean get() = opacity <= 0f || height <= 0f

    companion object {
        /** A row at rest, open or done. */
        fun rest(done: Boolean): RowLook {
            val d = if (done) 1f else 0f
            return RowLook(fill = d, scale = 1f, tick = d, strike = d, dim = d, opacity = 1f, height = 1f)
        }

        /** A row that isn't there (yet): no height, nothing drawn. */
        fun hidden(done: Boolean): RowLook = rest(done).copy(opacity = 0f, height = 0f)
    }
}

private sealed interface Phase {
    /** Fill, tick and hold in the open rows, then collapse; released into Completed at the end. */
    data object Complete : Phase

    /** The disc clears, then the row collapses out of its completed place; released into the open rows. */
    data object Reopen : Phase

    /** Back to rest where it is, open or done: a toggle that reverses a completion or a reopening. */
    data class Settle(val done: Boolean) : Phase

    /** Entering a list at its new place, after a release. */
    data class Enter(val done: Boolean) : Phase
}

/**
 * Where a row is drawn in a room's list: among its rows, or under its Completed section. The Completed
 * and Agent activity smart lists show done tasks among their rows, so a row's section isn't its task's
 * done state.
 */
enum class Section { ROWS, COMPLETED }

/**
 * A row's choreography: drawn in [section] (where it was toggled; for [Phase.Enter], where it entered),
 * and released into [next].
 */
private class RowMotion(
    val phase: Phase,
    val start: Long,
    val from: RowLook,
    val pace: Pace,
    val section: Section,
    val next: Section = section,
) {
    private val tickAt: Long get() = pace.movement(M.CHECK_TICK_DELAY)

    /** When the fill, the tick, the title and any restored height have all landed. */
    private fun landed(): Long {
        val title = tickAt + max(pace.movement(M.CHECK_TICK), pace.fade(M.CHECK_TICK))
        return maxOf(title, pace.movement(M.CHECK_FILL), restoreEnd())
    }

    /** How long putting a partly collapsed or faded row back takes (nothing for a row in place). */
    private fun restoreEnd(): Long {
        var end = 0L
        if (from.height < 1f) end = max(end, pace.movement(M.ROW_ENTER))
        if (from.opacity < 1f) end = max(end, pace.fade(M.ROW_ENTER))
        return end
    }

    private fun clear(): Long = pace.fade(M.OVERLAY_OUT)

    /** When the row starts to leave. */
    fun exitAt(): Long = when (phase) {
        Phase.Complete -> landed() + M.COMPLETE_HOLD
        Phase.Reopen -> max(clear(), restoreEnd())
        is Phase.Settle, is Phase.Enter -> Long.MAX_VALUE
    }

    private fun exitSpan(): Long = max(pace.movement(M.ROW_COLLAPSE), pace.fade(M.ROW_COLLAPSE))

    private fun length(): Long = when (phase) {
        Phase.Complete, Phase.Reopen -> exitAt() + exitSpan()
        is Phase.Settle -> if (phase.done) {
            landed()
        } else {
            maxOf(clear(), pace.movement(M.CHECK_TICK), pace.fade(M.CHECK_TICK), restoreEnd())
        }
        is Phase.Enter -> max(pace.movement(M.ROW_ENTER), pace.fade(M.ROW_ENTER))
    }

    fun end(): Long = start + length()

    fun look(now: Long): RowLook {
        val e = max(0L, now - start)
        val p = pace
        val f = from
        // Filling: the disc pops in from its starting scale (or from where it is), and the tick and the
        // title follow after the tick delay.
        fun filling(): RowLook {
            val t = stretch(e, 0, p.movement(M.CHECK_FILL))
            val fromScale = if (f.fill > 0f) f.scale else M.CHECK_FILL_FROM_SCALE
            val draw = M.EASE_STANDARD.ease(stretch(e, tickAt, p.movement(M.CHECK_TICK)))
            val dim = M.EASE_STANDARD.ease(stretch(e, tickAt, p.fade(M.CHECK_TICK)))
            return f.copy(
                fill = mix(f.fill, 1f, min(M.EASE_POP.ease(t), 1f)),
                scale = mix(fromScale, 1f, M.EASE_POP.ease(t)),
                tick = mix(f.tick, 1f, draw),
                strike = mix(f.strike, 1f, draw),
                dim = mix(f.dim, 1f, dim),
            )
        }
        // Clearing: the disc fades out with its tick, and the title loses its strike and brightens.
        fun clearing(): RowLook {
            val t = stretch(e, 0, clear())
            val undraw = M.EASE_STANDARD.ease(stretch(e, 0, p.movement(M.CHECK_TICK)))
            val undim = M.EASE_STANDARD.ease(stretch(e, 0, p.fade(M.CHECK_TICK)))
            return f.copy(
                fill = mix(f.fill, 0f, M.EASE_EXIT.ease(t)),
                scale = mix(f.scale, 1f, t),
                tick = if (t >= 1f) 0f else f.tick,
                strike = mix(f.strike, 0f, undraw),
                dim = mix(f.dim, 0f, undim),
            )
        }
        // A row that was partly collapsed or faded comes back first; a leaving one goes.
        fun placed(look: RowLook): RowLook {
            val backH = M.EASE_ENTER.ease(stretch(e, 0, p.movement(M.ROW_ENTER)))
            val backO = M.EASE_ENTER.ease(stretch(e, 0, p.fade(M.ROW_ENTER)))
            val at = exitAt()
            // With reduced motion the row only fades, and leaves the layout at once when it's released.
            val goneH = if (p.reduced) 0f else M.EASE_EXIT.ease(stretch(e, at, p.movement(M.ROW_COLLAPSE)))
            val goneO = M.EASE_EXIT.ease(stretch(e, at, p.fade(M.ROW_COLLAPSE)))
            return look.copy(
                height = mix(f.height, 1f, backH) * (1f - goneH),
                opacity = mix(f.opacity, 1f, backO) * (1f - goneO),
            )
        }
        return when (phase) {
            Phase.Complete -> placed(filling())
            Phase.Reopen -> placed(clearing())
            is Phase.Settle -> placed(if (phase.done) filling() else clearing())
            is Phase.Enter -> RowLook.rest(phase.done).copy(
                height = M.EASE_ENTER.ease(stretch(e, 0, p.movement(M.ROW_ENTER))),
                opacity = M.EASE_ENTER.ease(stretch(e, 0, p.fade(M.ROW_ENTER))),
            )
        }
    }

    fun wake(now: Long): Wake {
        if (phase == Phase.Complete) {
            // Holding still: nothing to draw until the row starts to leave.
            val quiet = start + landed()
            val exit = start + exitAt()
            if (now in quiet until exit) return Wake.At(exit)
        }
        return Wake.Frame
    }
}

/** When the screen needs its next frame. */
sealed interface Wake {
    /** At rest: no frames, no timers. */
    data object Idle : Wake

    /** Nothing moves until then (a row holding still). */
    data class At(val time: Long) : Wake

    /** Something moves: the next frame. */
    data object Frame : Wake

    fun or(other: Wake): Wake = when {
        this == Frame || other == Frame -> Frame
        this == Idle -> other
        other == Idle -> this
        else -> At(min((this as At).time, (other as At).time))
    }
}

/** A row that finished leaving: what the list holds changes now. */
sealed interface Release {
    val id: String

    /** A completed task left the open rows (and enters Completed, if that's open). */
    data class Completed(override val id: String) : Release

    /** A reopened task left its completed place (and enters the open rows). */
    data class Reopened(override val id: String) : Release
}

/** Where a toggled task stays while its choreography plays. */
enum class Keep {
    /** In the open rows, as if still open ([Motion.holding]). */
    HOLDING,

    /** In its completed place, as if still done ([Motion.reopening]). */
    REOPENING,

    /** Where it already belongs: the toggle reversed a choreography that hadn't released it. */
    IN_PLACE,
}

/** The sun's place on its arc, gliding from where it was to where the count puts it. */
private data class Travel(val room: RoomKey, val from: Float, val to: Float, val start: Long, val span: Long) {
    fun at(now: Long): Float = mix(from, to, M.EASE_SUN.ease(stretch(now, start, span)))

    fun moving(now: Long): Boolean = from != to && now < start + span
}

/**
 * Where the header is between the day's strip and the finished day's dusk scene (moment 2). One linear
 * phase drives every part, read off at the doc's cues: 0 is the strip, 1 the dusk scene.
 */
data class DuskLook(
    val phase: Float,
    /** Where it's heading: the finished scene or the strip. */
    val finished: Boolean,
    val reduced: Boolean = false,
) {
    /** How far the dusk scene has faded in over the strip (SUN). */
    val scene: Float get() = M.EASE_SUN.ease(phase)

    /** How far the header has grown towards the dusk scene's height. With reduced motion it switches at once. */
    val height: Float get() = if (reduced) (if (finished) 1f else 0f) else M.EASE_SUN.ease(phase)

    /** Whether the header text is in the dark theme's ink, over the night sky. */
    val nightInk: Boolean get() = phase >= M.DUSK_NIGHT_AT

    /** How far the moon and stars have faded in. */
    val night: Float get() = night(0)

    /**
     * How far the stars of batch [b] have faded in: up to STAGGER_MAX_ITEMS batches, STAGGER apart, all
     * landing together at the end of the dusk (no stagger with reduced motion).
     */
    fun night(b: Int): Float {
        val span = (1f - M.DUSK_NIGHT_AT) * M.DUSK
        val delay = if (reduced) 0f else stagger(b)
        val ms = (phase - M.DUSK_NIGHT_AT) * M.DUSK - delay
        return M.EASE_SUN.ease(ms / (span - delay))
    }

    /**
     * How far item [i] of tomorrow's hand-over has entered (ROW_ENTER, ENTER, STAGGER apart for the first
     * STAGGER_MAX_ITEMS), and how far it still has to rise, in dp.
     */
    fun handover(i: Int): Pair<Float, Float> {
        if (reduced) return phase to 0f
        val ms = (phase - M.DUSK_HANDOVER_AT) * M.DUSK - stagger(i)
        val t = M.EASE_ENTER.ease((ms / M.ROW_ENTER).coerceIn(0f, 1f))
        return t to (1f - t) * M.DUSK_HANDOVER_RISE
    }

    /** Whether item [i] of the hand-over has begun to enter: only then does it take taps. */
    fun handoverShown(i: Int): Boolean = handover(i).first > 0f

    /** Whether it's at rest. */
    val settled: Boolean get() = phase == (if (finished) 1f else 0f)

    companion object {
        /** At rest: the strip, or the dusk scene. */
        fun rest(finished: Boolean) = DuskLook(if (finished) 1f else 0f, finished)

        private fun stagger(i: Int): Float = (min(i, M.STAGGER_MAX_ITEMS.toInt()) * M.STAGGER).toFloat()
    }
}

/** The dusk on its way: its phase runs linearly from [from] to the finished scene or the strip. */
private data class Dusking(val room: RoomKey, val from: Float, val to: Boolean, val start: Long, val span: Long, val reduced: Boolean) {
    fun at(now: Long) = DuskLook(mix(from, if (to) 1f else 0f, stretch(now, start, span)), to, reduced)
}

/** The screen's motion: rows in their choreography, the sun and the dusk. Not thread-safe: one owner. */
class Motion {
    private val rows = LinkedHashMap<String, RowMotion>()
    private var sun: Travel? = null
    private var dusk: Dusking? = null

    /** How the row of task [id] looks now, or null when it's at rest. */
    fun row(id: String, now: Long): RowLook? = rows[id]?.look(now)

    /**
     * The section of the list the row of task [id] is drawn in while it moves (where it was toggled, or
     * where it entered after its release), or null when it's at rest (wherever its task is).
     */
    fun placedIn(id: String): Section? = rows[id]?.section

    /**
     * Whether task [id] is done, as its choreography assumes (completing: done; reopening: open), or null
     * when it's at rest. Used for a task whose latest state hasn't been read yet.
     */
    fun expectedDone(id: String): Boolean? = when (val p = rows[id]?.phase) {
        null -> null
        Phase.Complete -> true
        Phase.Reopen -> false
        is Phase.Settle -> p.done
        is Phase.Enter -> p.done
    }

    private fun start(id: String, phase: Phase, now: Long, pace: Pace, rest: Boolean, section: Section, next: Section = section) {
        val from = row(id, now) ?: RowLook.rest(rest)
        rows[id] = RowMotion(phase, now, from, pace, section, next)
    }

    /**
     * Task [id] was just completed, its row in section [from]; once released it enters [to]. A task that
     * was being reopened settles back where it is; any other starts the completion where it is.
     */
    fun complete(id: String, now: Long, pace: Pace, from: Section = Section.ROWS, to: Section = Section.COMPLETED): Keep {
        val r = rows[id]
        return if (r?.phase == Phase.Reopen) {
            start(id, Phase.Settle(done = true), now, pace, rest = true, section = r.section)
            Keep.IN_PLACE
        } else {
            start(id, Phase.Complete, now, pace, rest = false, section = from, next = to)
            Keep.HOLDING
        }
    }

    /**
     * Task [id] was just reopened, its row in section [from]; once released it enters [to]. A task that
     * was being completed settles back where it is; any other starts leaving its completed place.
     */
    fun reopen(id: String, now: Long, pace: Pace, from: Section = Section.COMPLETED, to: Section = Section.ROWS): Keep {
        val r = rows[id]
        return if (r?.phase == Phase.Complete) {
            start(id, Phase.Settle(done = false), now, pace, rest = false, section = r.section)
            Keep.IN_PLACE
        } else {
            start(id, Phase.Reopen, now, pace, rest = true, section = from, next = to)
            Keep.REOPENING
        }
    }

    /** Tasks completing in the open rows: they count as open until they're released. */
    fun holding(): Set<String> = ids(Phase.Complete)

    /** Tasks leaving their completed place: they count as done until they're released. */
    fun reopening(): Set<String> = ids(Phase.Reopen)

    private fun ids(phase: Phase): Set<String> = rows.filterValues { it.phase == phase }.keys.toSet()

    /** The tasks in motion, for following the data ([reconcile]). */
    fun ids(): Set<String> = rows.keys.toSet()

    /**
     * Follows the data when it changed under a choreography (another device, an agent, a refused
     * command): [done] says whether each task is done now, or null when it's gone. A completion whose task
     * is open again, or a reopening whose task is done again, settles back in place; a row at rest or
     * entering that no longer matches its task's state stops at once.
     */
    fun reconcile(now: Long, pace: Pace, done: (String) -> Boolean?) {
        val it = rows.entries.iterator()
        while (it.hasNext()) {
            val entry = it.next()
            val r = entry.value
            val d = done(entry.key)
            when {
                d == null -> it.remove()
                (d == false && r.phase == Phase.Complete) || (d == true && r.phase == Phase.Reopen) ->
                    entry.setValue(RowMotion(Phase.Settle(done = r.phase == Phase.Reopen), now, r.look(now), pace, r.section))
                r.phase is Phase.Settle && r.phase.done != d -> it.remove()
                r.phase is Phase.Enter && r.phase.done != d -> it.remove()
            }
        }
    }

    /**
     * Moves every row past its end: rows that finished leaving are released (and enter their new place,
     * timed from when they left), finished ones come to rest.
     */
    fun advance(now: Long): List<Release> {
        val released = ArrayList<Release>()
        val it = rows.entries.iterator()
        while (it.hasNext()) {
            val entry = it.next()
            val r = entry.value
            val end = r.end()
            if (now < end) continue
            val id = entry.key
            val done = when (r.phase) {
                Phase.Complete -> true
                Phase.Reopen -> false
                is Phase.Settle, is Phase.Enter -> {
                    it.remove()
                    continue
                }
            }
            released += if (done) Release.Completed(id) else Release.Reopened(id)
            val entering = RowMotion(Phase.Enter(done), end, RowLook.hidden(done), r.pace, r.next)
            if (now < entering.end()) entry.setValue(entering) else it.remove()
        }
        return released
    }

    /**
     * Stops the motion of every row the list doesn't show ([shown] is false): deleted, filtered out, in a
     * closed Completed section, or never in this room, so nothing keeps waking the screen for it. A task
     * reopened from where it isn't shown (the detail pane, with Completed closed) goes straight to
     * entering the open rows. Returns whether a task held open or kept done was let go, so the screen
     * reads again.
     */
    fun retainShown(now: Long, shown: (String) -> Boolean): Boolean {
        var letGo = false
        val it = rows.entries.iterator()
        while (it.hasNext()) {
            val entry = it.next()
            if (shown(entry.key)) continue
            val r = entry.value
            when (r.phase) {
                Phase.Complete -> {
                    letGo = true
                    it.remove()
                }
                Phase.Reopen -> {
                    letGo = true
                    entry.setValue(RowMotion(Phase.Enter(false), now, RowLook.hidden(false), r.pace, r.next))
                }
                is Phase.Settle, is Phase.Enter -> it.remove()
            }
        }
        return letGo
    }

    /** Whether any row is in its choreography. */
    val rowsMoving: Boolean get() = rows.isNotEmpty()

    /** Everything at rest at once, as on switching rooms: completions and reopenings are let go. */
    fun settleAll() = rows.clear()

    /**
     * Points the sun at [target] (its place on the arc, or null when the room has no sun). In the same
     * room it glides there from wherever it is now; a new room, or a sun that appears or goes, snaps.
     */
    fun aimSun(room: RoomKey, target: Float?, now: Long, pace: Pace) {
        if (target == null) {
            sun = null
            return
        }
        val s = sun
        when {
            s != null && s.room == room && s.to == target -> Unit
            s != null && s.room == room -> sun = Travel(room, s.at(now), target, now, pace.movement(M.SUN_TRAVEL))
            else -> placeSun(room, target, now)
        }
    }

    /**
     * Puts the sun at [to] at once, without travelling (as [aimSun] does in a new room). The finished day
     * doesn't use it: its sun travels to the end of the arc while the strip fades under the dusk scene.
     */
    fun placeSun(room: RoomKey, to: Float, now: Long) {
        sun = Travel(room, to, to, now, 0L)
    }

    /** The sun's place on its arc now (0 to 1) in [room], or null without one there. */
    fun sun(room: RoomKey, now: Long): Float? = sun?.takeIf { it.room == room }?.at(now)

    /**
     * Turns the header towards the dusk scene (the day just finished) or back to the strip (a task
     * reopened or added): over DUSK there and the brisker ROOM_HUE back, from wherever it is. With reduced
     * motion it crossfades in REDUCED_FADE. A new room starts at rest.
     */
    fun aimDusk(room: RoomKey, finished: Boolean, now: Long, pace: Pace) {
        val d = dusk
        dusk = when {
            d != null && d.room == room && d.to == finished -> return
            d != null && d.room == room ->
                Dusking(room, d.at(now).phase, finished, now, pace.fade(if (finished) M.DUSK else M.ROOM_HUE), pace.reduced)
            else -> Dusking(room, if (finished) 1f else 0f, finished, now, 0L, pace.reduced)
        }
    }

    /** Where the header of [room] is between the strip and the dusk scene now, or null if it's not known. */
    fun dusk(room: RoomKey, now: Long): DuskLook? = dusk?.takeIf { it.room == room }?.at(now)

    /** When the screen needs its next frame. At rest it's [Wake.Idle]: no frames and no timers. */
    fun wake(now: Long): Wake {
        val dusking = dusk?.let { !it.at(now).settled } ?: false
        val moving = dusking || sun?.moving(now) == true
        return rows.values.fold(if (moving) Wake.Frame else Wake.Idle) { w, r -> w.or(r.wake(now)) }
    }

    /** Whether a row has reached its end by [now], so [advance] has something to do. */
    fun due(now: Long): Boolean = rows.values.any { now >= it.end() }
}
