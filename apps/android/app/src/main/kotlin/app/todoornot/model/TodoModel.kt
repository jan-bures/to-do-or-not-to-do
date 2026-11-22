package app.todoornot.model

import app.todoornot.core.ChangeListener
import app.todoornot.core.FailedChange
import app.todoornot.core.FfiException
import app.todoornot.core.Home
import app.todoornot.core.LocalNow
import app.todoornot.core.ProblemKind
import app.todoornot.core.Reopening
import app.todoornot.core.RoomKey
import app.todoornot.core.RoomView
import app.todoornot.core.RowPlacement
import app.todoornot.core.SmartList
import app.todoornot.core.SyncState
import app.todoornot.core.SyncStatus
import app.todoornot.core.TaskDetail
import app.todoornot.core.TodoEngine
import app.todoornot.core.localNow
import kotlinx.coroutines.CoroutineDispatcher
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.channels.BufferOverflow
import kotlinx.coroutines.flow.MutableSharedFlow
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.launch
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import kotlinx.coroutines.withContext
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.atomic.AtomicLong

/** A short message above the composer, with an optional undo. */
data class Toast(
    val serial: Long,
    val text: String,
    /** The task to reopen on undo, for a completion. */
    val reopen: String? = null,
)

/** Everything the main screen shows, read from the engine in one go. */
data class UiState(
    val home: Home,
    val room: RoomView,
    val detail: TaskDetail?,
    val status: SyncStatus,
    val failed: List<FailedChange>,
    val toast: Toast?,
    /** Set while the unpair confirmation is showing. */
    val unpairing: UnpairQuestion? = null,
    /** The filter field is open (the search button was tapped, or a filter is typed). */
    val searching: Boolean = false,
    /** The filter typed into the field: query language or words; empty when not filtering. */
    val filter: String = "",
    /** Collections folded in the drawer, by ID. */
    val collapsed: Set<String> = emptySet(),
    /** When the engine was last seen online and caught up, for "Synced 10:35". */
    val syncedAt: LocalNow? = null,
    /** The Completed section is open. */
    val showCompleted: Boolean = false,
    /**
     * The room generation the rows were read in: every way into a room starts a new one. A row tap passes
     * it back, so a tap on a row of the room before ([Actions.setDone]) only writes.
     */
    val generation: Long = 0L,
) {
    val selected: String? get() = detail?.id
    val signedOut: Boolean get() = status.problem == ProblemKind.UNAUTHORIZED
}

/**
 * The unpair confirmation: how many changes on this phone haven't reached the server (queued or refused).
 * [changed] is set when the owner confirmed a number that was out of date: they are asked again.
 */
data class UnpairQuestion(val unsent: Int, val changed: Boolean = false)

/** How the drawer's sync line reads: fine, busy, or a problem (its dot's colour). */
enum class Tone { OK, BUSY, PROBLEM, IDLE }

/**
 * The drawer footer's sync line, worded as on the desktop: "Synced 10:35", "Syncing…", "Offline, 3
 * waiting", "2 couldn't sync", "Signed out by the server". Paused (the app isn't syncing, as in the
 * screenshots) says how much is waiting, if anything.
 */
fun syncLine(status: SyncStatus, syncedAt: LocalNow?): Pair<String, Tone> = when {
    status.problem == ProblemKind.UNAUTHORIZED -> "Signed out by the server" to Tone.PROBLEM
    status.failed > 0u -> "${status.failed} couldn’t sync" to Tone.PROBLEM
    status.state == SyncState.OFFLINE && status.pending > 0u -> "Offline, ${status.pending} waiting" to Tone.PROBLEM
    status.state == SyncState.OFFLINE -> "Offline" to Tone.PROBLEM
    status.state == SyncState.SYNCING -> "Syncing…" to Tone.BUSY
    status.state == SyncState.ONLINE -> (syncedAt?.let { "Synced ${clock(it)}" } ?: "Synced") to Tone.OK
    status.pending > 0u -> "Not syncing, ${status.pending} waiting" to Tone.IDLE
    else -> "Not syncing" to Tone.IDLE
}

