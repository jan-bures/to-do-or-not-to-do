package app.todoornot

import android.content.Context
import android.net.ConnectivityManager
import android.net.Network
import android.os.Build
import android.provider.Settings
import android.util.Log
import androidx.lifecycle.DefaultLifecycleObserver
import androidx.lifecycle.LifecycleOwner
import androidx.lifecycle.ProcessLifecycleOwner
import app.todoornot.core.ChangeListener
import app.todoornot.core.EngineConfig
import app.todoornot.core.FfiException
import app.todoornot.core.NativeInit
import app.todoornot.core.TodoEngine
import app.todoornot.files.AttachmentFiles
import app.todoornot.model.CorePairing
import app.todoornot.model.Pace
import app.todoornot.model.PairingModel
import app.todoornot.model.TodoModel
import app.todoornot.push.AndroidPush
import app.todoornot.push.PushWork
import app.todoornot.reminders.AndroidAlarms
import app.todoornot.reminders.Reminders
import app.todoornot.session.EngineHost
import app.todoornot.session.Host
import app.todoornot.session.KeystoreCipher
import app.todoornot.session.PairingFiles
import app.todoornot.session.PrefsKeyValues
import app.todoornot.session.SessionStore
import app.todoornot.session.Unpaired
import app.todoornot.session.signedOut
import app.todoornot.widget.WidgetRefresh
import android.os.SystemClock
import app.todoornot.core.ProblemKind
import app.todoornot.core.SyncState
import java.time.Instant
import java.time.ZoneId
import java.util.concurrent.atomic.AtomicInteger
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.MainScope
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.channels.BufferOverflow
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableSharedFlow
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import app.todoornot.model.CoreDevicePairing
import app.todoornot.model.DevicePairingModel
import app.todoornot.model.PairingScan
import kotlinx.coroutines.flow.collectLatest
import kotlinx.coroutines.flow.filterNotNull
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.job
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import kotlinx.coroutines.withTimeoutOrNull

/**
 * The app's long-lived pieces: the pairing, the engine, the models and the reminders' alarms. It follows
 * the process lifecycle: syncing (with the live stream) while the app is in the foreground, stopped in the
 * background, and nudged when the network comes back. The alarms follow every change the engine reports.
 */
class AppGraph(private val app: Context) {
    private val scope: CoroutineScope = MainScope()
    private val cipher = KeystoreCipher()
    private var modelScope: CoroutineScope? = null
    private var foreground = false

    private val sessions = SessionStore(PrefsKeyValues(app), cipher)
    val host = EngineHost(
        sessions,
        PairingFiles(app.noBackupFilesDir),
        forgetKey = cipher::destroy,
        debug = { Log.d("EngineHost", it) },
    ) { s, db ->
        TodoEngine.open(EngineConfig(s.server, s.token, s.deviceId, db.absolutePath, Cleartext.allowLoopbackHttp))
    }

    private val _unpairProblem = MutableStateFlow<String?>(null)

    /** Why the last unpair didn't go ahead, for the can't-open screen (the main screen shows a toast). */
    val unpairProblem: StateFlow<String?> = _unpairProblem.asStateFlow()

    private val _model = MutableStateFlow<TodoModel?>(null)
    val model: StateFlow<TodoModel?> = _model.asStateFlow()
    private val _loaded = MutableStateFlow(false)
    internal val loaded = _loaded.asStateFlow()

    internal val widgets = WidgetRefresh(app)
    internal val push = AndroidPush(app)

    /** The reminders' alarms and notifications, kept in line with the synced reminders. */
    val reminders = Reminders(PrefsKeyValues(app, "reminders"), AndroidAlarms(app), debug = { Log.d("Reminders", it) })

    /** Work for the reminders (and their broadcasts), off the main thread. */
    private val reminderScope = CoroutineScope(SupervisorJob() + Dispatchers.IO)

    /** The engine reported a change: the alarms follow, at most one pass waiting. */
    private val reminderChanges = MutableSharedFlow<Unit>(extraBufferCapacity = 1, onBufferOverflow = BufferOverflow.DROP_OLDEST)

