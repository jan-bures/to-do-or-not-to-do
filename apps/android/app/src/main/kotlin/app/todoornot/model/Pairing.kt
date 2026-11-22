package app.todoornot.model

import app.todoornot.core.DeviceInviteTarget
import app.todoornot.core.FfiException
import app.todoornot.core.PairPoll
import app.todoornot.core.PairStarted
import app.todoornot.core.deviceInviteRedeem
import app.todoornot.core.deviceInviteTarget
import app.todoornot.core.normalizeServerAddress
import app.todoornot.core.pairPoll
import app.todoornot.core.pairStart
import app.todoornot.session.Session
import app.todoornot.session.SessionPersistenceException
import kotlinx.coroutines.CoroutineDispatcher
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.CoroutineStart
import kotlinx.coroutines.Job
import kotlinx.coroutines.NonCancellable
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext

/** The pairing endpoints, behind an interface so the state machine can be tested without a server. */
interface PairingApi {
    /** The canonical server address, or [FfiException.BadAddress]. */
    fun normalize(input: String): String

    fun start(server: String, deviceName: String): PairStarted

    fun poll(server: String, pollToken: String): PairPoll

    fun inviteTarget(address: String, invite: String): DeviceInviteTarget = DeviceInviteTarget(normalize(address), null)

    suspend fun redeem(server: String, invite: String, deviceName: String): Session =
        throw FfiException.Invalid("Invites are unavailable.")
}

/** The real endpoints, in the Rust core. Blocking: call off the main thread. */
/** Pairing through the Rust core. [allowLoopbackHttp] is true only in a debug build. */
class CorePairing(private val allowLoopbackHttp: Boolean) : PairingApi {
    override fun normalize(input: String): String = normalizeServerAddress(input, allowLoopbackHttp)

    override fun start(server: String, deviceName: String): PairStarted = pairStart(server, deviceName, allowLoopbackHttp)

    override fun poll(server: String, pollToken: String): PairPoll = pairPoll(server, pollToken, allowLoopbackHttp)

    override fun inviteTarget(address: String, invite: String): DeviceInviteTarget = deviceInviteTarget(address, invite, allowLoopbackHttp)

    override suspend fun redeem(server: String, invite: String, deviceName: String): Session =
        deviceInviteRedeem(server, invite, deviceName, allowLoopbackHttp).let { Session(it.server, it.deviceId, it.token) }
}

sealed interface PairingState {
    /** Asking for the server address. `error` explains why the last attempt didn't work. */
    data class Entering(val address: String = "", val error: String? = null) : PairingState

    /** Asking the server for a code. */
    data class Starting(val server: String) : PairingState

    /** A link supplied the address; show its host before making any request. */
    data class ConfirmInvite(val host: String) : PairingState

    /** The one-use invite may already be spent; don't offer Cancel. */
    data class Finishing(val server: String) : PairingState

    /** The credential stays in memory; Try again only retries the save. */
    data object SaveFailed : PairingState

    /** Showing the code and link until the owner approves on another device. */
    data class Waiting(
        val server: String,
        val userCode: String,
        val approvalUrl: String,
        val expiresAtMs: Long,
        /** The last poll failed to reach the server; still trying. */
        val retrying: Boolean = false,
    ) : PairingState
}

/**
 * Pairing: the owner types the server address, the app asks the server for a short code, and then polls
 * while the owner approves it at the shown link (with a passkey, on a device that has one). The address
 * is only ever what the owner typed: nothing is compiled in.
 */