/** "09:05", in the time's own UTC offset. */
fun clock(t: LocalNow): String {
    val minutes = Math.floorDiv(t.epochMs + t.utcOffsetSecs * 1000L, 60_000L)
    val day = Math.floorMod(minutes, 24L * 60L)
    return "%02d:%02d".format(day / 60, day % 60)
}

/** The sun's place on its arc for a room's count, or null when the room has no sun (nothing in it). */
fun sunAt(done: UInt, total: UInt): Float? =
    if (total == 0u) null else done.coerceAtMost(total).toFloat() / total.toFloat()

/**
 * Whether [room] lists its done tasks among its rows (the Completed and Agent activity smart lists, as the
 * engine derives them) rather than under a Completed section.
 */
fun listsDoneAmongRows(room: RoomKey): Boolean =
    room is RoomKey.Smart && (room.list == SmartList.COMPLETED || room.list == SmartList.AGENT_ACTIVITY)

/** Unsent changes, from one status snapshot (the engine reads both counts under one lock). */
fun unsentCount(status: SyncStatus): Int = (status.pending + status.failed).toInt()

/**
 * The main screen's model over the Rust engine. The engine keeps the state, the cache and the outbox;
 * this class only remembers what is open, reads the views, and turns taps into commands. Every engine
 * call runs on `io`, and the state is re-read whenever the engine says something changed.
 *
 * It also owns the screen's [motion] (`docs/design-language.md`, Motion): a toggled task is written at
 * once, but its row stays where it was, and counts as it did, until its choreography releases it. The
 * reads place it there ([RowPlacement]); only the release changes the rows, the counts, the sun and the
 * finished day. A row the room drops while it plays for any other reason (in Agent activity, the owner's
 * toggle makes the owner its last editor) plays on as a ghost ([Motion.ghost]). [clock] is a monotonic
 * clock in milliseconds; [pace] reads the system's animator duration scale when a transition starts. [listen] registers what to call when the engine says something
 * changed: by default its listener, which calls back on the engine's own threads.
 */