    val pairing = PairingModel(CorePairing(Cleartext.allowLoopbackHttp), scope, Dispatchers.IO, deviceName(), host::paired)
    internal val pairingScan = PairingScan()
    internal var devicePairing: DevicePairingModel? = null
        private set

    // Don't expose a model until stale staging and viewer files have been cleared off Main.
    private var transferCleanup = scope.launch(Dispatchers.IO) { AttachmentFiles.clear(app.cacheDir) }

    init {
        NativeInit.ensure(app)
        scope.launch(Dispatchers.IO) {
            sessions.revocationPending.collectLatest { id ->
                if (id == null) return@collectLatest
                var retryIn = 250L
                while (sessions.revocationPending.value == id) {
                    delay(retryIn)
                    sessions.retryRevocation(id)
                    retryIn = (retryIn * 2).coerceAtMost(30_000L)
                }
            }
        }
        reminderScope.launch { reminderChanges.collect { reconcileReminders() } }
        scope.launch {
            withContext(Dispatchers.IO) { host.load() }
            host.state.collectLatest { h ->
                transferCleanup.join()
                if (h === host.state.value) {
                    follow(h)
                    _loaded.value = true
                }
            }
        }
        ProcessLifecycleOwner.get().lifecycle.addObserver(object : DefaultLifecycleObserver {
            override fun onStart(owner: LifecycleOwner) {
                foreground = true
                _model.value?.resume()
                (host.state.value as? Host.Ready)?.let { PushWork.register(app, it.session.pairingId) }
                // Notifications may have been allowed in the settings meanwhile: show what is due.
                reminderChanges.tryEmit(Unit)
            }

            override fun onStop(owner: LifecycleOwner) {
                foreground = false
                _model.value?.pause()
            }
        })
        app.getSystemService(ConnectivityManager::class.java)?.registerDefaultNetworkCallback(object : ConnectivityManager.NetworkCallback() {
            override fun onAvailable(network: Network) {
                if (foreground) _model.value?.nudge()
            }
        })
    }

    /** Builds the model when an engine opens, and drops it otherwise. */
    private suspend fun follow(h: Host) {
        widgets.invalidate()
        val ready = h as? Host.Ready
        if (_model.value?.engine === ready?.engine) return
        modelScope?.cancel()
        modelScope = null
        devicePairing = null
        _model.value = ready?.let { paired ->
            val e = paired.engine
            val ms = CoroutineScope(SupervisorJob() + Dispatchers.Main)
            modelScope = ms
            devicePairing = DevicePairingModel(CoreDevicePairing(paired.session, Cleartext.allowLoopbackHttp), ms)
            // One listener per engine: also let the reminders and home-screen widgets follow.
            val listen = { onChange: () -> Unit ->
                e.setListener(object : ChangeListener {
                    override fun onChange() {
                        // Record a known revocation before telling the app and its widgets to read it.
                        try {
                            host.status(paired)
                        } catch (_: IllegalStateException) {
                            return // The pairing closed while the callback was in flight.
                        }
                        onChange()
                        reminderChanges.tryEmit(Unit)
                        widgets.invalidate()
                    }
                })
            }
            withContext(Dispatchers.IO) {
                TodoModel(
                    e, ms, Dispatchers.IO, documents = AttachmentFiles(app), onUnpair = ::unpair,
                    pace = ::pace, listen = listen, zone = { ZoneId.systemDefault() },
                    syncStatus = { host.status(paired) },
                )
            }.also { m ->
                withContext(Dispatchers.IO) { push.activate(paired.session) { host.isAuthorized(paired) } }
                if (host.state.value === paired) PushWork.start(app, paired.session.pairingId)
                EngineHost.recoveredNotice(paired.recovered)?.let(m::notice)
                if (foreground) m.resume()
                reminderChanges.tryEmit(Unit)
            }
        }
    }

    /** The open engine, if the pairing's cache is open. */
    private fun engine(): TodoEngine? = (host.state.value as? Host.Ready)?.engine

