package app.todoornot.ui

import android.Manifest
import android.content.ClipData
import android.content.ClipboardManager
import android.os.PersistableBundle
import android.os.SystemClock
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.OutlinedTextField
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.setValue
import androidx.compose.runtime.staticCompositionLocalOf
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.dp
import app.todoornot.Cleartext
import app.todoornot.core.DeviceInvitation
import app.todoornot.core.DeviceInviteStatus
import app.todoornot.core.DevicePairingReview
import app.todoornot.core.FfiException
import app.todoornot.core.deviceInviteCancel
import app.todoornot.core.deviceInviteCreate
import app.todoornot.core.deviceInvitePollMs
import app.todoornot.core.deviceInviteStatus
import app.todoornot.core.devicePairingReview
import app.todoornot.core.pairingQr
import app.todoornot.design.Design
import app.todoornot.design.LocalDesign
import app.todoornot.design.Tokens
import app.todoornot.model.PairingModel
import app.todoornot.session.Session
import com.journeyapps.barcodescanner.ScanContract
import com.journeyapps.barcodescanner.ScanOptions
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import kotlin.math.floor

/** Present only while the paired session is ready. Never persisted in Compose state. */
val LocalPairingOwner = staticCompositionLocalOf<Session?> { null }

/** Decode locally; permission is requested only by the visible scan control. */
@Composable
internal fun rememberPairingScanner(onRead: (String) -> Unit, onDenied: () -> Unit): () -> Unit {
    val read by rememberUpdatedState(onRead)
    val denied by rememberUpdatedState(onDenied)
    val scanner = rememberLauncherForActivityResult(ScanContract()) { result -> result.contents?.let(read) }
    val request = rememberLauncherForActivityResult(ActivityResultContracts.RequestPermission()) { granted ->
        if (granted) {
            scanner.launch(ScanOptions().setDesiredBarcodeFormats(ScanOptions.QR_CODE).setBeepEnabled(false).setOrientationLocked(false))
        } else {
            denied()
        }
    }
    return { request.launch(Manifest.permission.CAMERA) }
}

@Composable
internal fun PairingQrTile(link: String, label: String) {
    val qr = remember(link) { pairingQr(link) } ?: return
    val colors = remember { Design(false, "my-day").room }
    Canvas(Modifier.size(Tokens.Metrics.Component.PAIRING_QR.dp).background(colors.pane).semantics { contentDescription = label }) {
        val width = qr.width.toInt()
        val module = floor(size.minDimension / width)
        val origin = floor((size.minDimension - module * width) / 2)
        qr.dark.forEachIndexed { index, dark ->
            if (dark) drawRect(colors.ink, Offset(origin + module * (index % width), origin + module * (index / width)), Size(module, module))
        }
    }
}

internal fun pairingRemaining(expires: Long, now: Long): Long = (expires - now).coerceAtLeast(0).let { (it + 999) / 1000 }

internal fun pairTime(left: Long): String = "Expires in ${left / 60}:${(left % 60).toString().padStart(2, '0')}"

@Composable
internal fun DeviceInviteContent(invite: DeviceInvitation, left: Long, busy: Boolean, cancel: () -> Unit) {
    val r = LocalDesign.current.room
    val context = LocalContext.current
    Label("Scan on the new device, type the code in its app, or open the link in its browser.", Tokens.Type.BODY, r.ink2, maxLines = 3)
    PairingQrTile(invite.link, "Invite QR code")
    Label(invite.code, Tokens.Type.DUE, r.ink)
    Label(invite.link, Tokens.Type.META_PHONE, r.ink2, maxLines = 4)
    Label(pairTime(left), Tokens.Type.DUE, r.ink3)
    Row(horizontalArrangement = Arrangement.spacedBy(Tokens.Space.S2.dp)) {
        Button("Copy link", primary = false, modifier = Modifier.weight(1f)) {
            val clip = ClipData.newPlainText("Device invitation", invite.link)
            clip.description.extras = PersistableBundle().apply { putBoolean("android.content.extra.IS_SENSITIVE", true) }
            context.getSystemService(ClipboardManager::class.java)?.setPrimaryClip(clip)
        }
        Button("Cancel invite", primary = false, enabled = !busy, modifier = Modifier.weight(1f), onClick = cancel)
    }
}

@Composable
internal fun DeviceReviewContent(name: String, busy: Boolean, approve: () -> Unit, back: () -> Unit) {
    Label("Approve “$name”? Only continue if the code matches your own device.", Tokens.Type.BODY, LocalDesign.current.room.ink2, maxLines = 4)
    Button("Approve", primary = true, enabled = !busy, onClick = approve)
    Button("Use another code", primary = false, enabled = !busy, onClick = back)
}

