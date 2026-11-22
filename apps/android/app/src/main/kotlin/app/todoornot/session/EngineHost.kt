package app.todoornot.session

import app.todoornot.core.CacheRecovery
import app.todoornot.core.FfiException
import app.todoornot.core.ProblemKind
import app.todoornot.core.SyncStatus
import app.todoornot.core.TodoEngine
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import java.io.File
import java.io.IOException

/** Where the app stands with its pairing and its local cache. */
sealed interface Host {
    /** Not paired: show pairing. */
    data object SignedOut : Host

    /** Paired, with the engine open. [recovered] is set when opening had to set a damaged cache aside. */
    data class Ready(val session: Session, val engine: TodoEngine, val recovered: CacheRecovery?) : Host

    /**
     * Paired, but the local cache couldn't be opened, not even by setting it aside (the engine never
     * deletes it, and finishes the recovery on the next try). The pairing is kept: the owner can try
     * again, or unpair on purpose.
     */
    data class CantOpen(val session: Session, val problem: String) : Host
}

internal val SyncStatus.signedOut: Boolean get() = problem == ProblemKind.UNAUTHORIZED

/** How an unpair ended. */
sealed interface Unpaired {
    /** The pairing and all of its data are gone. */
    data object Clean : Unpaired

    /** Unpaired, but some of the data couldn't be removed yet. It is marked, and removed at the next start. */
    data object DataLeft : Unpaired

    /** Nothing was removed (the intent couldn't be recorded): still paired, and unpairing can be tried again. */
    data class StillPaired(val reason: String) : Unpaired
}

/**
 * Opens the engine for the saved pairing, each pairing on its own cache ([PairingFiles]). A failure to
 * open never forgets the pairing: unsent changes live in the cache, so it shows [Host.CantOpen] with a
 * retry instead. Only [unpair], on the owner's request, forgets it.
 */
class EngineHost(
    private val sessions: SessionStore,
    private val files: PairingFiles,
    /** Destroys the token's key once the pairing is forgotten. */
    private val forgetKey: () -> Unit,
    private val newPairingId: () -> String = { PairingFiles.newPairingId() },
    /** Debug-level log lines (never task content). */
    private val debug: (String) -> Unit = {},
    private val openEngine: (Session, File) -> TodoEngine,
) {
    private val _state = MutableStateFlow<Host>(Host.SignedOut)
    val state: StateFlow<Host> = _state.asStateFlow()

    /**
     * Opens the saved pairing, if there is one. First finishes whatever an earlier unpair left: if the
     * saved pairing itself was being unpaired when the app stopped, that unpair is completed.
     */
    fun load() {
        var s = sessions.load()
        if (s != null && s.pairingId.isEmpty()) {
            // Saved before pairings had their own directories (never in a release): a new, empty cache.
            s = s.copy(pairingId = newPairingId())
            sessions.save(s)
        }
        files.oldLayoutFiles().takeIf { it.isNotEmpty() }?.let {
            debug("${it.size} cache files from before pairing directories are left alone")
        }
        if (s != null && files.isSignedOut(s.pairingId)) {
            sessions.clear()
            forgetKey()
            s = null
        }
        files.sweep(keep = s?.pairingId)
        s?.let(::open)
    }

    /** A new pairing was approved: finish any earlier unpair, keep the pairing, then open its own cache. */
    fun paired(session: Session) {
        val s = session.copy(pairingId = newPairingId())
        files.sweep(keep = null)
        sessions.save(s)
        open(s)
    }

    /** The app's signed-out status is sticky for this pairing, even when the engine restarts offline. */
    internal fun status(ready: Host.Ready): SyncStatus {
        val sync = ready.engine.status()
        return if (sessions.signedOut(ready.session.pairingId, sync.signedOut) && !sync.signedOut) {
            sync.copy(problem = ProblemKind.UNAUTHORIZED, problemDetail = null)
        } else sync
    }

    internal fun isAuthorized(ready: Host.Ready): Boolean = try {
        state.value === ready && !status(ready).signedOut && state.value === ready
    } catch (_: IllegalStateException) {
        false // The current engine may have closed during unpair.
    }

    /** Tries to open the cache again after [Host.CantOpen]. */
    fun retry() {
        (_state.value as? Host.CantOpen)?.let { open(it.session) }
    }

    /**
     * Forgets the pairing on the owner's request, in order: records that its data is to be removed
     * (nothing happens without that record), forgets the token and its key, lets [close] stop and close
     * the open engine, and removes the data, the record last. A crash at any point is finished by the
     * next [load] or [paired].
     */
    fun unpair(close: (TodoEngine) -> Unit): Unpaired {
        val current = _state.value
        val session = when (current) {
            is Host.Ready -> current.session
            is Host.CantOpen -> current.session
            Host.SignedOut -> return Unpaired.Clean
        }
        try {
            files.markSignedOut(session.pairingId)
        } catch (e: IOException) {
            return Unpaired.StillPaired(e.message ?: "the unpair couldn't be recorded")
        }
        sessions.clear()
        forgetKey()
        (current as? Host.Ready)?.engine?.let(close)
        _state.value = Host.SignedOut
        return if (files.remove(session.pairingId)) Unpaired.Clean else Unpaired.DataLeft
    }

    private fun open(session: Session) {
        _state.value = try {
            val engine = openEngine(session, files.cacheToOpen(session.pairingId))
            Host.Ready(session, engine, engine.recovered())
        } catch (e: FfiException) {
            Host.CantOpen(session, problem(e))
        } catch (e: IOException) {
            debug("the pairing's directory isn't usable: ${e.message}")
            Host.CantOpen(session, FOLDER_PROBLEM)
        }
    }

    companion object {
        const val FOLDER_PROBLEM =
            "This phone’s folder for your tasks isn’t a plain folder any more, so the app won’t open it. Nothing has been deleted."

        fun problem(e: FfiException): String = when (e) {
            is FfiException.Storage -> "This phone’s copy of your tasks couldn’t be opened. Nothing unsent has been deleted."
            is FfiException.BadAddress -> "The saved server address isn’t usable any more: ${e.reason}"
            else -> "The app couldn’t start syncing."
        }

        /** What to tell the owner after a recovery, or null when nothing needs saying. */
        fun recoveredNotice(r: CacheRecovery?): String? = when {
            r == null -> null
            r.lost > 0u -> "Recovered from a damaged copy: ${r.kept} unsent changes kept, ${r.lost} couldn’t be read."
            r.kept > 0u -> "Recovered from a damaged copy: all ${r.kept} unsent changes kept."
            else -> "Recovered from a damaged copy. Nothing was waiting to be sent."
        }
    }
}
