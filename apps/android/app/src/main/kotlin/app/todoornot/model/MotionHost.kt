package app.todoornot.model

import androidx.compose.runtime.Stable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableLongStateOf
import androidx.compose.runtime.setValue
import androidx.compose.runtime.withFrameMillis
import app.todoornot.core.RoomKey
import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.withTimeoutOrNull
import kotlin.math.max

/**
 * What the screen samples: the [Motion] at the current frame's time. Every read is a snapshot-state read,
 * so a composable, layout or draw that calls these follows the frames, and only while something moves.
 */
@Stable
interface MotionFrame {
    /** How task [id]'s row looks now, or null at rest. */
    fun row(id: String): RowLook?

    /** The section task [id]'s row is drawn in while it moves (see [Motion.placedIn]), or null at rest. */
    fun placedIn(id: String): Section?

    /** Whether task [id] is done as its choreography has it (see [Motion.expectedDone]), or null at rest. */
    fun checked(id: String): Boolean?

    /** The sun's place on its arc in [room], or null when the screen should use the room's count. */
    fun sun(room: RoomKey): Float?

    /** Where [room]'s header is between the strip and the dusk scene, or null when it's at rest. */
    fun dusk(room: RoomKey): DuskLook?

    /** The rows the room no longer lists, painted while their choreography plays out ([Motion.ghost]). */
    fun ghosts(): List<Ghost> = emptyList()

    /** Runs the frames this motion needs while the screen is shown: none at rest. */
    suspend fun drive() = Unit

    /** Nothing moves: the screen draws its data at rest. */
    object Rest : MotionFrame {
        override fun row(id: String): RowLook? = null
        override fun placedIn(id: String): Section? = null
        override fun checked(id: String): Boolean? = null
        override fun sun(room: RoomKey): Float? = null
        override fun dusk(room: RoomKey): DuskLook? = null
    }
}

/**
 * How task [id]'s row looks in [section] of the list, or null at rest. A row in the section it's leaving or
 * hasn't reached yet (the screen's data is a moment behind the motion) takes no room and shows nothing.
 */
fun MotionFrame.rowIn(id: String, section: Section): RowLook? {
    val placed = placedIn(id) ?: return null
    return if (placed != section) RowLook.hidden(checked(id) ?: (section == Section.COMPLETED)) else row(id)
}

/**
 * The [Motion] of one screen, with the time its frames are drawn at. [clock] is a monotonic clock in
 * milliseconds on the same base as the frame clock (`System.nanoTime`). [onRelease] runs when rows finish
 * leaving, so the model reads the screen again. Everything here locks on this host; whoever changes
 * [motion] holds that lock too.
 */
class MotionHost(private val clock: () -> Long) : MotionFrame {
    /** The model itself: only under this host's lock. */
    val motion = Motion()

    var onRelease: (List<Release>) -> Unit = {}

    /** The time the frame being drawn samples: never goes back. */
    private var time by mutableLongStateOf(clock())

    /** Bumped on every change that isn't a new frame (a toggle, a new count, a room switch). */
    private var version by mutableIntStateOf(0)

    private val kick = Channel<Unit>(Channel.CONFLATED)

    private var frozen = false

    /** Now, for starting a transition: the clock, but never before the frame on screen. */
    fun now(): Long = synchronized(this) { max(time, clock()) }

    /** Something changed outside a frame: samplers read again, and the driver wakes. */
    fun touched() {
        synchronized(this) {
            time = now()
            version++
        }
        kick.trySend(Unit)
    }

    /** One frame at time [t]: rows that finished leaving are released, then everything samples [t]. */
    fun frame(t: Long) {
        val released = synchronized(this) {
            val at = max(time, t)
            val released = if (motion.due(at)) motion.advance(at) else emptyList()
            time = at
            // What the list holds may have changed (a ghost gone): whatever reads it reads again.
            if (released.isNotEmpty()) version++
            released
        }
        if (released.isNotEmpty()) onRelease(released)
    }

    /** Stops the clock at [t] (the screenshot gallery's frozen frames), after releasing what's due by then. */
    fun freeze(t: Long) {
        frozen = true
        frame(t)
        synchronized(this) { version++ }
    }

    /** Samples the motion at the frame's time, as a snapshot-state read so the caller follows the frames. */
    private inline fun <T> sample(f: (Long) -> T): T = synchronized(this) {
        version
        f(time)
    }

    override fun row(id: String): RowLook? = sample { motion.row(id, it) }

    override fun placedIn(id: String): Section? = sample { motion.placedIn(id) }

    override fun checked(id: String): Boolean? = sample { motion.expectedDone(id) }

    override fun sun(room: RoomKey): Float? = sample { motion.sun(room, it) }

    override fun dusk(room: RoomKey): DuskLook? = sample { motion.dusk(room, it) }

    /** Follows changes, not frames: a ghost's look is sampled by its row. */
    override fun ghosts(): List<Ghost> = synchronized(this) {
        version
        motion.ghosts()
    }

    /**
     * Asks for exactly the frames the motion needs: every frame while something moves, a single timer
     * while rows only hold still, and nothing at rest (it waits for [touched]).
     *
     * It decides from the time the screen last sampled, not the clock: a frame drawn just before the
     * dusk ends (or the tick lands) is followed by one more, even if drawing it ran past the end, so the
     * last thing on screen is the end. When a hold's timer runs out it asks for a frame, which samples
     * past the hold.
     */
    override suspend fun drive() {
        if (frozen) return
        while (true) {
            when (val w = synchronized(this) { motion.wake(time) }) {
                Wake.Idle -> kick.receive()
                is Wake.At -> {
                    val left = w.time - clock()
                    // A change while holding (a toggle, a new count) decides again; else the timer ran out.
                    val changed = left > 0L && withTimeoutOrNull(left) { kick.receive() } != null
                    if (!changed) withFrameMillis { frame(it) }
                }
                Wake.Frame -> withFrameMillis { frame(it) }
            }
        }
    }
}

/**
 * Counts down a functional timer of [ms] (the undo window) on the frame clock, reporting what's left (1 to
 * 0) on each frame, and returns once it has run out. Frame times are real time, so unlike an animation it
 * keeps its length whatever the system's animation scale, even 0 (`docs/design-language.md`: timers keep
 * their full duration).
 */
suspend fun countdown(ms: Long, onLeft: (Float) -> Unit) {
    val start = withFrameMillis { it }
    while (true) {
        val elapsed = withFrameMillis { it } - start
        onLeft(if (ms <= 0L) 0f else (1f - elapsed.toFloat() / ms).coerceIn(0f, 1f))
        if (elapsed >= ms) return
    }
}
