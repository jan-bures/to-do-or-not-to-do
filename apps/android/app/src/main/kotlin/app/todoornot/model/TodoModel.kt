package app.todoornot.model

import app.todoornot.core.ChangeListener
import app.todoornot.core.FailedChange
import app.todoornot.core.FfiException
import app.todoornot.core.Home
import app.todoornot.core.LocalNow
import app.todoornot.core.ProblemKind
import app.todoornot.core.RoomKey
import app.todoornot.core.RoomView
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
    /** The filter typed into the bar: query language or words; empty when not filtering. */
    val filter: String = "",
    /** Collections folded in the drawer, by ID. */
    val collapsed: Set<String> = emptySet(),
    /** When the engine was last seen online and caught up, for "Synced 10:35". */
    val syncedAt: LocalNow? = null,
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

/** Unsent changes, from one status snapshot (the engine reads both counts under one lock). */
fun unsentCount(status: SyncStatus): Int = (status.pending + status.failed).toInt()

/**
 * The main screen's model over the Rust engine. The engine keeps the state, the cache and the outbox;
 * this class only remembers what is open, reads the views, and turns taps into commands. Every engine
 * call runs on `io`, and the state is re-read whenever the engine says something changed.
 */
class TodoModel(
    val engine: TodoEngine,
    private val scope: CoroutineScope,
    private val io: CoroutineDispatcher,
    private val now: () -> LocalNow = { localNow() },
    start: RoomKey = RoomKey.Smart(SmartList.MY_DAY),
    private val onUnpair: () -> Unit = {},
) : Actions {
    @Volatile private var key: RoomKey = start

    @Volatile private var selected: String? = null

    @Volatile private var toast: Toast? = null

    @Volatile private var serial = 0L

    @Volatile private var askingUnpair = false

    @Volatile private var unpairChanged = false

    @Volatile private var filter = ""

    @Volatile private var collapsed: Set<String> = emptySet()

    @Volatile private var syncedAt: LocalNow? = null

    /**
     * Local writes run one at a time, in the order they were made (the mutex is first come, first served).
     * Confirming an unpair takes its count behind them, so an edit already on its way is counted.
     */
    private val writes = Mutex()

    private val _ui = MutableStateFlow(read())
    val ui: StateFlow<UiState> = _ui.asStateFlow()

    private val changes = MutableSharedFlow<Unit>(extraBufferCapacity = 1, onBufferOverflow = BufferOverflow.DROP_OLDEST)

    init {
        engine.setListener(object : ChangeListener {
            override fun onChange() {
                changes.tryEmit(Unit)
            }
        })
        scope.launch { changes.collect { refresh() } }
    }

    /** Reads the whole screen state synchronously (the screenshot tests use this directly). */
    fun read(): UiState {
        val n = now()
        val detail = selected?.let { engine.task(it, n) }
        if (detail == null) selected = null
        val status = engine.status()
        if (status.state == SyncState.ONLINE && status.pending == 0u) syncedAt = n
        val f = filter
        return UiState(
            home = engine.home(n),
            room = if (f.isBlank()) engine.room(key, n) else engine.roomMatching(key, f, n),
            detail = detail,
            status = status,
            failed = engine.failed(),
            toast = toast,
            unpairing = if (askingUnpair) UnpairQuestion(unsentCount(status), unpairChanged) else null,
            filter = f,
            collapsed = collapsed,
            syncedAt = syncedAt,
        )
    }

    suspend fun refresh() {
        _ui.value = withContext(io) { read() }
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

    /** Opens a room. A filter belongs to the room it was typed in, so it is cleared. */
    override fun open(room: RoomKey) {
        key = room
        selected = null
        filter = ""
        scope.launch { refresh() }
    }

    override fun setFilter(text: String) {
        filter = text
        scope.launch { refresh() }
    }

    override fun toggleCollection(id: String) {
        collapsed = if (id in collapsed) collapsed - id else collapsed + id
        scope.launch { refresh() }
    }

    /** Adds a list and opens it. */
    override fun createList(name: String) = act {
        val room = engine.createList(name)
        key = room
        selected = null
        filter = ""
    }

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

    override fun setDone(id: String, done: Boolean) = act {
        val title = engine.task(id, now())?.title
        engine.setDone(id, done)
        if (done && title != null) show(Toast(++serial, "Completed “$title”", reopen = id))
    }

    override fun undo(t: Toast) = act {
        t.reopen?.let { engine.setDone(it, false) }
        if (toast?.serial == t.serial) show(null)
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

    override fun addStep(task: String, title: String) = act { engine.addStep(task, title) }

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