@Composable
internal fun DevicePairingDialog(session: Session, inviting: Boolean, onClose: () -> Unit) {
    val r = LocalDesign.current.room
    val scope = rememberCoroutineScope()
    val title = if (inviting) "Pair a new device" else "Approve a device"
    val allowHttp = Cleartext.allowLoopbackHttp
    var invite by remember { mutableStateOf<DeviceInvitation?>(null) }
    var expires by remember { mutableStateOf(0L) }
    var left by remember { mutableStateOf(0L) }
    var answer by remember { mutableStateOf<DevicePairingReview?>(null) }
    var code by remember { mutableStateOf("") }
    var reviewedCode by remember { mutableStateOf("") }
    var busy by remember { mutableStateOf(false) }
    var error by remember { mutableStateOf<String?>(null) }
    var status by remember { mutableStateOf<String?>(null) }

    fun run(operation: suspend () -> Unit) {
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
            } catch (e: FfiException) {
                error = PairingModel.describe(e)
            } finally {
                busy = false
            }
        }
    }

    val current = invite
    LaunchedEffect(current?.id) {
        if (current == null) return@LaunchedEffect
        val pollMs = deviceInvitePollMs().toLong()
        var nextPoll = SystemClock.elapsedRealtime() + pollMs
        while (true) {
            delay(Tokens.Motion.PAIRING_TICK.toLong())
            val now = SystemClock.elapsedRealtime()
            left = pairingRemaining(expires, now)
            if (left == 0L) {
                status = "Invite expired. Make a new one if needed."
                invite = null
                break
            }
            if (now < nextPoll) continue
            nextPoll = now + pollMs
            val result = try {
                withContext(Dispatchers.IO) { deviceInviteStatus(session.server, session.token, current.id, allowHttp) }
            } catch (_: FfiException.Expired) {
                invite = null
                status = "The invite is no longer available."
                break
            } catch (_: FfiException) {
                continue
            }
            when (result) {
                is DeviceInviteStatus.Used -> {
                    invite = null
                    status = "${result.deviceName} signed in."
                    break
                }
                DeviceInviteStatus.Cancelled -> {
                    invite = null
                    status = "Invite cancelled."
                    break
                }
                DeviceInviteStatus.Pending -> Unit
            }
        }
    }

    fun review(input: String, previous: DevicePairingReview?) = run {
        val reviewed = withContext(Dispatchers.IO) { devicePairingReview(session.server, session.token, input, previous?.createdAt, allowHttp) }
        if (previous != null) {
            status = "Approved ${reviewed.deviceName}."
            code = ""
            answer = null
        } else {
            reviewedCode = input
            answer = reviewed
        }
    }

    OverlayDialog(onClose) { overlay ->
        DialogCard(title, Modifier.fillMaxWidth()) {
            Column(Modifier.verticalScroll(rememberScrollState()), verticalArrangement = Arrangement.spacedBy(Tokens.Space.S3.dp)) {
                Label(title, Tokens.Type.DETAIL_TITLE, r.ink)
                if (inviting) {
                    if (current == null) {
                        Button("Create invite", primary = true, enabled = !busy) {
                            run {
                                val issued = withContext(Dispatchers.IO) { deviceInviteCreate(session.server, session.token, allowHttp) }
                                left = issued.expiresIn.toLong()
                                expires = SystemClock.elapsedRealtime() + left * 1000
                                invite = issued
                            }
                        }
                    } else {
                        DeviceInviteContent(current, left, busy) {
                            run {
                                withContext(Dispatchers.IO) { deviceInviteCancel(session.server, session.token, current.id, allowHttp) }
                                invite = null
                                status = "Invite cancelled."
                            }
                        }
                    }
                } else {
                    val pending = answer
                    if (pending == null) {
                        val scan = rememberPairingScanner({ review(it, null) }, { error = "Camera access was not granted. You can type the code instead." })
                        OutlinedTextField(code, { code = it }, label = { Label("Code shown on the device", Tokens.Type.META_PHONE, r.ink2) }, singleLine = true, modifier = Modifier.fillMaxWidth())
                        Button("Review", primary = true, enabled = code.isNotBlank() && !busy) { review(code, null) }
                        Button("Scan QR code", primary = false, enabled = !busy, onClick = scan)
                    } else {
                        DeviceReviewContent(pending.deviceName, busy, approve = { review(reviewedCode, pending) }, back = { answer = null })
                    }
                }
                error?.let { Label(it, Tokens.Type.META_PHONE, r.dangerInk) }
                status?.let { Label(it, Tokens.Type.BODY, r.ink) }
                Button("Close", primary = false, onClick = overlay::close)
            }
        }
    }
}
