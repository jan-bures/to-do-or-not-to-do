package app.todoornot.push

import android.content.Context
import android.util.Log
import app.todoornot.Cleartext
import app.todoornot.core.FfiException
import app.todoornot.core.PushOptions
import app.todoornot.core.TodoEngine
import app.todoornot.core.pushUnregister
import app.todoornot.session.Session
import com.google.firebase.FirebaseApp
import com.google.firebase.FirebaseOptions
import com.google.firebase.messaging.FirebaseMessaging
import java.io.IOException
import java.time.Instant
import java.time.ZoneId
import java.time.format.DateTimeFormatter
import java.util.Locale
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import kotlinx.coroutines.tasks.await
import kotlinx.coroutines.withTimeoutOrNull

/** On means that the paired server accepted this installation's token, not a delivery guarantee. */
data class PushStatus(val state: State = State.OFF, val lastWake: Instant? = null) {
    enum class State { OFF, NOT_CONFIGURED, ON }

    fun label(zone: ZoneId = ZoneId.systemDefault()): String = when (state) {
        State.OFF -> "Push: Off"
        State.NOT_CONFIGURED -> "Push: Not configured"
        State.ON -> "Push: On"
    } + (lastWake?.let { " · Last wake: ${format.withZone(zone).format(it)}" } ?: "")

    private companion object {
        val format = DateTimeFormatter.ofPattern("MMM d, HH:mm", Locale.getDefault())
    }
}

/** Testable handshake; an unpaired or superseded worker must never upload a token. */
internal suspend fun registerForPush(
    current: () -> Boolean,
    options: () -> PushOptions?,
    token: suspend (PushOptions) -> String?,
    register: (String) -> Unit,
    renew: suspend () -> String? = { null },
): PushStatus.State {
    if (!current()) return PushStatus.State.OFF
    val config = options() ?: return PushStatus.State.NOT_CONFIGURED
    if (!current()) return PushStatus.State.OFF
    val registration = token(config) ?: return PushStatus.State.OFF
    if (!current()) return PushStatus.State.OFF
    try {
        register(registration)
    } catch (e: FfiException.Refused) {
        if (e.status.toInt() != 409) throw e
        if (!current()) return PushStatus.State.OFF
        // Re-pairing can reuse the installation's old token. Ask Firebase for a different one;
        // the new pairing may not claim the old pairing's registration.
        val fresh = renew()?.takeUnless { it == registration } ?: return PushStatus.State.OFF
        if (!current()) return PushStatus.State.OFF
        register(fresh)
    }
    return PushStatus.State.ON
}

/** Firebase can wrap cleanup failures in SDK-specific exception types. Never swallow cancellation. */
private suspend fun bestEffortFirebaseCleanup(failed: () -> Unit, cleanup: suspend () -> Unit) {
    try {
        cleanup()
    } catch (e: CancellationException) {
        throw e
    } catch (_: Exception) {
        failed()
    }
}

internal suspend fun retireFirebase(deleteToken: suspend () -> Unit, clear: () -> Unit, failed: () -> Unit) {
    try {
        bestEffortFirebaseCleanup(failed, deleteToken)
    } finally {
        bestEffortFirebaseCleanup(failed) { clear() }
    }
}

/** One Firebase default app (needed by Messaging), bound to one pairing at a time. All I/O off Main. */
internal class AndroidPush(private val context: Context) {
    // Only public client options and the local wake time, never an FCM token or server bearer.
    private val prefs = context.getSharedPreferences("reminder_push", Context.MODE_PRIVATE)
    private val change = Mutex()
    private var firebase: FirebaseApp? = null
    private var pairing: String? = null
    private val _status = MutableStateFlow(PushStatus())
    val status = _status.asStateFlow()

    /** Available even when a background message launched the process and SQLite is still opening. */
    fun messagePairing(): String? = prefs.getString("pairing", null)

    private fun lastWake(id: String): Instant? = if (messagePairing() == id) {
        prefs.getLong("last_wake", 0L).takeIf { it > 0 }?.let(Instant::ofEpochMilli)
    } else null

    suspend fun activate(session: Session, current: () -> Boolean) = change.withLock {
        if (!current()) return@withLock
        if (pairing != session.pairingId) {
            firebase?.delete()
            firebase = null
            pairing = session.pairingId
            _status.value = PushStatus(lastWake = lastWake(session.pairingId))
        }
        if (firebase == null && messagePairing() == session.pairingId) {
            try {
                cachedOptions()?.let(::configure)
            } catch (_: IllegalStateException) {
                Log.d("ReminderPush", "could not restore Firebase options")
            }
        }
    }