    /**
     * Brings the alarms in line with the reminders; [force] sets every alarm again (after a reboot or a
     * clock change). Without an open engine (not paired, or the cache won't open) they stay as they are.
     */
    fun reconcileReminders(force: Boolean = false) {
        val ready = host.state.value as? Host.Ready ?: return
        try {
            reminders.reconcile(ready.engine, force, catchingUp = catchingUp.get() > 0) {
                if (host.state.value !== ready) false
                else if (host.isAuthorized(ready)) true
                else {
                    reminders.clear()
                    false
                }
            }
        } catch (e: IllegalStateException) {
            // The engine was closed meanwhile (unpaired): nothing to follow any more.
            Log.d("Reminders", "the engine closed during a pass: ${e::class.simpleName}")
        }
    }

    /** Snooze 10 min, from a reminder's notification. */
    fun snoozeReminder(id: String) = reminders.snooze(id, engine())

    /** Catch-up syncs under way: nothing due is shown until they end. */
    private val catchingUp = AtomicInteger()

    /** What is left of a broadcast's time, in milliseconds. */
    class Deadline(private val until: Long = now() + RECEIVER_BUDGET_MS) {
        fun left(): Long = (until - now()).coerceAtLeast(0)

        private companion object {
            fun now() = SystemClock.elapsedRealtime()
        }
    }

    /**
     * The model, once the engine is open; null at once when it isn't (not paired, or the cache won't
     * open), and after [wait] ms. It may still be on its way when the process started for a broadcast.
     */
    private suspend fun readyModel(wait: Long): TodoModel? {
        if (host.state.value !is Host.Ready) return null
        return withTimeoutOrNull(wait) { model.filterNotNull().first() }
    }

    /**
     * A reminder's alarm went off. The alarms come from the last synced state, so with the app in the
     * background it first catches up for a moment (what changed elsewhere: the reminder moved or removed,
     * the task completed), showing nothing meanwhile, then shows what is still due.
     */
    suspend fun reminderFired(deadline: Deadline) {
        val m = readyModel(minOf(MainActivity.MODEL_WAIT_MS, deadline.left() / 2))
        if (m != null) {
            catchingUp.incrementAndGet()
            try {
                m.syncInBackground(minOf(FIRE_SYNC_MS, deadline.left() - MARGIN_MS)) {
                    it.signedOut || (it.state == SyncState.ONLINE && it.caughtUp) ||
                        (it.state == SyncState.OFFLINE && it.problem == ProblemKind.NETWORK)
                }
            } finally {
                catchingUp.decrementAndGet()
            }
        }
        reconcileReminders()
    }

    /** A silent wake-up or periodic fallback: wait for a fresh backlog, then plan local alarms. */
    internal suspend fun syncReminders(pairing: String, wake: Instant?): Boolean {
        catchingUp.incrementAndGet()
        try {
            if (withTimeoutOrNull(MainActivity.MODEL_WAIT_MS) { loaded.first { it } } == null) return false
            val ready = host.state.value as? Host.Ready ?: return true
            if (ready.session.pairingId != pairing || !host.isAuthorized(ready)) return true
            if (wake != null) scope.launch(Dispatchers.IO) { push.woke(pairing, wake) }
            val m = readyModel(MainActivity.MODEL_WAIT_MS)?.takeIf { it.engine === ready.engine } ?: return false
            m.engine.nudge()
            return m.syncInBackground(FIRE_SYNC_MS) {
                it.signedOut || (it.state == SyncState.ONLINE && it.caughtUp && it.pending == 0u)
            }
        } catch (e: FfiException) {
            Log.d("Reminders", "background sync failed: ${e::class.simpleName}")
            return false
        } catch (_: IllegalStateException) {
            // The pairing closed during the wake-up.
            return true
        } finally {
            catchingUp.decrementAndGet()
            reconcileReminders()
        }
    }

    /**
     * Complete, from a reminder's notification: the app's own command, which syncs. The notification goes
     * only once the completion is saved; if the engine can't be opened, it stays, and nothing else happens.
     */
    suspend fun completeFromReminder(id: String, task: String, deadline: Deadline) {
        val m = readyModel(minOf(MainActivity.MODEL_WAIT_MS, deadline.left() / 2)) ?: return
        m.completeFromNotification(task, syncFor = deadline.left() - MARGIN_MS) { reminders.dismiss(id) }
        reconcileReminders()
    }

