package app.todoornot.model

import android.os.SystemClock
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import app.todoornot.core.DeviceInvitation
import app.todoornot.core.DeviceInviteStatus
import app.todoornot.core.DevicePairingReview
import app.todoornot.core.FfiException
import app.todoornot.core.deviceInviteCancel
import app.todoornot.core.deviceInviteCreate
import app.todoornot.core.deviceInvitePollMs
import app.todoornot.core.deviceInviteStatus
import app.todoornot.core.devicePairingReview
import app.todoornot.design.Tokens
import app.todoornot.session.Session
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext

internal interface DevicePairingApi {
    val pollMs: Long
    suspend fun create(): DeviceInvitation
    suspend fun status(id: String): DeviceInviteStatus
    suspend fun cancel(id: String)
    suspend fun review(code: String, approveAt: Long?): DevicePairingReview
}

internal class CoreDevicePairing(private val session: Session, private val allowHttp: Boolean) : DevicePairingApi {
    override val pollMs get() = deviceInvitePollMs().toLong()
    override suspend fun create() = withContext(Dispatchers.IO) { deviceInviteCreate(session.server, session.token, allowHttp) }
    override suspend fun status(id: String) = withContext(Dispatchers.IO) { deviceInviteStatus(session.server, session.token, id, allowHttp) }
    override suspend fun cancel(id: String) = withContext(Dispatchers.IO) { deviceInviteCancel(session.server, session.token, id, allowHttp) }
    override suspend fun review(code: String, approveAt: Long?) = withContext(Dispatchers.IO) {
        devicePairingReview(session.server, session.token, code, approveAt, allowHttp)
    }
}

/** Bound to an open session, not an Activity or its saved state. Never serialize the invite or scan. */
internal class DevicePairingModel(
    private val api: DevicePairingApi,
    private val scope: CoroutineScope,
    private val clock: () -> Long = SystemClock::elapsedRealtime,
) {
    private var active: DeviceInvitation? by mutableStateOf(null)
    val invite get() = active?.takeIf { left > 0 }
    val checking get() = active != null && left == 0L
    var left by mutableStateOf(0L)
        private set
    var answer: DevicePairingReview? by mutableStateOf(null)
        private set
    var code by mutableStateOf("")
    var busy by mutableStateOf(false)
        private set
    var error: String? by mutableStateOf(null)
        private set
    var status: String? by mutableStateOf(null)
        private set

    private fun run(operation: suspend () -> Unit) {
        if (busy) return
        busy = true
        error = null
        status = null
        scope.launch {
            try {
                operation()
            } catch (e: FfiException.Expired) {
                error = "This code is invalid, expired or already used."
                answer = null
            } catch (e: FfiException.Refused) {
                error = if (e.code == "too_many_codes") "Too many wrong codes. Wait five minutes and try again." else PairingModel.describe(e)
            } catch (e: FfiException) {
                error = PairingModel.describe(e)
            } finally {
                busy = false
            }
        }
    }

    fun create() {
        if (active != null) return
        run {
            val issued = api.create()
            left = issued.expiresIn.toLong()
            val deadline = clock() + left * 1_000
            active = issued
            countdown(issued.id, deadline)
            follow(issued.id, deadline)
        }
    }

    private fun countdown(id: String, deadline: Long) = scope.launch {
        while (active?.id == id) {
            val remaining = (deadline - clock()).coerceAtLeast(0)
            left = (remaining + 999) / 1_000
            if (left == 0L) break
            delay(minOf(Tokens.Motion.PAIRING_TICK.toLong(), remaining))
        }
    }

    private fun follow(id: String, deadline: Long) = scope.launch {
        while (active?.id == id) {
            val remaining = deadline - clock()
            delay(if (remaining > 0) minOf(api.pollMs, remaining) else api.pollMs)
            if (active?.id != id) break
            val result = try {
                api.status(id)
            } catch (_: FfiException.Expired) {
                finish(id, if (clock() >= deadline) "Invite expired. Make a new one if needed." else "The invite is no longer available.")
                break
            } catch (_: FfiException) {
                null
            }
            when (result) {
                is DeviceInviteStatus.Used -> finish(id, "${result.deviceName} signed in.")
                DeviceInviteStatus.Cancelled -> finish(id, "Invite cancelled.")
                else -> if (clock() >= deadline + STATUS_GRACE_MS) finish(id, "Couldn’t confirm whether the device joined. Check Devices.")
            }
        }
    }

    private fun finish(id: String, message: String) {
        if (active?.id != id) return
        active = null
        status = message
    }

    fun cancelInvite() {
        val id = invite?.id ?: return
        run {
            try {
                api.cancel(id)
                finish(id, "Invite cancelled.")
            } catch (_: FfiException.Expired) {
                // The status check may still report a join.
            }
        }
    }

    fun review(input: String) = run { answer = api.review(input, null) }

    fun approve() {
        val pending = answer ?: return
        run {
            val reviewed = api.review(pending.code, pending.createdAt)
            status = "Approved ${reviewed.deviceName}."
            code = ""
            answer = null
        }
    }

    fun anotherCode() {
        answer = null
        code = ""
    }

    fun cameraDenied() { error = "Camera access was not granted. You can type the code instead." }

    private companion object {
        const val STATUS_GRACE_MS = 60_000L
    }
}

/** One in-memory scanner request, delivered by whichever Activity currently owns the result. */
internal class PairingScan {
    private class Pending(val read: (String) -> Unit, val denied: () -> Unit)
    private var pending: Pending? = null

    fun begin(read: (String) -> Unit, denied: () -> Unit): Boolean {
        if (pending != null) return false
        pending = Pending(read, denied)
        return true
    }

    fun permission(granted: Boolean): Boolean {
        val request = pending ?: return false
        if (!granted) {
            pending = null
            request.denied()
        }
        return granted
    }

    fun result(contents: String?) {
        val request = pending
        pending = null
        if (contents != null) request?.read?.invoke(contents)
    }
}
