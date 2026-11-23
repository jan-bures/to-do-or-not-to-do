package app.todoornot.model

import app.todoornot.core.BulkBar
import app.todoornot.core.BulkEdit
import app.todoornot.core.RoomSelection
import app.todoornot.core.ColorTarget
import app.todoornot.core.ActivityRow
import app.todoornot.core.ActivityLoad
import app.todoornot.core.ActivityPager
import app.todoornot.core.ActivityView
import app.todoornot.core.AgentUpdate
import app.todoornot.core.OrganizeChange
import app.todoornot.core.OrganizeDeletion
import app.todoornot.core.Completion
import app.todoornot.core.ChangeListener
import app.todoornot.core.Deletion
import app.todoornot.core.FailedChange
import app.todoornot.core.FfiException
import app.todoornot.core.FieldEdit
import app.todoornot.core.Holding
import app.todoornot.core.Home
import app.todoornot.core.ListDeletion
import app.todoornot.core.LocalNow
import app.todoornot.core.ReminderChange
import app.todoornot.core.ReminderRef
import app.todoornot.core.Reopening
import app.todoornot.core.RoomKey
import app.todoornot.core.RoomView
import app.todoornot.core.RowPlacement
import app.todoornot.core.SmartList
import app.todoornot.core.SyncState
import app.todoornot.core.SyncStatus
import app.todoornot.core.SyncText
import app.todoornot.core.TaskMove
import app.todoornot.core.TaskDrag
import app.todoornot.core.TaskDrop
import app.todoornot.core.TaskDetail
import app.todoornot.core.TodoEngine
import app.todoornot.core.localNow
import app.todoornot.core.QuickAddDraft
import app.todoornot.core.reminderSetText
import app.todoornot.core.syncStatusText
import kotlinx.coroutines.CoroutineDispatcher
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.channels.BufferOverflow
import kotlinx.coroutines.flow.MutableSharedFlow
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.NonCancellable
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import kotlinx.coroutines.withContext
import kotlinx.coroutines.withTimeoutOrNull
import app.todoornot.reminders.reminderLabel
import app.todoornot.session.signedOut
import java.time.LocalDate
import java.time.ZoneId
import java.time.ZoneOffset
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.atomic.AtomicLong
import java.util.concurrent.atomic.AtomicReference

/** The task ID drives only the row animation; the handle identifies exactly which event to undo. */
data class CompletionAction(val task: String, val target: Completion, val done: Boolean = true)

/**
 * A short message above the composer, with an optional undo: of a completion (which plays the row's
 * motion back), a deletion, a move or a reminder change.
 */
data class Toast(
    val serial: Long,
    val text: String,
    val completion: CompletionAction? = null,
    val deletion: Deletion? = null,
    val moved: TaskMove? = null,
    val swap: TaskSwap? = null,
    val reminder: ReminderChange? = null,
    val organize: OrganizeChange? = null,
    val deadlineMs: Long = 0,
) {
    val undoable: Boolean get() = completion != null || deletion != null || moved != null || reminder != null || organize != null
}

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
     * The room generation the rows were read in: every way into a room starts a new one (and so does the
     * engine falling back to another room). A row tap and the detail pane pass it back, so a toggle made in
     * the room before ([Actions.setDone]) only writes.
     */
    val generation: Long = 0L,
    /** Today's date (ISO) by the model's clock: where the date picker opens for a task with no due date. */
    val today: String = "",
    /** Last local reorder (including Undo). Only these two rows glide when its read lands. */
    val reorder: TaskSwap? = null,
    /** null for a task room, empty for all activity, otherwise the agent's wire ID. */
    val activity: String? = null,
    /** The generation in which this pager was created, unchanged if the underlying task room moves. */
    val pagerId: Long = 0L,
    val feed: ActivityView? = null,
    val attachment: TransferStatus? = null,
    val attachmentTaskConfirmed: Boolean = false,
    val bulkIds: List<String> = emptyList(),
    val bulk: BulkBar? = null,
) {
    val selected: String? get() = detail?.id
    val signedOut: Boolean get() = status.signedOut
}

data class TaskSwap(val serial: Long, val room: RoomKey, val rows: Set<String>)

/**
 * The unpair confirmation: how many changes on this phone haven't reached the server (queued or refused).
 * [changed] is set when the owner confirmed a number that was out of date: they are asked again.
 */
data class UnpairQuestion(val unsent: Int, val changed: Boolean = false)

/** The drawer's compact sync label, its accessible description, and the dot's tone. */
fun syncLine(status: SyncStatus, syncedAt: LocalNow?): SyncText =
    syncStatusText(status, syncedAt?.let(::clock))

/** "09:05", in the time's own UTC offset. */
fun clock(t: LocalNow): String {
    val minutes = Math.floorDiv(t.epochMs + t.utcOffsetSecs * 1000L, 60_000L)
    val day = Math.floorMod(minutes, 24L * 60L)
    return "%02d:%02d".format(day / 60, day % 60)
}