class TodoModel(
    val engine: TodoEngine,
    private val scope: CoroutineScope,
    private val io: CoroutineDispatcher,
    private val now: () -> LocalNow = { localNow() },
    start: RoomKey = RoomKey.Smart(SmartList.MY_DAY),
    private val onUnpair: () -> Unit = {},
    private val clock: () -> Long = { System.nanoTime() / 1_000_000L },
    private val pace: () -> Pace = { Pace.FULL },
    listen: (onChange: () -> Unit) -> Unit = { onChange ->
        engine.setListener(object : ChangeListener {
            override fun onChange() = onChange()
        })
    },
) : Actions {
    @Volatile private var key: RoomKey = start

    @Volatile private var selected: String? = null

    @Volatile private var toast: Toast? = null

    @Volatile private var serial = 0L

    @Volatile private var askingUnpair = false

    @Volatile private var unpairChanged = false

    @Volatile private var filter = ""

    @Volatile private var searching = false

    /** Refreshes in the order they started; one that finishes after a newer one is dropped. */
    private val refreshes = AtomicLong()

    private var published = 0L

    /**
     * Bumped (under [publishing]) whenever what a read depends on changes: the room, the filter text,
     * whether the filter field is open, or whether Completed is open. A read taken before the bump is
     * stale and is never published.
     */
    private var inputs = 0L

    /** The room generation ([UiState.generation]): bumped (under [motion] and [publishing]) by [enter]. */
    @Volatile private var generation = 0L

    private val publishing = Any()

    @Volatile private var collapsed: Set<String> = emptySet()

    /**
     * "+ Add step" drafts by task: here, not in the UI, so a pending one survives a recreation. The only
     * cleanup is [pruneDrafts]; the map is bounded by the open tasks viewed in this process.
     */
    private val stepDrafts = ConcurrentHashMap<String, StepDraft>()

    @Volatile private var syncedAt: LocalNow? = null

    @Volatile private var showCompleted = false

    /**
     * The screen's motion. Its lock also guards the bookkeeping below that follows it, so a read published
     * from any thread (the engine calls back on its own) is followed whole, in order.
     */
    val motion = MotionHost(clock)

    /** Where the reads keep the rows in their choreography, and how often that has changed. */
    private class Placed(val version: Long, val rows: RowPlacement)

    @Volatile private var placement = Placed(0L, RowPlacement(emptyList(), emptyList()))

    /**
     * The tasks in motion or with a toggle still to be written, whose state each read reports so the
     * motion can follow the data, each with its latest toggle's serial (0 for none) as the read saw it.
     */
    @Volatile private var moving: Map<String, Long> = emptyMap()

    /**
     * When each task being reopened had been completed (exactly, as the engine gave it), so it keeps its
     * place among the done tasks.
     */
    private val reopenedAt = HashMap<String, String>()

    /**
     * Where each task in motion was last listed (its row, section and place), so a row the room drops
     * while it plays is painted as it last was ([Motion.ghost]).
     */
    private val lastSeen = HashMap<String, Ghost>()

    /** A toggle: its place in the order they were made, and the state it asks for. */
    private class Toggle(val serial: Long, val done: Boolean)

    /**
     * Toggles made, and the latest one of each task in motion or still to be written. A toggle still to
     * be written places its row however long the write takes, past the end of its choreography: it goes
     * when the write lands or is refused, when its task is gone, or when another room opens.
     */
    private var toggles = 0L
    private val toggled = HashMap<String, Toggle>()

    /** The latest toggle the engine has applied: a read that starts after it sees it. */
    @Volatile private var written = 0L

    /** What the published screen was read with: the latest toggle applied, and the placement. */
    private var shown = Truth(emptyMap(), 0L, 0L)

    /**
     * Local writes run one at a time, in the order they were made (the mutex is first come, first served).
     * Confirming an unpair takes its count behind them, so an edit already on its way is counted.
     */
    private val writes = Mutex()

    private val _ui = MutableStateFlow(read())
    val ui: StateFlow<UiState> = _ui.asStateFlow()

    private val changes = MutableSharedFlow<Unit>(extraBufferCapacity = 1, onBufferOverflow = BufferOverflow.DROP_OLDEST)

    init {
        motion.onRelease = {
            synchronized(motion) { placeRows() }
            scope.launch { refresh() }
        }
        synchronized(motion) { followNow() }
        listen { changes.tryEmit(Unit) }
        scope.launch { changes.collect { refresh() } }
    }

    /**
     * Whether each task in motion is done (null when gone), as of the toggles up to [written], read with
     * the placement [placed], while each task's latest toggle was the one in [toggles].
     */
    private class Truth(
        val done: Map<String, Boolean?>,
        val written: Long,
        val placed: Long,
        val toggles: Map<String, Long> = emptyMap(),
    )

    /** A read: the [inputs] it was read for, the screen, and the state of the tasks in motion. */
    private class Snapshot(val epoch: Long, val state: UiState, val truth: Truth)

    /** The [inputs] a read belongs to, taken together, with the room [generation]. */
    private data class Inputs(
        val epoch: Long,
        val filter: String,
        val searching: Boolean,
        val showCompleted: Boolean,
        val generation: Long,
    )

    /** Reads the whole screen state synchronously (the screenshot tests use this directly). */
    fun read(): UiState = snapshot().state

    /** Reads the screen, with the [inputs] it was read for. */
    private fun snapshot(): Snapshot {
        // The inputs first, together: the state read belongs to them, whatever changes while reading.
        val (epoch, f, open, showDone, gen) = synchronized(publishing) {
            Inputs(inputs, filter, searching, showCompleted, generation)
        }
        // Toggles applied before the read are in it; the placement and the tasks in motion go with it.
        val seen = written
        val place = placement
        val ids = moving
        val k = key
        val n = now()
        pruneDrafts(n)
        val detail = selected?.let { engine.task(it, n) }
        if (detail == null) selected = null
        val status = engine.status()
        if (status.state == SyncState.ONLINE && status.pending == 0u) syncedAt = n
        val state = UiState(
            home = engine.homePlaced(place.rows, n),
            room = engine.roomPlaced(k, f, place.rows, n),
            detail = detail,
            status = status,
            failed = engine.failed(),
            toast = toast,
            unpairing = if (askingUnpair) UnpairQuestion(unsentCount(status), unpairChanged) else null,
            searching = open || f.isNotEmpty(),
            filter = f,
            collapsed = collapsed,
            syncedAt = syncedAt,
            showCompleted = showDone,
            generation = gen,
        )
        return Snapshot(epoch, state, Truth(ids.keys.associateWith { engine.taskDone(it) }, seen, place.version, ids))
    }

    /**
     * Re-reads the screen. Reads can finish out of order (a slow read for "m" after the read for
     * "milk"), so only the newest read so far is published, and only while the room, the filter, the
     * field's open state and Completed's it was read for are still the current ones: the rows always
     * belong to the filter on screen, and a closed field is never reopened by an old read. A read taken
     * with a placement the motion has since changed is dropped too, so rows never step back to where a
     * choreography no longer keeps them. Every change to those inputs, and to the placement, starts a
     * refresh of its own, so the newer read follows.
     */
    suspend fun refresh() {
        val generation = refreshes.incrementAndGet()
        val read = withContext(io) { snapshot() }
        val again = synchronized(motion) {
            val fresh = synchronized(publishing) {
                (generation > published && read.epoch == inputs && read.truth.placed == placement.version).also {
                    if (it) {
                        published = generation
                        _ui.value = read.state
                    }
                }
            }
            if (fresh) {
                shown = read.truth
                follow(read.state, read.truth)
            } else {
                false
            }
        }
        if (again) scope.launch { refresh() }
    }

    /**
     * After a read is published: the motion follows the data that changed under a
     * choreography (another device, an agent, a refused command), the sun and the dusk head for the new
     * count, and rows the list doesn't show stop moving. Returns whether that changed where the rows go,
     * so the screen must be read again. Under the [motion] lock.
     */
    private fun follow(state: UiState, truth: Truth): Boolean {
        val m = motion.motion
        val now = motion.now()
        val p = pace()
        // A task toggled after the read began isn't in it: its own state, and its row, stand until the
        // next read.
        fun ahead(id: String) = (toggled[id]?.serial ?: 0L) > truth.written
        m.reconcile(now, p) { id -> if (id in truth.done && !ahead(id)) truth.done[id] else m.expectedDone(id) }
        // A task that's gone has no row to keep: its toggle (whose write is refused) is let go. Only the
        // toggle the read saw, though: one made since (the task restored and toggled again) stands.
        toggled.entries.removeAll { (id, t) -> id in truth.done && truth.done[id] == null && truth.toggles[id] == t.serial }
        val room = state.room
        m.aimDusk(room.key, room.finished, now, p)
        // The finished day's sun travels from where it is to the end of its arc while the strip it's
        // drawn on fades under the dusk scene (with reduced motion it's there at once).
        m.aimSun(room.key, if (room.finished) 1f else sunAt(room.done, room.total), now, p)
        // Only a read with the current placement says what the list shows; an older one is followed by a
        // newer read.
        val rows = room.tasks
        val done = if (state.showCompleted) room.completed else emptyList()
        val listed = HashSet<String>()
        rows.mapTo(listed) { it.id }
        done.mapTo(listed) { it.id }
        fun visible(section: Section) = section == Section.ROWS || state.showCompleted
        val current = truth.placed == placement.version
        var letGo = false
        if (current) {
            val ids = m.ids()
            rows.forEachIndexed { i, t -> if (t.id in ids) lastSeen[t.id] = Ghost(t, Section.ROWS, i) }
            done.forEachIndexed { i, t -> if (t.id in ids) lastSeen[t.id] = Ghost(t, Section.COMPLETED, i) }
            for (id in ids) {
                val ghost = m.ghostOf(id)
                val seen = lastSeen[id]
                if (ghost != null) {
                    // Listed again (an agent changed it once more): the live row, as the read has it, at
                    // rest. A read from before its latest toggle doesn't decide that. A ghost in a section
                    // that has closed goes too.
                    if ((id in listed && !ahead(id)) || !visible(ghost.section)) m.drop(id)
                } else if (id !in listed && seen != null && visible(seen.section)) {
                    // Dropped while it plays, for a reason other than its done state (the placement keeps
                    // that): painted as it last was, it plays on.
                    m.ghost(id, seen, now, p)
                }
            }
            letGo = m.retainShown(now) { it in listed || ahead(it) }
            // A row whose settle has ended, once the read has its toggle written, and still listed (else it
            // has become a ghost, or stopped): it comes to rest. A toggle still to be written keeps it until
            // a read has that too.
            for (id in m.lingering()) if (!ahead(id)) m.letGo(id)
        }
        lastSeen.keys.retainAll(m.ids())
        val moved = placeRows()
        motion.touched()
        return letGo || moved
    }

    /** Follows the screen on show at once (after a toggle), without a new read. Under the [motion] lock. */
    private fun followNow(): Boolean = follow(_ui.value, Truth(emptyMap(), shown.written, shown.placed))

    /**
     * Keeps the reads' [placement] in step with the motion. Returns whether it changed. Under the [motion] lock.
     *
     * A task completing counts as open and one being reopened as done, until released. A toggle not yet
     * written (a second tap that reverses a choreography, say) keeps its task as it asked, open or done,
     * until the engine has it too, even past the end of its choreography: a read in between never moves
     * the row or changes the counts. Only the done state is placed: a row the room drops for another
     * reason plays on as a ghost ([follow]).
     */
    private fun placeRows(): Boolean {
        val m = motion.motion
        val ids = m.ids()
        toggled.entries.removeAll { (id, t) -> t.serial <= written && id !in ids }
        val held = m.holding()
        val back = m.reopening()
        val unwritten = toggled.filter { (id, t) -> t.serial > written && id !in held && id !in back }
        val holding = held + unwritten.filterValues { !it.done }.keys
        val reopening = back + unwritten.filterValues { it.done }.keys
        reopenedAt.keys.retainAll(reopening)
        val next = RowPlacement(
            holding.sorted(),
            reopening.sorted().mapNotNull { id -> reopenedAt[id]?.let { Reopening(id, it) } },
        )
        moving = (ids + toggled.keys).associateWith { toggled[it]?.serial ?: 0L }
        if (next == placement.rows) return false
        placement = Placed(placement.version + 1, next)
        return true
    }

    private fun act(block: () -> Unit) {
        scope.launch {
            writes.withLock {
                withContext(io) {
                    try {
                        block()
                    } catch (e: FfiException) {
                        show(Toast(++serial, PairingModel.describe(e)))
                    }
                }
            }
            refresh()
        }
    }

    private fun show(t: Toast?) {
        toast = t
    }

    /**
     * Opens a room, even the one already open. A filter belongs to the room it was typed in, so the
     * filter and its field are cleared together.
     */
    override fun open(room: RoomKey) {
        enter(room)
        scope.launch { refresh() }
    }

    /**
     * Every way into a room goes through here ([open], [createList]). A new room starts at rest:
     * choreographies under way, the rows kept for writes still on their way, and the ghosts are let go, so
     * the rows are the engine's own. Nothing is selected, the filter and its field are cleared, and
     * Completed is closed; a read for the room before is never published. A new room generation starts:
     * a tap on a row of the room before only writes.
     */
    private fun enter(room: RoomKey) {
        synchronized(motion) {
            motion.motion.settleAll()
            toggled.clear()
            lastSeen.clear()
            placeRows()
            motion.touched()
            synchronized(publishing) {
                generation++
                key = room
                selected = null
                filter = ""
                searching = false
                showCompleted = false
                inputs++
                _ui.value = _ui.value.copy(filter = "", searching = false, showCompleted = false)
            }
        }
    }

    override fun setShowCompleted(on: Boolean) {
        synchronized(motion) {
            synchronized(publishing) {
                showCompleted = on
                inputs++
                _ui.value = _ui.value.copy(showCompleted = on)
            }
            followNow()
        }
        scope.launch { refresh() }
    }

    /**
     * The model holds the filter: the field shows [UiState.filter], so the text on screen and the rows
     * under it always come from here. The text is published at once (typing stays responsive), the rows
     * when they have been read.
     */
    override fun setFilter(text: String) {
        synchronized(publishing) {
            filter = text
            inputs++
            _ui.value = _ui.value.copy(filter = text, searching = searching || text.isNotEmpty())
        }
        scope.launch { refresh() }
    }

    /** Opens or closes the filter field; closing it clears the filter. */
    override fun setSearching(on: Boolean) {
        synchronized(publishing) {
            searching = on
            if (!on) filter = ""
            inputs++
            _ui.value = _ui.value.copy(searching = on || filter.isNotEmpty(), filter = filter)
        }
        scope.launch { refresh() }
    }

    override fun toggleCollection(id: String) {
        collapsed = if (id in collapsed) collapsed - id else collapsed + id
        scope.launch { refresh() }
    }

    /** Adds a list and opens it. */
    override fun createList(name: String) = act { enter(engine.createList(name)) }

    override fun select(id: String?) {
        selected = id
        scope.launch { refresh() }
    }

    // The room is read now, on the caller's thread: the task goes where the owner was when they typed
    // it, even if they switch rooms before the IO action runs.
    override fun add(title: String) {
        val room = key
        act { engine.addTask(room, title, now()) }
    }

    /**
     * Completes or reopens a task: written at once, while its row plays the choreography where it is (a
     * second toggle part way reverses it in place). A task the list doesn't show just changes, and so does
     * one tapped on a row of a room before this one ([generation] isn't the current one): its write lands
     * in a room that never showed that row, so nothing there moves, is placed or is painted. A ghost's row
     * is inert: a tap on it does nothing at all (the Undo toast, with no [generation], still reverses it).
     */
    override fun setDone(id: String, done: Boolean, generation: Long?) {
        val (toggle, again) = synchronized(motion) {
            if (generation != null && generation != this.generation) return@synchronized null to false
            val m = motion.motion
            if (generation != null && m.isGhost(id)) return
            val at = motion.now()
            val room = _ui.value.room
            // The row as the screen shows it: its section, and (reopening) when it had been completed.
            val row = room.tasks.firstOrNull { it.id == id }
            val doneRow = room.completed.firstOrNull { it.id == id }
            val from = when {
                row != null -> Section.ROWS
                doneRow != null -> Section.COMPLETED
                done -> Section.ROWS
                else -> Section.COMPLETED
            }
            val to = when {
                listsDoneAmongRows(room.key) -> Section.ROWS
                done -> Section.COMPLETED
                else -> Section.ROWS
            }
            val toggle = ++toggles
            toggled[id] = Toggle(toggle, done)
            // Where the row is, should the room drop it while it plays (a ghost keeps its own).
            if (!m.isGhost(id)) {
                if (row != null) {
                    lastSeen[id] = Ghost(row, Section.ROWS, room.tasks.indexOf(row))
                } else if (doneRow != null && _ui.value.showCompleted) {
                    lastSeen[id] = Ghost(doneRow, Section.COMPLETED, room.completed.indexOf(doneRow))
                }
            }
            if (done) {
                m.complete(id, at, pace(), from, to)
            } else {
                m.reopen(id, at, pace(), from, to)
                (row ?: doneRow)?.completedAt?.let { reopenedAt[id] = it }
            }
            toggle to followNow()
        }
        if (again) scope.launch { refresh() }
        act {
            try {
                val title = engine.task(id, now())?.title
                engine.setDone(id, done)
                if (done && title != null) show(Toast(++serial, "Completed “$title”", reopen = id))
            } finally {
                // Written (or refused): the engine has the task as it is now, so the placement lets it go
                // before the read that follows.
                if (toggle != null) {
                    synchronized(motion) {
                        if (toggle > written) written = toggle
                        placeRows()
                    }
                }
            }
        }
    }

    override fun undo(t: Toast) {
        if (toast?.serial == t.serial) show(null)
        val id = t.reopen
        if (id != null) setDone(id, false) else scope.launch { refresh() }
    }

    override fun dismissToast(t: Toast) {
        if (toast?.serial == t.serial) {
            show(null)
            scope.launch { refresh() }
        }
    }

    override fun rename(id: String, title: String) = act { engine.rename(id, title) }

    override fun setMyDay(id: String, on: Boolean) = act { engine.setMyDay(id, on, now()) }

    override fun setImportant(id: String, on: Boolean) = act { engine.setImportant(id, on) }

    /**
     * Adds a step and says how it went: null once it is added, else why not (a title too long, say),
     * so the field can keep the draft and show the reason in the detail pane, where a toast is hidden.
     */
    override fun stepDraft(task: String): StepDraft = stepDrafts.getOrPut(task) { StepDraft(task) }

    /**
     * Drops the drafts of tasks that are done or gone, except one on its way (its answer is still to
     * land; the refresh after it drops it) and the open task's, which its pane may still show.
     */
    private fun pruneDrafts(n: LocalNow) {
        for ((id, d) in stepDrafts) {
            if (d.sending || id == selected) continue
            val t = engine.task(id, n)
            if (t == null || t.done) stepDrafts.remove(id, d)
        }
    }

    override fun addStep(task: String, title: String, onResult: (String?) -> Unit) {
        scope.launch {
            val problem = writes.withLock {
                withContext(io) {
                    try {
                        engine.addStep(task, title)
                        null
                    } catch (e: FfiException) {
                        PairingModel.describe(e)
                    }
                }
            }
            onResult(problem)
            refresh()
        }
    }

    override fun setStepDone(id: String, done: Boolean) = act { engine.setStepDone(id, done) }

    override fun dismissFailed(id: String) = act { engine.dismissFailed(id) }

    /** Whether the app is in the foreground: the latest call to [resume] or [pause] wins. */
    @Volatile private var foreground = false

    private val lifecycle = Mutex()

    /** The app came to the foreground: sync and follow live changes. */
    fun resume() {
        foreground = true
        applyLifecycle()
    }

    /** The app went to the background: stop the stream. The outbox stays on disk. */
    fun pause() {
        foreground = false
        applyLifecycle()
    }

    /**
     * Brings the engine in line with the latest foreground state. Transitions run one at a time and each
     * reads [foreground] when it runs, not when it was queued, so a start that was still waiting when the
     * app went to the background stops instead of reopening the stream.
     */
    private fun applyLifecycle() {
        scope.launch {
            withContext(io) {
                lifecycle.withLock {
                    try {
                        if (foreground) engine.start() else engine.stop()
                    } catch (e: FfiException) {
                        show(Toast(++serial, PairingModel.describe(e)))
                    }
                }
            }
            refresh()
        }
    }

    /** Shows a message, for example after the cache was recovered. */
    fun notice(text: String) {
        show(Toast(++serial, text))
        scope.launch { refresh() }
    }

    /** The network came back, or the owner asked: retry now instead of waiting out the backoff. */
    override fun nudge() = act { engine.nudge() }

    override fun unpair() {
        askingUnpair = true
        unpairChanged = false
        scope.launch { refresh() }
    }

    override fun cancelUnpair() {
        askingUnpair = false
        unpairChanged = false
        scope.launch { refresh() }
    }

    /**
     * The owner confirmed, having seen [unsent] unsent changes. The count is read again behind every write
     * already made, and while no other write can start; if it differs, the question shows the new count
     * and asks again. Otherwise the unpair runs before any later write.
     */
    override fun confirmUnpair(unsent: Int) {
        scope.launch {
            writes.withLock {
                val now = withContext(io) { unsentCount(engine.status()) }
                if (!askingUnpair) return@withLock
                if (now != unsent) {
                    unpairChanged = true
                } else {
                    askingUnpair = false
                    unpairChanged = false
                    onUnpair()
                }
            }
            refresh()
        }
    }
}