    private fun cachedOptions(): PushOptions? {
        return PushOptions(
            projectId = prefs.getString("project", null) ?: return null,
            applicationId = prefs.getString("application", null) ?: return null,
            apiKey = prefs.getString("key", null) ?: return null,
            gcmSenderId = prefs.getString("sender", null) ?: return null,
        )
    }

    private fun configure(options: PushOptions) {
        val firebaseOptions = FirebaseOptions.Builder()
            .setProjectId(options.projectId)
            .setApplicationId(options.applicationId)
            .setApiKey(options.apiKey)
            .setGcmSenderId(options.gcmSenderId)
            .build()
        if (firebase?.options == firebaseOptions) return
        firebase?.delete()
        firebase = FirebaseApp.initializeApp(context, firebaseOptions)
    }

    suspend fun refresh(session: Session, engine: TodoEngine, authorized: () -> Boolean): Boolean = change.withLock {
        val id = session.pairingId
        val current = { pairing == id && authorized() }
        if (!current()) return@withLock true
        val state = try {
            registerForPush(
                current,
                options = engine::pushOptions,
                token = { options ->
                    configure(options)
                    val edit = prefs.edit()
                    if (messagePairing() != id) edit.clear()
                    edit.putString("pairing", id).putString("project", options.projectId)
                        .putString("application", options.applicationId).putString("key", options.apiKey)
                        .putString("sender", options.gcmSenderId).apply()
                    withTimeoutOrNull(TOKEN_WAIT_MS) { FirebaseMessaging.getInstance().token.await() }
                },
                register = engine::pushRegister,
                renew = {
                    withTimeoutOrNull(TOKEN_WAIT_MS) {
                        val messaging = FirebaseMessaging.getInstance()
                        messaging.deleteToken().await()
                        messaging.token.await()
                    }
                },
            )
        } catch (e: FfiException.Refused) {
            Log.d("ReminderPush", "push request refused (${e.status})")
            PushStatus.State.OFF
        } catch (e: FfiException) {
            Log.d("ReminderPush", "push setup failed: ${e::class.simpleName}")
            PushStatus.State.OFF
        } catch (e: java.util.concurrent.ExecutionException) {
            Log.d("ReminderPush", "Firebase couldn't refresh the installation token")
            PushStatus.State.OFF
        } catch (e: IOException) {
            Log.d("ReminderPush", "Firebase couldn't obtain a registration")
            PushStatus.State.OFF
        } catch (e: IllegalStateException) {
            Log.d("ReminderPush", "Firebase isn't available")
            PushStatus.State.OFF
        }
        if (!current()) return@withLock true
        val last = lastWake(id)
        if (state == PushStatus.State.NOT_CONFIGURED) clearFirebase(id)
        _status.value = PushStatus(state, last)
        state != PushStatus.State.OFF
    }

    suspend fun woke(id: String, at: Instant = Instant.now()) = change.withLock {
        if (pairing == id && messagePairing() == id && lastWake(id)?.isAfter(at) != true) {
            prefs.edit().putLong("last_wake", at.toEpochMilli()).apply()
            _status.value = _status.value.copy(lastWake = at)
        }
    }

    suspend fun unpair(session: Session) = change.withLock {
        // Use the old credential only for this bounded, best-effort request. A new pairing never
        // reads it from disk or attaches to this cleanup.
        try {
            pushUnregister(session.server, session.token, Cleartext.allowLoopbackHttp)
        } catch (e: FfiException) {
            Log.d("ReminderPush", "could not unregister this pairing: ${e::class.simpleName}")
        }
        if (pairing == session.pairingId) {
            retireFirebase(
                deleteToken = {
                    if (firebase != null) withTimeoutOrNull(2_000L) { FirebaseMessaging.getInstance().deleteToken().await() }
                },
                clear = {
                    try {
                        clearFirebase(session.pairingId)
                    } finally {
                        pairing = null
                        _status.value = PushStatus()
                    }
                },
                failed = { Log.d("ReminderPush", "could not finish Firebase cleanup") },
            )
        } else if (messagePairing() == session.pairingId) {
            prefs.edit().clear().apply()
        }
    }

    private fun clearFirebase(id: String) {
        try {
            firebase?.delete()
        } finally {
            firebase = null
            if (messagePairing() == id) prefs.edit().clear().apply()
        }
    }

    companion object {
        private const val TOKEN_WAIT_MS = 10_000L
    }
}
