package app.todoornot.ui

import android.content.ClipData
import android.content.ClipboardManager
import android.os.PersistableBundle
import android.view.Window
import android.view.WindowManager.LayoutParams.FLAG_SECURE
import androidx.activity.compose.LocalActivity
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
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.remember
import androidx.compose.runtime.staticCompositionLocalOf
import androidx.compose.ui.Modifier
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.input.key.Key
import androidx.compose.ui.input.key.KeyEventType
import androidx.compose.ui.input.key.key
import androidx.compose.ui.input.key.onPreviewKeyEvent
import androidx.compose.ui.input.key.type
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.dp
import androidx.compose.ui.window.SecureFlagPolicy
import app.todoornot.core.DeviceInvitation
import app.todoornot.core.pairingQr
import app.todoornot.design.Design
import app.todoornot.design.LocalDesign
import app.todoornot.design.Tokens
import app.todoornot.model.DevicePairingModel
import kotlin.math.floor

/** Present only while the paired session is ready. Never persisted in Compose state. */
internal val LocalPairingOwner = staticCompositionLocalOf<DevicePairingModel?> { null }
internal val LocalScanPairing = staticCompositionLocalOf<(DevicePairingModel) -> Unit> { {} }

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
internal fun DeviceReviewContent(name: String, code: String, source: String?, busy: Boolean, approve: () -> Unit, back: () -> Unit) {
    val r = LocalDesign.current.room
    Label("Approve “$name”? Only continue if the code matches your own device.", Tokens.Type.BODY, r.ink2, maxLines = 4)
    Label(code, Tokens.Type.DUE, r.ink)
    Label("Source address: ${source ?: "Unknown"}", Tokens.Type.BODY, r.ink2, maxLines = 2)
    Button("Approve", primary = true, enabled = !busy, onClick = approve)
    Button("Use another code", primary = false, enabled = !busy, onClick = back)
}

/** Guard the Activity as well as the dialog: task snapshots can use the Activity's window. */
internal fun protectInviteWindow(window: Window): () -> Unit {
    val alreadySecure = window.attributes.flags and FLAG_SECURE != 0
    window.addFlags(FLAG_SECURE)
    return { if (!alreadySecure) window.clearFlags(FLAG_SECURE) }
}

internal enum class DevicePairingPage(val title: String) {
    SETTINGS("Device settings"),
    INVITE("Pair a new device"),
    APPROVE("Approve a device"),
}

@Composable
internal fun DevicePairingDialog(
    model: DevicePairingModel,
    page: DevicePairingPage,
    onPage: (DevicePairingPage) -> Unit,
    onClose: () -> Unit,
) {
    val r = LocalDesign.current.room
    val scan = LocalScanPairing.current
    val current = if (page == DevicePairingPage.INVITE) model.invite else null
    val activityWindow = LocalActivity.current?.window
    if (current != null && activityWindow != null) {
        DisposableEffect(activityWindow) {
            val restore = protectInviteWindow(activityWindow)
            onDispose(restore)
        }
    }
    OverlayDialog(onClose, if (current != null) SecureFlagPolicy.SecureOn else SecureFlagPolicy.Inherit) { overlay ->
        val first = remember { FocusRequester() }
        val close = remember { FocusRequester() }
        LaunchedEffect(page) { (if (page == DevicePairingPage.SETTINGS) first else close).requestFocus() }
        DialogCard(page.title, Modifier.fillMaxWidth().onPreviewKeyEvent { event ->
            if (event.key != Key.Escape) false else {
                if (event.type == KeyEventType.KeyUp) overlay.close()
                true
            }
        }) {
            Column(Modifier.verticalScroll(rememberScrollState()), verticalArrangement = Arrangement.spacedBy(Tokens.Space.S3.dp)) {
                Label(page.title, Tokens.Type.DETAIL_TITLE, r.ink)
                if (page == DevicePairingPage.SETTINGS) {
                    Button(DevicePairingPage.INVITE.title, primary = false, modifier = Modifier.focusRequester(first)) {
                        onPage(DevicePairingPage.INVITE)
                    }
                    Button(DevicePairingPage.APPROVE.title, primary = false) { onPage(DevicePairingPage.APPROVE) }
                } else if (page == DevicePairingPage.INVITE) {
                    when {
                        current != null -> DeviceInviteContent(current, model.left, model.busy, model::cancelInvite)
                        model.checking -> Label("Checking whether the device joined…", Tokens.Type.BODY, r.ink)
                        else -> Button("Create invite", primary = true, enabled = !model.busy, onClick = model::create)
                    }
                } else {
                    val pending = model.answer
                    if (pending == null) {
                        OutlinedTextField(model.code, { model.code = it }, label = { Label("Code shown on the device", Tokens.Type.META_PHONE, r.ink2) }, singleLine = true, modifier = Modifier.fillMaxWidth())
                        Button("Review", primary = true, enabled = model.code.isNotBlank() && !model.busy) { model.review(model.code) }
                        Button("Scan QR code", primary = false, enabled = !model.busy) { scan(model) }
                    } else {
                        DeviceReviewContent(pending.deviceName, pending.code, pending.source, model.busy, approve = model::approve, back = model::anotherCode)
                    }
                }
                if (page != DevicePairingPage.SETTINGS) {
                    model.error?.let { Label(it, Tokens.Type.META_PHONE, r.dangerInk) }
                    model.status?.let { Label(it, Tokens.Type.BODY, r.ink) }
                }
                Button("Close", primary = false, modifier = Modifier.focusRequester(close), onClick = overlay::close)
            }
        }
    }
}
