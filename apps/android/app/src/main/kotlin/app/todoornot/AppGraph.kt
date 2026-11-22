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
import app.todoornot.model.CorePairing
import app.todoornot.model.Pace
import app.todoornot.model.PairingModel
import app.todoornot.model.TodoModel
import app.todoornot.reminders.AndroidAlarms
import app.todoornot.reminders.Reminders
import app.todoornot.session.EngineHost
import app.todoornot.session.Host
import app.todoornot.session.KeystoreCipher
import app.todoornot.session.PairingFiles
import app.todoornot.session.PrefsKeyValues
import app.todoornot.session.SessionStore
import app.todoornot.session.Unpaired
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.MainScope
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.channels.BufferOverflow
import kotlinx.coroutines.flow.MutableSharedFlow
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.filterNotNull
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.launch
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

    val host = EngineHost(
        SessionStore(PrefsKeyValues(app), cipher),
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

    /** The reminders' alarms and notifications, kept in line with the synced reminders. */
    val reminders = Reminders(PrefsKeyValues(app, "reminders"), AndroidAlarms(app), debug = { Log.d("Reminders", it) })

    /** Work for the reminders (and their broadcasts), off the main thread. */
    private val reminderScope = CoroutineScope(SupervisorJob() + Dispatchers.IO)

    /** The engine reported a change: the alarms follow, at most one pass waiting. */
    private val reminderChanges = MutableSharedFlow<Unit>(extraBufferCapacity = 1, onBufferOverflow = BufferOverflow.DROP_OLDEST)

    val pairing = PairingModel(CorePairing(Cleartext.allowLoopbackHttp), scope, Dispatchers.IO, deviceName(), host::paired)

    init {
        NativeInit.ensure(app)
        reminderScope.launch { reminderChanges.collect { reconcileReminders() } }
        scope.launch { host.state.collect(::follow) }
        host.load()
        ProcessLifecycleOwner.get().lifecycle.addObserver(object : DefaultLifecycleObserver {
            override fun onStart(owner: LifecycleOwner) {
                foreground = true
                _model.value?.resume()
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
    private fun follow(h: Host) {
        val engine = (h as? Host.Ready)?.engine
        if (_model.value?.engine === engine) return
        modelScope?.cancel()
        modelScope = null
        _model.value = engine?.let { e ->
            val ms = CoroutineScope(SupervisorJob() + Dispatchers.Main)
            modelScope = ms
            // One listener per engine: the model's, which also tells the reminders.
            val listen = { onChange: () -> Unit ->
                e.setListener(object : ChangeListener {
                    override fun onChange() {
                        onChange()
                        reminderChanges.tryEmit(Unit)
                    }
                })
            }
            TodoModel(e, ms, Dispatchers.IO, onUnpair = ::unpair, pace = ::pace, listen = listen).also { m ->
                EngineHost.recoveredNotice((h as Host.Ready).recovered)?.let(m::notice)
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
        val engine = engine() ?: return
        try {
            reminders.reconcile(engine, force)
        } catch (e: IllegalStateException) {
            // The engine was closed meanwhile (unpaired): nothing to follow any more.
            Log.d("Reminders", "the engine closed during a pass: ${e::class.simpleName}")
        }
    }

    /** Snooze 10 min, from a reminder's notification. */
    fun snoozeReminder(id: String) = reminders.snooze(id, engine())

    /** Complete, from a reminder's notification: the app's own command, which syncs. */
    suspend fun completeFromReminder(id: String, task: String) {
        reminders.dismiss(id)
        // The model may still be on its way when the process started for this broadcast.
        withTimeoutOrNull(MainActivity.MODEL_WAIT_MS) { model.filterNotNull().first() }?.completeFromNotification(task)
        reconcileReminders()
    }

    /** Runs a broadcast's [work] off the main thread, then [finish]es the broadcast. */
    fun reminderWork(finish: () -> Unit, work: suspend () -> Unit) {
        reminderScope.launch {
            try {
                work()
            } catch (e: FfiException) {
                Log.d("Reminders", "a reminder action failed: ${e::class.simpleName}")
            } catch (e: IllegalStateException) {
                Log.d("Reminders", "a reminder action failed: ${e::class.simpleName}")
            } finally {
                finish()
            }
        }
    }

    /** Opens the cache again after it couldn't be opened. */
    fun retry() = host.retry()

    /**
     * Forgets the server once the owner has confirmed it: stops syncing, deletes the token, its key and
     * this pairing's cache (with any copy set aside after damage). Anything not yet sent is lost, which
     * is why the app never does this by itself and always asks first, saying how much would be lost.
     */
    fun unpair() {
        val m = _model.value
        val result = host.unpair { engine ->
            _model.value = null
            modelScope?.cancel()
            modelScope = null
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
}

/**
 * Plain http to a loopback server (the emulator workflow) only in the debug build type, from an explicit
 * build config field: a release build made debuggable still talks HTTPS only.
 */
object Cleartext {
    val allowLoopbackHttp: Boolean = BuildConfig.ALLOW_LOOPBACK_HTTP
}