class PairingModel(
    private val api: PairingApi,
    private val scope: CoroutineScope,
    private val io: CoroutineDispatcher,
    private val deviceName: String,
    private val onPaired: (Session) -> Unit,
    private val clock: () -> Long = System::currentTimeMillis,
) {
    private val _state = MutableStateFlow<PairingState>(PairingState.Entering())
    val state: StateFlow<PairingState> = _state.asStateFlow()

    private var job: Job? = null
    private class Invite(val address: String, val server: String, val input: String)
    private var awaitingConfirmation: Invite? = null
    private var unsavedSession: Session? = null

    fun submit(address: String) {
        if (_state.value !is PairingState.Entering) return
        job?.cancel()
        job = scope.launch {
            val server = try {
                api.normalize(address)
            } catch (e: FfiException) {
                _state.value = PairingState.Entering(address, describe(e))
                return@launch
            }
            _state.value = PairingState.Starting(server)
            val started = try {
                withContext(io) { api.start(server, deviceName) }
            } catch (e: FfiException) {
                _state.value = PairingState.Entering(address, describe(e))
                return@launch
            }
            val expiresAt = clock() + started.expiresInSecs.toLong() * 1000
            _state.value = PairingState.Waiting(server, started.userCode, started.approvalUrl, expiresAt)
            follow(address, server, started, expiresAt)
        }
    }

    fun redeem(address: String, invite: String) {
        if (_state.value !is PairingState.Entering) return
        val target = try {
            api.inviteTarget(address, invite)
        } catch (e: FfiException) {
            _state.value = PairingState.Entering(address, describe(e))
            return
        }
        val plan = Invite(address, target.server, invite)
        val host = target.confirmHost
        if (host != null) {
            awaitingConfirmation = plan
            _state.value = PairingState.ConfirmInvite(host)
        } else {
            finishInvite(plan)
        }
    }

    fun confirmInvite() {
        val plan = awaitingConfirmation ?: return
        awaitingConfirmation = null
        finishInvite(plan)
    }

    fun cameraDenied(address: String) {
        if (_state.value is PairingState.Entering) {
            _state.value = PairingState.Entering(address, "Camera access was not granted. You can type the code instead.")
        }
    }

    private fun finishInvite(invite: Invite) {
        job?.cancel()
        _state.value = PairingState.Finishing(invite.server)
        job = scope.launch(start = CoroutineStart.UNDISPATCHED) {
            // Keep the network call and the durable save in the same non-cancellable region.
            // Wrapping just the IO call would still allow cancellation to discard its reply.
            withContext(NonCancellable) {
                try {
                    save(withContext(io) { api.redeem(invite.server, invite.input, deviceName) })
                } catch (e: FfiException.Expired) {
                    _state.value = PairingState.Entering(invite.address, "This invite is invalid, expired or already used.")
                } catch (e: FfiException) {
                    _state.value = PairingState.Entering(invite.address, describe(e))
                }
            }
        }
    }

    private fun save(session: Session) {
        try {
            onPaired(session)
            unsavedSession = null
            _state.value = PairingState.Entering(session.server)
        } catch (_: SessionPersistenceException) {
            unsavedSession = session
            _state.value = PairingState.SaveFailed
        }
    }

    fun retrySave() {
        if (_state.value != PairingState.SaveFailed) return
        val session = unsavedSession ?: return
        _state.value = PairingState.Finishing(session.server)
        save(session)
    }

    fun discardSave() {
        if (_state.value != PairingState.SaveFailed) return
        val server = unsavedSession?.server.orEmpty()
        unsavedSession = null
        _state.value = PairingState.Entering(server, DISCARDED)
    }

    private suspend fun follow(address: String, server: String, started: PairStarted, expiresAt: Long) {
        var interval = started.intervalSecs.toLong().coerceAtLeast(1) * 1000
        while (true) {
            delay(interval)
            if (clock() >= expiresAt) {
                _state.value = PairingState.Entering(address, EXPIRED)
                return
            }
            val answer = try {
                withContext(io) { api.poll(server, started.pollToken) }
            } catch (e: FfiException.Network) {
                markRetrying(true)
                continue
            } catch (e: FfiException.Refused) {
                if (e.status.toInt() == TOO_MANY) {
                    interval = maxOf(interval, (e.retryAfterSecs?.toLong() ?: 5) * 1000)
                    continue
                }
                _state.value = PairingState.Entering(address, describe(e))
                return
            } catch (e: FfiException) {
                _state.value = PairingState.Entering(address, describe(e))
                return
            }
            when (answer) {
                is PairPoll.Pending -> {
                    interval = answer.intervalSecs.toLong().coerceAtLeast(1) * 1000
                    markRetrying(false)
                }
                is PairPoll.Approved -> {
                    save(Session(server, answer.deviceId, answer.token))
                    return
                }
                PairPoll.Denied -> {
                    _state.value = PairingState.Entering(address, DENIED)
                    return
                }
            }
        }
    }

    private fun markRetrying(retrying: Boolean) {
        val s = _state.value
        if (s is PairingState.Waiting && s.retrying != retrying) _state.value = s.copy(retrying = retrying)
    }

    /** Back to the address field, keeping what was typed. */
    fun cancel() {
        val address = when (val s = _state.value) {
            is PairingState.Finishing, PairingState.SaveFailed -> return
            is PairingState.ConfirmInvite -> awaitingConfirmation?.address.orEmpty()
            is PairingState.Entering -> s.address
            is PairingState.Starting -> s.server
            is PairingState.Waiting -> s.server
        }
        awaitingConfirmation = null
        job?.cancel()
        _state.value = PairingState.Entering(address)
    }

    companion object {
        const val EXPIRED = "The code expired. Start again for a new one."
        const val DENIED = "The pairing was denied."
        const val DISCARDED = "Check Devices on another signed-in device. This phone may already appear there."
        private const val TOO_MANY = 429

        fun describe(e: FfiException): String = when (e) {
            is FfiException.BadAddress -> e.reason
            is FfiException.Network -> "Couldn't reach the server. Check the address and your connection. (${e.detail})"
            is FfiException.Expired -> EXPIRED
            is FfiException.Refused -> "The server refused (${e.status}): ${e.detail}"
            is FfiException.Protocol -> "That doesn't look like a to-do server: ${e.detail}"
            is FfiException.Storage -> "Couldn't save on this phone: ${e.detail}"
            is FfiException.Invalid -> e.detail
        }
    }
}