/** The local date of [t] (ISO, `2026-11-21`), in its own UTC offset. */
fun isoDay(t: LocalNow): String =
    LocalDate.ofEpochDay(Math.floorDiv(t.epochMs + t.utcOffsetSecs * 1000L, 86_400_000L)).toString()

/**
 * The detail with its Reminder row worded in the phone's [zone], at the offset that applies at the
 * reminder's own instant. The core words it at the phone's offset now, which is an hour off for a reminder
 * on the other side of a change of the clocks.
 */
fun localReminder(detail: TaskDetail, now: LocalNow, zone: ZoneId): TaskDetail {
    val r = detail.reminder ?: return detail
    val label = reminderLabel(r.atMs, now.epochMs, zone)
    return detail.copy(fields = detail.fields.map { if (it.edit == FieldEdit.REMINDER) it.copy(value = label) else it })
}

/** A name on one line, for a toast. */
private fun oneLine(s: String): String = s.replace(Regex("[\\p{Cntrl}\u2028\u2029]"), " ")

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
    /**
     * The phone's time zone, for the times the model words itself (a reminder's): the app passes the
     * system's. By default the fixed offset [now] carries, so tests and the gallery don't depend on the
     * machine's zone.
     */
    private val zone: () -> ZoneId = { ZoneOffset.ofTotalSeconds(now().utcOffsetSecs) },
    documents: Documents? = null,
    private val syncStatus: () -> SyncStatus = engine::status,
) : Actions {
    @Volatile private var key: RoomKey = start

    private val selection = AtomicReference<String?>(null)
    private val selected get() = selection.get()

    @Volatile private var toast: Toast? = null

    @Volatile private var serial = 0L
    @Volatile private var reorder: TaskSwap? = null

    @Volatile private var askingUnpair = false

    @Volatile private var unpairChanged = false

    @Volatile private var filter = ""

    @Volatile private var searching = false
    private class ActivityRoom(val filter: String, val id: Long) {
        val pager = ActivityPager(filter.ifEmpty { null })
    }
    @Volatile private var activity: ActivityRoom? = null

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

    // A draft stays with its task through rotation, navigation during a save, or a refused save.
    // Never put potentially long notes in the Activity's saved-state Bundle.
    private val noteDrafts = ConcurrentHashMap<String, NotesDraft>()

    override fun notesDraft(task: String): NotesDraft = noteDrafts.getOrPut(task) { NotesDraft(task) }

    private fun selectTask(id: String?) {
        val previous = selection.getAndSet(id)
        if (previous == null || previous == id) return
        transfers.forget(previous)
        val draft = noteDrafts[previous] ?: return
        draft.save(this) // No-op for a closed editor or a save already on its way.
        if (draft.text == null) noteDrafts.remove(previous, draft)
    }

    @Volatile private var syncedAt: LocalNow? = null

    @Volatile private var showCompleted = false

    /**
     * The screen's motion. Its lock also guards the bookkeeping below that follows it, so a read published
     * from any thread (the engine calls back on its own) is followed whole, in order.
     */
    val motion = MotionHost(clock)

    /**
     * Where the reads keep the rows in their choreography, and how often that has changed (or which rows are
     * [ghosts]: a read that lists one again shows it as it is, not placed).
     */
    private class Placed(val version: Long, val rows: RowPlacement, val ghosts: Set<String> = emptySet())

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

    /** A completing row's last agent update, so it keeps its place in Agent activity. */
    private val heldUpdates = HashMap<String, AgentUpdate>()

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

    // Protected by publishing. Selection is local to a room generation, never synced or persisted.
    private val bulkSelection = RoomSelection()
    private var bulkRevision = 0L
    private var bulkPending = false

    private val transfers = AttachmentTransfers(AttachmentTransferCore(engine), documents, scope, io, { selected }, ::publishTransfer)
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

    /**
     * A read: the [inputs] it was read for, the room [key] asked for, the screen, and the state of the tasks
     * in motion.
     */
    private class Snapshot(val epoch: Long, val key: RoomKey, val state: UiState, val truth: Truth)

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
        val reading = selected
        val detail = reading?.let { engine.task(it, n) }?.let { localReminder(it, n, zone()) }
        if (detail == null) selection.compareAndSet(reading, null)
        val status = syncStatus()
        if (!status.signedOut && status.state == SyncState.ONLINE && status.pending == 0u) syncedAt = n
        val readingActivity = activity
        val state = UiState(
            home = engine.homePlaced(place.rows, n),
            room = roomRead(k, f, place, ids, seen, n),
            detail = detail,
            attachment = transfers.status(detail?.id),
            attachmentTaskConfirmed = detail != null && engine.attachmentTaskConfirmed(detail.id),
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
            today = isoDay(n),
            activity = readingActivity?.filter,
            pagerId = readingActivity?.id ?: 0L,
            feed = readingActivity?.let { engine.activityView(it.pager, n) },
            reorder = reorder?.takeIf { it.room == k },
        )
        return Snapshot(epoch, k, state, Truth(ids.keys.associateWith { engine.taskDone(it) }, seen, place.version, ids))
    }

    /**
     * The room, with the rows in their choreography placed. A ghost the room lists again (an agent changed it
     * once more) is no longer placed among the rows: it's listed as it is, done or not, as the live row that
     * replaces it ([follow]). The counts stay placed, until a read without it in motion. A ghost toggled
     * after the read began ([toggles] newer than [seen]) stays placed: that read doesn't decide it.
     */
    private fun roomRead(k: RoomKey, f: String, place: Placed, toggles: Map<String, Long>, seen: Long, n: LocalNow): RoomView {
        val room = engine.roomPlaced(k, f, place.rows, n)
        if (place.ghosts.isEmpty()) return room
        val back = HashSet<String>()
        for (t in room.tasks + room.completed) if (t.id in place.ghosts && (toggles[t.id] ?: 0L) <= seen) back += t.id
        if (back.isEmpty()) return room
        val rows = RowPlacement(
            place.rows.holding.filter { it.id !in back },
            place.rows.reopening.filter { it.id !in back },
        )
        val free = engine.roomPlaced(k, f, rows, n)
        return room.copy(tasks = free.tasks, completed = free.completed)
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
            // The engine resolved the room to another (a sync deleted its list: My Day): that's a new room,
            // entered here. Nothing of the room before plays on in it, and the read for it follows.
            val moved = synchronized(publishing) { read.epoch == inputs && read.state.room.key != read.key }
            if (moved) {
                restart(read.state.room.key, fresh = false)
                return@synchronized true
            }
            val fresh = synchronized(publishing) {
                (generation > published && read.epoch == inputs && read.truth.placed == placement.version).also {
                    if (it) {
                        published = generation
                        val ids = if (_ui.value.bulkIds.isEmpty()) emptyList() else bulkSelection.retain(bulkVisible(read.state))
                        _ui.value = withBulk(read.state.copy(attachment = transfers.status(read.state.selected)), ids)
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
        fun doneNow(id: String) = if (id in truth.done && !ahead(id)) truth.done[id] else m.expectedDone(id)
        val room = state.room
        // Only a read with the current placement says what the list shows; an older one is followed by a
        // newer read.
        val rows = room.tasks
        val done = if (state.showCompleted) room.completed else emptyList()
        val listed = HashSet<String>()
        rows.mapTo(listed) { it.id }
        done.mapTo(listed) { it.id }
        fun visible(section: Section) = section == Section.ROWS || state.showCompleted
        val current = truth.placed == placement.version
        if (current) {
            // Where each row in motion is painted, among the rows and the ghosts on screen.
            val ids = m.ids()
            val ghosts = m.ghosts()
            for (t in rows) if (t.id in ids && !m.isGhost(t.id)) lastSeen[t.id] = ghostAt(t, Section.ROWS, rows, ghosts)
            for (t in done) if (t.id in ids && !m.isGhost(t.id)) lastSeen[t.id] = ghostAt(t, Section.COMPLETED, done, ghosts)
            // Placement keeps the done state and pre-completion agent update. If the room still drops
            // the row, paint it as it was: a ghost. Do this before reconcile can stop a settle the new
            // state no longer matches. A deleted task's row stops at once instead.
            for (id in ids) {
                if (m.isGhost(id) || id in listed) continue
                val seen = lastSeen[id] ?: continue
                if (visible(seen.section) && doneNow(id) != null) m.ghost(id, seen, now, p)
            }
        }
        m.reconcile(now, p, ::doneNow)
        // A task that's gone has no row to keep: its toggle (whose write is refused) is let go. Only the
        // toggle the read saw, though: one made since (the task restored and toggled again) stands.
        toggled.entries.removeAll { (id, t) -> id in truth.done && truth.done[id] == null && truth.toggles[id] == t.serial }
        m.aimDusk(room.key, room.finished, now, p)
        // The finished day's sun travels from where it is to the end of its arc while the strip it's
        // drawn on fades under the dusk scene (with reduced motion it's there at once).
        m.aimSun(room.key, if (room.finished) 1f else sunAt(room.done, room.total), now, p)
        var letGo = false
        if (current) {
            for (id in m.ids()) {
                val ghost = m.ghostOf(id) ?: continue
                // Listed again (an agent changed it once more): the live row, as the read has it, at rest.
                // A read from before its latest toggle doesn't decide that. A ghost in a section that has
                // closed goes too.
                if ((id in listed && !ahead(id)) || !visible(ghost.section)) m.drop(id)
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
     * the row or changes the counts. The old agent update is restored too. A row the room drops for
     * another reason plays on as a ghost ([follow]).
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
        heldUpdates.keys.retainAll(holding)
        val next = RowPlacement(
            holding.sorted().map { id -> Holding(id, heldUpdates[id]) },
            reopening.sorted().mapNotNull { id -> reopenedAt[id]?.let { Reopening(id, it) } },
        )
        moving = (ids + toggled.keys).associateWith { toggled[it]?.serial ?: 0L }
        val ghosts = m.ghostIds()
        if (next == placement.rows && ghosts == placement.ghosts) return false
        placement = Placed(placement.version + 1, next, ghosts)
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
        toast = t?.copy(deadlineMs = clock() + app.todoornot.design.Tokens.Motion.UNDO_WINDOW)
    }

    override fun toastRemaining(t: Toast): Long = (t.deadlineMs - clock()).coerceAtLeast(0L)

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
    private fun enter(room: RoomKey) = restart(room, fresh = true)

    /**
     * Starts [room] at rest, in a new room generation ([enter]). Only a [fresh] start (the owner opened it)
     * clears the selection, the filter and Completed; the engine falling back to another room ([refresh])
     * keeps them.
     */
    override fun openActivity(agent: String?) {
        restart(key, fresh = true, feed = agent.orEmpty())
        scope.launch { refresh() }
    }

    private fun restart(room: RoomKey, fresh: Boolean, feed: String? = null) {
        synchronized(motion) {
            motion.motion.settleAll()
            toggled.clear()
            lastSeen.clear()
            placeRows()
            motion.touched()
            synchronized(publishing) {
                generation++
                bulkSelection.clear()
                bulkRevision++
                _ui.value = withBulk(_ui.value, emptyList())
                reorder = null
                key = room
                if (fresh) {
                    activity = feed?.let { ActivityRoom(it, generation) }
                    selectTask(null)
                    filter = ""
                    searching = false
                    showCompleted = false
                    _ui.value = _ui.value.copy(filter = "", searching = false, showCompleted = false)
                }
                inputs++
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

    /** Adds a list in the current collection, or at the top level, and opens it. */
    override fun createList(name: String) {
        val room = key
        act { enter(engine.createList(name, room)) }
    }

    override fun select(id: String?) {
        if (id != null && activity != null) enter(key)
        if (_ui.value.bulkIds.isNotEmpty()) clearBulk()
        selectTask(id)
        scope.launch { refresh() }
    }

    private fun bulkVisible(state: UiState): List<String> = buildList {
        addAll(state.room.tasks.map { it.id })
        state.room.tomorrow?.let { plan ->
            add(plan.first.id)
            addAll(plan.also.map { it.id })
            addAll(plan.week.map { it.id })
        }
        if (state.showCompleted) addAll(state.room.completed.map { it.id })
    }

    private fun withBulk(state: UiState, ids: List<String>): UiState = state.copy(
        bulkIds = ids,
        bulk = if (ids.isEmpty()) null else engine.bulkBar(ids, state.today),
        detail = state.detail.takeIf { ids.isEmpty() },
    )

    override fun toggleBulk(id: String, generation: Long) {
        synchronized(publishing) {
            val state = _ui.value
            if (generation != this.generation || generation != state.generation || state.signedOut) return
            val visible = bulkVisible(state)
            if (id !in visible) return
            selectTask(null)
            val ids = bulkSelection.toggle(visible, id)
            inputs++
            bulkRevision++
            _ui.value = withBulk(state.copy(detail = null), ids)
        }
        scope.launch { refresh() }
    }

    override fun clearBulk() {
        synchronized(publishing) {
            bulkSelection.clear()
            bulkRevision++
            _ui.value = withBulk(_ui.value, emptyList())
        }
    }

    override fun editBulk(edit: BulkEdit) {
        val (ids, revision) = synchronized(publishing) {
            val state = _ui.value
            val ids = state.bulkIds
            if (ids.size < 2 || state.generation != generation || state.signedOut || bulkPending) return
            bulkPending = true
            ids to bulkRevision
        }
        act {
            try {
                engine.bulkEdit(ids, edit)?.let { change ->
                    show(Toast(++serial, change.toastText(), organize = change))
                    synchronized(publishing) { if (revision == bulkRevision) clearBulk() }
                }
            } finally {
                synchronized(publishing) { bulkPending = false }
            }
        }
    }

    override suspend fun bulkDestinations(tasks: List<String>, filter: String) =
        withContext(io) { engine.bulkDestinations(tasks, filter) }

    override fun composerNow(): LocalNow = now()

    // Capture the room on the caller's thread, even if they switch before the IO action runs.
    override fun add(draft: QuickAddDraft) {
        val room = key
        act { engine.addTaskDraft(room, draft, now()) }
    }

    /**
     * Completes or reopens a task: written at once, while its row plays the choreography where it is (a
     * second toggle part way reverses it in place). A task the list doesn't show just changes, and so does
     * one toggled in a room before this one: a row tap or the detail pane's checkbox with a [generation]
     * that isn't the current one, or Undo (no [generation]) while the screen still shows the room before
     * (another room opened, and its read hasn't landed). Its write lands in a room that never showed that
     * row, so nothing there moves, is placed or is painted. A ghost's [row] is inert: a tap on it does
     * nothing at all (the Undo toast and the detail pane still reverse it).
     */
    override fun setDone(id: String, done: Boolean, generation: Long?, row: Boolean) =
        toggle(id, done, generation, row, undo = null)

    private fun toggle(id: String, done: Boolean, generation: Long?, row: Boolean, undo: Completion?) {
        val (toggle, again) = synchronized(motion) {
            if ((generation ?: _ui.value.generation) != this.generation) return@synchronized null to false
            val m = motion.motion
            if (row && m.isGhost(id)) return
            val at = motion.now()
            val room = _ui.value.room
            // The row as the screen shows it: its section, and (reopening) when it had been completed.
            val openRow = room.tasks.firstOrNull { it.id == id }
            val doneRow = room.completed.firstOrNull { it.id == id }
            val from = when {
                openRow != null -> Section.ROWS
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
            // Where the row is painted, among the rows and the ghosts, should the room drop it while it plays
            // (a ghost keeps its own).
            if (!m.isGhost(id)) {
                if (openRow != null) {
                    lastSeen[id] = ghostAt(openRow, Section.ROWS, room.tasks, m.ghosts())
                } else if (doneRow != null && _ui.value.showCompleted) {
                    lastSeen[id] = ghostAt(doneRow, Section.COMPLETED, room.completed, m.ghosts())
                }
            }
            if (done) {
                (openRow ?: doneRow)?.agentUpdate?.let { heldUpdates[id] = it }
                m.complete(id, at, pace(), from, to)
            } else {
                m.reopen(id, at, pace(), from, to)
                (openRow ?: doneRow)?.completedAt?.let { reopenedAt[id] = it }
            }
            toggle to followNow()
        }
        if (again) scope.launch { refresh() }
        act {
            try {
                if (undo != null) {
                    engine.undoCompletion(undo)
                } else {
                    val title = engine.task(id, now())?.title
                    val target = engine.setDone(id, done)
                    if (target != null && title != null) {
                        val verb = if (done) "Completed" else "Reopened"
                        show(Toast(++serial, "$verb “$title”", CompletionAction(id, target, done)))
                    }
                }
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
        if (toast?.serial != t.serial) return
        if (toastRemaining(t) == 0L) { dismissToast(t); return }
        t.organize?.let { change ->
            show(null)
            act {
                engine.undoOrganize(change)
                t.swap?.let { reorder = it.copy(serial = ++serial) }
            }
            return
        }
        t.reminder?.let { change ->
            show(null)
            act { engine.undoReminder(change) }
            return
        }
        t.deletion?.let { deletion ->
            show(null)
            act { engine.undoDeletion(deletion)?.let { show(Toast(++serial, it)) } }
            return
        }
        t.moved?.let { moved ->
            show(null)
            act {
                engine.undoTaskMove(moved)
                t.swap?.let { reorder = it.copy(serial = ++serial) }
            }
            return
        }
        val action = t.completion ?: return
        show(null)
        toggle(action.task, !action.done, generation = null, row = false, undo = action.target)
    }

    override fun dismissToast(t: Toast) {
        if (toast?.serial == t.serial) {
            show(null)
            scope.launch { refresh() }
        }
    }

    override fun restoreTrash(row: app.todoornot.core.TrashRow) {
        if (key != RoomKey.Trash) return
        act {
            val change = engine.restoreTrash(row)
            show(Toast(++serial, change.toastText(), organize = change))
        }
    }

    override fun deleteTask(id: String) {
        noteDrafts[id]?.save(this)
        act {
            val title = engine.task(id, now())?.title ?: return@act
            val target = engine.deleteTask(id)
            // The next read drops this detail if it is still selected, not a newer selection.
            show(Toast(++serial, "Deleted “${oneLine(title)}”", deletion = target))
        }
    }

    override suspend fun moveDestinations(id: String, filter: String) = withContext(io) { engine.moveDestinations(id, filter) }

    override fun moveTaskTo(id: String, from: String, list: String) {
        val here = key
        noteDrafts[id]?.save(this)
        act {
            val title = engine.task(id, now())?.title ?: return@act
            val dest = RoomKey.List(list)
            val group = _ui.value.home.groups.firstOrNull { g -> g.lists.any { it.key == dest } }
            val name = group?.lists?.firstOrNull { it.key == dest }?.name
            engine.moveTaskTo(id, from, list)?.let { target ->
                val leaves = when (here) {
                    is RoomKey.List -> here != dest
                    is RoomKey.Collection -> group != null && group.collection?.key != here
                    is RoomKey.Smart, RoomKey.Trash -> false
                }
                val open = selected
                if (key == here && leaves && open == id) selection.compareAndSet(open, null)
                val where = name?.let { " to ${oneLine(it)}" }.orEmpty()
                show(Toast(++serial, "Moved “${oneLine(title)}”$where", moved = target))
            }
        }
    }

    override suspend fun mergeCandidates(id: String, filter: String) = withContext(io) { engine.mergeCandidates(id, filter) }

    override suspend fun mergeQuestion(id: String, target: String) = withContext(io) { engine.mergeQuestion(id, target) }

    override fun mergeTasks(id: String, target: String, asked: String, onResult: (String?) -> Unit) {
        noteDrafts[id]?.save(this)
        scope.launch {
            var again: String? = null
            writeEdit {
                when (val result = engine.mergeTasks(id, target, asked)) {
                    is app.todoornot.core.MergeResult.Merged -> {
                        selection.compareAndSet(id, null)
                        show(Toast(++serial, result.change.toastText(), organize = result.change))
                    }
                    is app.todoornot.core.MergeResult.Changed -> {
                        again = result.question
                        if (again == null) show(Toast(++serial, app.todoornot.core.mergeChangedText()))
                    }
                }
            }?.let { show(Toast(++serial, it)) }
            refresh()
            onResult(again)
        }
    }

    override fun reorderTask(list: String, id: String, neighbour: String, by: Int) {
        val room = RoomKey.List(list)
        if (key != room) return
        act {
            val title = engine.task(id, now())?.title ?: return@act
            engine.reorderTask(list, id, neighbour, by)?.let { target ->
                val n = ++serial
                val swap = TaskSwap(n, room, setOf(id, neighbour))
                reorder = swap
                show(Toast(n, "Moved “${oneLine(title)}”", moved = target, swap = swap))
            }
        }
    }

    private fun dragState(generation: Long): UiState? = _ui.value.takeIf {
        it.generation == generation && this.generation == generation && !it.signedOut && it.activity == null
    }

    private fun openDragOrder(state: UiState) = state.room.tasks.filterNot { it.done }.map { it.id }

    override fun startTaskDrag(id: String, generation: Long): TaskDrag? {
        val state = dragState(generation) ?: return null
        if (id !in bulkVisible(state)) return null
        return engine.startTaskDrag(id, state.bulkIds)
    }

    override fun taskDropValid(drag: TaskDrag, target: TaskDrop, generation: Long): Boolean {
        val state = dragState(generation) ?: return false
        return engine.taskDropValid(drag, target, state.room.key, openDragOrder(state), state.today)
    }

    override fun dropTasks(drag: TaskDrag, target: TaskDrop, generation: Long) {
        val revision = synchronized(publishing) { bulkRevision }
        act {
            val state = dragState(generation) ?: return@act
            val open = openDragOrder(state)
            val change = engine.dropTasks(drag, target, state.room.key, open, state.today) ?: return@act
            val n = ++serial
            val swap = if (target is TaskDrop.Slot) TaskSwap(n, state.room.key, open.toSet()) else null
            if (swap != null) reorder = swap
            else synchronized(publishing) { if (revision == bulkRevision) clearBulk() }
            show(Toast(n, change.toastText(), organize = change, swap = swap))
        }
    }

    override fun rename(id: String, title: String) = act { engine.rename(id, title) }

    override fun swipeTask(id: String, generation: Long, action: TaskSwipe) {
        val ui = _ui.value
        if (generation != ui.generation || ui.bulkIds.isNotEmpty() || ui.signedOut || ui.activity != null) return
        val task = ui.room.tasks.firstOrNull { it.id == id }
            ?: ui.room.completed.takeIf { ui.showCompleted }?.firstOrNull { it.id == id }
            ?: return
        when (action) {
            TaskSwipe.COMPLETION -> setDone(id, !(motion.checked(id) ?: task.done), generation)
            TaskSwipe.MY_DAY -> setMyDay(id, !task.myDay)
        }
    }

    override fun setMyDay(id: String, on: Boolean) = act {
        engine.setMyDay(id, on, now())?.let { show(Toast(++serial, it.toastText(), organize = it)) }
    }

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

    override fun setNotes(id: String, notes: String, onResult: (String?) -> Unit) {
        scope.launch {
            val error = writeEdit { engine.setNotes(id, notes) }
            // Publish the saved text before switching back to the read-only label.
            refresh()
            onResult(error)
            if (id != selected) {
                if (error == null) {
                    noteDrafts[id]?.takeIf { it.text == null }?.let { noteDrafts.remove(id, it) }
                } else {
                    notice("Couldn't save notes: $error")
                }
            }
        }
    }

    override fun addStep(task: String, title: String, onResult: (String?) -> Unit) {
        scope.launch {
            onResult(writeEdit { engine.addStep(task, title) })
            refresh() // Prune drafts after the step is no longer in flight.
        }
    }

    private suspend fun writeEdit(edit: () -> Unit): String? = writes.withLock {
        withContext(io) {
            try {
                edit()
                null
            } catch (e: FfiException) {
                PairingModel.describe(e)
            }
        }
    }

    override fun setStepDone(id: String, done: Boolean) = act { engine.setStepDone(id, done) }

    override fun setDue(id: String, date: String?) = act { engine.setDue(id, date) }

    override fun setPriority(id: String, marks: Int) = act { engine.setPriority(id, marks.coerceIn(0, 3).toUByte()) }

    /**
     * Sets the reminder the owner saw ([seen], from the detail pane) to [atMs] (milliseconds since the Unix
     * epoch), adds one to a task that had none, or removes it ([atMs] null). If that reminder changed or
     * went while the picker was open, nothing is written and the toast says so. Otherwise the toast says
     * when it is now (the time saved, in the phone's zone) and offers Undo, which reverses exactly this.
     */
    override fun setReminder(id: String, seen: ReminderRef?, atMs: Long?) = act {
        val change = if (atMs == null) {
            engine.removeReminder(id, seen ?: return@act)
        } else {
            engine.setReminder(id, seen, atMs, now()) ?: return@act
        }
        val text = if (atMs == null) "Reminder removed" else reminderSetText(reminderLabel(atMs, now().epochMs, zone()))
        show(Toast(++serial, text, reminder = change))
    }

    /**
     * Completes a task from its reminder's notification: the same command as the checkbox's, so it syncs.
     * No row plays (the app is usually not on screen; a change from elsewhere settles anything that does)
     * and no toast offers Undo. Once the completion is in the outbox (on disk), [saved] runs: only then may
     * the notification go. With the app in the background it then syncs for up to [syncFor] ms, to send it
     * now. Returns whether the completion was saved.
     */
    suspend fun completeFromNotification(id: String, syncFor: Long = NOTIFICATION_SYNC_MS, saved: () -> Unit = {}): Boolean {
        if (!applyExternalTaskEdit { it.setDone(id, true) }) return false
        saved()
        syncInBackground(syncFor) { it.pending == 0u }
        return true
    }

    /** A system surface's edit uses the same write lock and on-disk outbox as the app. */
    internal suspend fun applyExternalTaskEdit(
        isCurrent: () -> Boolean = { true },
        edit: (TodoEngine) -> Unit,
    ): Boolean {
        var authorized = false
        val error = writeEdit {
            // Check after waiting for earlier writes, not when the system tap is first delivered.
            authorized = isCurrent() && !syncStatus().signedOut
            if (authorized) edit(engine)
        }
        scope.launch { refresh() }
        return authorized && error == null
    }

    /**
     * With the app in the background, syncs for up to [budget] ms, or until [done] says the status is
     * enough, then stops again (also when cancelled). In the foreground it syncs anyway: nothing to do.
     */
    suspend fun syncInBackground(budget: Long, done: (SyncStatus) -> Boolean) {
        if (foreground || budget <= 0) return
        var started = false
        try {
            withContext(io) {
                lifecycle.withLock {
                    if (!foreground) {
                        if (backgroundSyncs == 0) engine.start()
                        backgroundSyncs++
                        started = true
                    }
                }
            }
            if (!started) return
            withTimeoutOrNull(budget) {
                do delay(SYNC_POLL_MS) while (!done(withContext(io) { syncStatus() }))
            }
        } finally {
            if (started) withContext(NonCancellable + io) {
                lifecycle.withLock {
                    backgroundSyncs--
                    if (!foreground && backgroundSyncs == 0) engine.stop()
                }
            }
        }
    }

    override fun renameStep(id: String, title: String) = act { engine.renameStep(id, title) }

    override fun moveStep(id: String, by: Int) = act { engine.moveStep(id, by) }

    /** Deletes a step: the toast names it and offers Undo, which brings it back exactly as it was. */
    override fun deleteStep(id: String) {
        val title = _ui.value.detail?.steps?.firstOrNull { it.id == id }?.title
        act {
            val target = engine.deleteStep(id)
            show(Toast(++serial, title?.let { "Deleted step “${oneLine(it)}”" } ?: "Deleted a step", deletion = target))
        }
    }

    private fun publishTransfer(): Unit = synchronized(publishing) {
        _ui.value = _ui.value.copy(attachment = transfers.status(_ui.value.selected))
    }

    override fun transfer(action: FileAction) = transfers.start(action)
    override fun attachmentNotice(task: String, text: String) = transfers.notice(task, text)

    override fun organize(edit: OrganizeEdit) = act {
        fun tracked(change: OrganizeChange?) {
            if (change != null) show(Toast(++serial, change.toastText(), organize = change))
        }
        when (edit) {
            is OrganizeEdit.Repeat -> tracked(engine.setRepeat(edit.task, edit.choice, now()))
            is OrganizeEdit.ToggleTag -> engine.toggleTag(edit.task, edit.id)
            is OrganizeEdit.TagNamed -> engine.tagTaskNamed(edit.task, edit.name)
            is OrganizeEdit.RenameTag -> engine.renameTag(edit.id, edit.name)
            is OrganizeEdit.Color -> engine.setSwatch(edit.target, edit.hue)
            is OrganizeEdit.AddLink -> tracked(engine.addWebLink(edit.task, edit.address))
            is OrganizeEdit.RemoveAttachment -> tracked(engine.removeAttachment(edit.id))
            is OrganizeEdit.RemoveLink -> tracked(engine.removeLink(edit.id))
            is OrganizeEdit.NewCollection -> enter(engine.createCollection(edit.name))
            is OrganizeEdit.MoveList -> tracked(engine.moveList(edit.id, edit.collection))
        }
    }

    override fun tagNamed(name: String) = engine.tagNamed(name)
    override fun previewRepeat(id: String, draft: app.todoornot.core.CustomRepeat) = engine.previewRepeat(id, draft, now())

    override fun organizeQuestion(target: ColorTarget) = engine.organizeQuestion(target)

    override fun confirmDelete(target: ColorTarget, asked: String, result: (String?) -> Unit) {
        scope.launch {
            var next: String? = null
            writeEdit {
                when (val reply = engine.confirmOrganizeDelete(target, asked)) {
                    is OrganizeDeletion.Changed -> next = reply.question
                    is OrganizeDeletion.Deleted -> show(Toast(++serial, reply.change.toastText(), organize = reply.change))
                }
            }?.let { show(Toast(++serial, it)) }
            refresh()
            result(next)
        }
    }

    private fun pagerFor(id: Long) = activity?.takeIf { it.id == id }?.pager

    override fun loadActivity(pagerId: Long, load: ActivityLoad) {
        val pager = pagerFor(pagerId) ?: return
        // Belongs to the model, not the composable: rotation doesn't strand a pending read.
        scope.launch {
            val started = withContext(io) { engine.beginActivity(pager, load) }
            refresh() // A refresh queued behind an older read changes the view without starting I/O.
            var request = started ?: return@launch
            while (true) {
                val next = withContext(io) {
                    if (activity?.pager === pager) engine.readActivity(pager, request) else null
                }
                if (activity?.pager !== pager) return@launch
                refresh()
                request = next ?: break
            }
        }
    }

    override fun undoEvent(pagerId: Long, event: ActivityRow) {
        val pager = pagerFor(pagerId) ?: return
        act { engine.undoActivityEvent(pager, event.seq, event.id) }
    }

    override fun renameRoom(room: RoomKey, name: String) = act {
        when (room) {
            is RoomKey.List -> engine.renameList(room.id, name)
            is RoomKey.Collection -> engine.renameCollection(room.id, name)
            is RoomKey.Smart, RoomKey.Trash -> Unit
        }
    }

    override fun moveRoom(room: RoomKey, by: Int) = act { engine.moveRoom(room, by) }

    override fun deleteListQuestion(id: String): String? = engine.deleteListQuestion(id)

    /**
     * Deletes a list and its tasks, if [asked] is still what deleting it means: the engine checks the
     * question again behind every write already made, and if the list was renamed or tasks came or went
     * meanwhile, deletes nothing and [onResult] gets the new question. If it's the room on screen, My Day
     * opens (as on the desktop); the toast names the list and offers Undo either way.
     */
    override fun deleteList(id: String, asked: String, onResult: (String?) -> Unit) {
        val name = _ui.value.home.groups.flatMap { it.lists }.firstOrNull { (it.key as? RoomKey.List)?.id == id }?.name
        scope.launch {
            var again: String? = null
            writeEdit {
                when (val r = engine.deleteList(id, asked)) {
                    is ListDeletion.Deleted -> {
                        val here = key
                        if (here is RoomKey.List && here.id == id) enter(RoomKey.Smart(SmartList.MY_DAY))
                        show(Toast(++serial, name?.let { "Deleted list “${oneLine(it)}”" } ?: "Deleted a list", deletion = r.undo))
                    }
                    is ListDeletion.Changed -> {
                        again = r.question
                        if (r.question == null) show(Toast(++serial, "That list was deleted elsewhere."))
                    }
                }
            }?.let { show(Toast(++serial, it)) }
            refresh()
            onResult(again)
        }
    }

    override fun dismissFailed(id: String) = act { engine.dismissFailed(id) }

    /** Whether the app is in the foreground: the latest call to [resume] or [pause] wins. */
    @Volatile private var foreground = false

    private val lifecycle = Mutex()
    // Protected by lifecycle: a reminder and a widget may catch up at the same time.
    private var backgroundSyncs = 0

    /** The app came to the foreground: sync and follow live changes. */
    fun resume() {
        foreground = true
        applyLifecycle()
    }

    /** The app went to the background: stop the stream. The outbox stays on disk. */
    fun pause() {
        transfers.cancel()
        engine.cancelAttachmentTransfers()
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
                        if (foreground || backgroundSyncs > 0) engine.start() else engine.stop()
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

/** How long a completion from a notification syncs for, with the app in the background. */
const val NOTIFICATION_SYNC_MS = 8_000L

private const val SYNC_POLL_MS = 250L