    /**
     * Runs a broadcast's [work] off the main thread, within one [Deadline] for the whole broadcast, then
     * [finish]es it.
     */
    fun reminderWork(finish: () -> Unit, work: suspend (Deadline) -> Unit) {
        reminderScope.launch {
            try {
                val deadline = Deadline()
                withTimeoutOrNull(RECEIVER_BUDGET_MS) {
                    loaded.first { it }
                    work(deadline)
                }
            } catch (e: FfiException) {
                Log.d("Reminders", "a reminder action failed: ${e::class.simpleName}")
            } catch (e: IllegalStateException) {
                Log.d("Reminders", "a reminder action failed: ${e::class.simpleName}")
            } finally {
                finish()
            }
        }
    }

    /** A create-document result returned after its pairing went away. */
    fun discardDocument(uri: String) { scope.launch(Dispatchers.IO) { AttachmentFiles(app).discardEmpty(uri) } }

    /** Opens the cache again after it couldn't be opened. */
    fun retry() = host.retry()

    /**
     * Forgets the server once the owner has confirmed it: stops syncing, deletes the token, its key and
     * this pairing's cache (with any copy set aside after damage). Anything not yet sent is lost, which
     * is why the app never does this by itself and always asks first, saying how much would be lost.
     */
    fun unpair() {
        val m = _model.value
        val oldModel = modelScope?.coroutineContext?.job
        val session = when (val current = host.state.value) {
            is Host.Ready -> current.session
            is Host.CantOpen -> current.session
            Host.SignedOut -> null
        }
        val result = host.unpair { engine ->
            _model.value = null
            modelScope?.cancel()
            modelScope = null
            engine.cancelAttachmentTransfers()
            engine.stop()
            engine.close()
        }
        when (result) {
            is Unpaired.StillPaired -> {
                val text = "Couldn’t unpair: ${result.reason}. This phone is still paired; try again."
                _unpairProblem.value = text
                m?.notice(text)
            }
            Unpaired.Clean, Unpaired.DataLeft -> {
                session?.let {
                    PushWork.cancel(app, it.pairingId)
                    scope.launch(Dispatchers.IO) { push.unpair(it) }
                }
                val previousCleanup = transferCleanup
                transferCleanup = scope.launch(Dispatchers.IO) {
                    previousCleanup.join()
                    oldModel?.join()
                    AttachmentFiles.clear(app.cacheDir)
                }
                reminderScope.launch { reminders.clear() }
                _unpairProblem.value = null
                pairing.cancel()
            }
        }
    }

    /**
     * How fast the next transition runs: the system's animator duration scale (Developer options, or
     * Accessibility's "Remove animations", which sets it to 0 for reduced motion). Read each time, so a
     * change applies to the next transition.
     */
    private fun pace(): Pace = Pace(Settings.Global.getFloat(app.contentResolver, Settings.Global.ANIMATOR_DURATION_SCALE, 1f))

    private fun deviceName(): String =
        Settings.Global.getString(app.contentResolver, Settings.Global.DEVICE_NAME)?.takeIf { it.isNotBlank() }
            ?: Build.MODEL

    companion object {
        /**
         * The time a reminder broadcast's work may take, all of it: well inside the ten seconds Android
         * allows a receiver that called `goAsync`.
         */
        const val RECEIVER_BUDGET_MS = 8_000L

        /** How long an alarm going off syncs for before showing the reminder. */
        const val FIRE_SYNC_MS = 4_000L

        /** Kept back from a broadcast's time for what follows its sync. */
        const val MARGIN_MS = 1_000L
    }
}

/**
 * Plain http to a loopback server (the emulator workflow) only in the debug build type, from an explicit
 * build config field: a release build made debuggable still talks HTTPS only.
 */
object Cleartext {
    val allowLoopbackHttp: Boolean = BuildConfig.ALLOW_LOOPBACK_HTTP
}
