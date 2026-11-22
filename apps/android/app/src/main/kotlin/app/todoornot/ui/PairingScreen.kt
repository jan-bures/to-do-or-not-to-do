package app.todoornot.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.remember
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.unit.dp
import app.todoornot.design.Landscape
import app.todoornot.design.LocalDesign
import app.todoornot.design.RoomColors
import app.todoornot.design.Scene
import app.todoornot.design.TodoTheme
import app.todoornot.design.Tokens
import app.todoornot.design.style
import app.todoornot.model.PairingState
import kotlinx.coroutines.delay

private val S = Tokens.Space
private val P = Tokens.Metrics.Phone

/** What the pairing screen can ask for. */
interface PairingActions {
    fun submit(address: String)
    fun cancel()
    fun redeem(address: String, invite: String) = Unit
    fun confirmInvite() = Unit
    fun retrySave() = Unit
    fun discardSave() = Unit
    fun scanInvite(address: String) = Unit
    fun openLink(url: String)
    fun copyLink(url: String)

    object None : PairingActions {
        override fun submit(address: String) = Unit
        override fun cancel() = Unit
        override fun openLink(url: String) = Unit
        override fun copyLink(url: String) = Unit
    }
}

/**
 * Pairing, in two steps: the server address, then a code to approve on a device with the owner's
 * passkey. The example address is a placeholder; nothing is filled in.
 */
@Composable
fun PairingScreen(state: PairingState, actions: PairingActions, dark: Boolean, fixedRemainingSeconds: Long? = null) {
    // No tasks yet, so nothing to measure: the hills only, no sun (`docs/design-language.md`).
    Welcome("Welcome", "Pair this phone", Scene(0, 0, finished = false), dark) {
        when (state) {
            is PairingState.Entering -> Entering(state, actions)
            is PairingState.Starting -> {
                Body("Contacting ${state.server}…")
                Button("Cancel", primary = false, onClick = actions::cancel)
            }
            is PairingState.ConfirmInvite -> {
                Body("This invite is for ${state.host}. Only continue if you trust this server.")
                Button("Pair with ${state.host}", primary = true, onClick = actions::confirmInvite)
                Button("Go back", primary = false, onClick = actions::cancel)
            }
            is PairingState.Finishing -> Body("Finishing pairing with ${state.server}…")
            PairingState.Discarding -> Body("Removing this phone’s sign-in…")
            PairingState.SaveFailed -> {
                Body("This phone joined, but couldn’t save its sign-in. Try saving again without using another code.")
                Button("Try saving again", primary = true, onClick = actions::retrySave)
                Button("Discard and start over", primary = false, onClick = actions::discardSave)
            }
            is PairingState.Waiting -> Waiting(state, actions, fixedRemainingSeconds)
        }
    }
}

/**
 * The paired server is known, but this phone's copy of the tasks couldn't be opened. The pairing is
 * kept: trying again finishes a recovery that couldn't complete. Unpairing is offered, but only as the
 * owner's choice, because it deletes what hasn't been sent.
 */
@Composable
fun CantOpenScreen(
    problem: String,
    server: String,
    dark: Boolean,
    onRetry: () -> Unit,
    onUnpair: () -> Unit,
    unpairProblem: String? = null,
    asking: Boolean = false,
) {
    var confirm by rememberSaveable { mutableStateOf(asking) }
    Welcome("Paired with ${server.removePrefix("https://")}", "Can’t open your tasks", Scene(0, 0, finished = false), dark) {
        val r = LocalDesign.current.room
        Body(problem)
        Body("Try again in a moment. If it keeps failing, free some space or restart the phone first.")
        Button("Try again", primary = true, onClick = onRetry)
        Spacer(Modifier.height(S.S2.dp))
        unpairProblem?.let { Label(it, Tokens.Type.META_PHONE, r.dangerInk, maxLines = 4) }
        Button("Unpair this phone", primary = false, danger = true) { confirm = true }
        // The cache can't be read, so how much is unsent can't be told.
        if (confirm) UnpairDialog(unsent = null, onCancel = { confirm = false }, onConfirm = { confirm = false; onUnpair() })
    }
}

/** The sky header with an eyebrow, a title and a landscape, over a scrolling column of content. */
@Composable
private fun Welcome(eyebrow: String, title: String, scene: Scene, dark: Boolean, content: @Composable () -> Unit) {
    TodoTheme(dark, "my-day") {
        val r = LocalDesign.current.room
        Column(Modifier.fillMaxSize().background(r.pane)) {
            Column(Modifier.fillMaxWidth().background(r.sky).statusBarsPadding()) {
                Column(Modifier.padding(start = S.S5.dp, end = S.S5.dp, top = S.S8.dp, bottom = S.S1.dp)) {
                    Label(eyebrow, Tokens.Type.EYEBROW_DAY_PHONE, r.accentStrong)
                    Label(title, Tokens.Type.ROOM_TITLE_PHONE, r.ink)
                }
                Landscape(scene, Tokens.Landscape.HEIGHT_COVER.dp)
            }
            Box(Modifier.fillMaxWidth().height(Tokens.Elevation.HAIRLINE.dp).background(r.line))
            Column(
                Modifier
                    .weight(1f)
                    .verticalScroll(rememberScrollState())
                    .imePadding()
                    .navigationBarsPadding()
                    .padding(S.S5.dp)
                    .widthIn(max = P.INNER_LIST.dp),
                verticalArrangement = Arrangement.spacedBy(S.S4.dp),
            ) {
                content()
            }
        }
    }
}

@Composable
private fun Entering(state: PairingState.Entering, actions: PairingActions) {
    val r = LocalDesign.current.room
    var address by rememberSaveable(state.address) { mutableStateOf(state.address) }
    var invite by remember { mutableStateOf("") }
    Body("Enter your to-do server’s address. You approve this phone there with your passkey.")
    val shape = RoundedCornerShape(Tokens.Radius.COMPOSER.dp)
    Box(
        Modifier
            .fillMaxWidth()
            .height(P.COMPOSER.dp)
            .clip(shape)
            .background(r.raised)
            .border(Tokens.Elevation.HAIRLINE.dp, if (state.error != null) r.danger else r.control, shape)
            .padding(horizontal = S.S4.dp),
        contentAlignment = Alignment.CenterStart,
    ) {
        BasicTextField(
            value = address,
            onValueChange = { address = it.trim() },
            modifier = Modifier.fillMaxWidth().semantics { contentDescription = "Server address" },
            textStyle = Tokens.Type.COMPOSER.copy(role = app.todoornot.design.FontRole.MONO).style(r.ink),
            singleLine = true,
            cursorBrush = SolidColor(r.accent),
            keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Uri, imeAction = ImeAction.Go, autoCorrectEnabled = false),
            keyboardActions = KeyboardActions(onGo = { actions.submit(address) }),
            decorationBox = { inner ->
                if (address.isEmpty()) Label("https://todo.example.com", Tokens.Type.COMPOSER.copy(role = app.todoornot.design.FontRole.MONO), r.ink4)
                inner()
            },
        )
    }
    state.error?.let { Label(it, Tokens.Type.META_PHONE, r.dangerInk, maxLines = 4) }
    Button("Get a pairing code", primary = true, enabled = address.isNotBlank()) { actions.submit(address) }
    Label("Or use an invite", Tokens.Type.CAPS_LABEL, r.ink3)
    androidx.compose.material3.OutlinedTextField(
        invite, { invite = it }, label = { Label("Invite code or link", Tokens.Type.META_PHONE, r.ink2) },
        singleLine = true, modifier = Modifier.fillMaxWidth(),
    )
    Button("Use invite", primary = false, enabled = invite.isNotBlank()) { actions.redeem(address, invite); invite = "" }
    Button("Scan invite QR code", primary = false) { actions.scanInvite(address) }
}

@Composable
private fun Waiting(state: PairingState.Waiting, actions: PairingActions, fixedRemainingSeconds: Long?) {
    val r = LocalDesign.current.room
    var left by remember(state.expiresAtMs) { mutableStateOf(pairingRemaining(state.expiresAtMs, System.currentTimeMillis())) }
    LaunchedEffect(state.expiresAtMs, fixedRemainingSeconds) {
        while (fixedRemainingSeconds == null && left > 0) {
            delay(Tokens.Motion.PAIRING_TICK.toLong())
            left = pairingRemaining(state.expiresAtMs, System.currentTimeMillis())
        }
    }
    Body("Scan with your signed-in phone or enter this code on a signed-in device. You can also open the approval link in a browser.")
    Column(
        Modifier
            .fillMaxWidth()
            .clip(RoundedCornerShape(Tokens.Radius.XL.dp))
            .background(r.raised)
            .border(Tokens.Elevation.HAIRLINE.dp, r.lineStrong, RoundedCornerShape(Tokens.Radius.XL.dp))
            .padding(S.S5.dp),
        verticalArrangement = Arrangement.spacedBy(S.S2.dp),
    ) {
        PairingQrTile(state.approvalUrl, "Approval QR code")
        Label("Code", Tokens.Type.CAPS_LABEL, r.ink3)
        Label(state.userCode, Tokens.Type.PAIRING_CODE, r.ink)
        Spacer(Modifier.height(S.S1.dp))
        Label("Link", Tokens.Type.CAPS_LABEL, r.ink3)
        Label(state.approvalUrl, Tokens.Type.DUE, r.accentStrong, maxLines = 3)
        Label(pairTime(fixedRemainingSeconds ?: left), Tokens.Type.META_PHONE, r.ink3)
    }
    if (state.retrying) Label("Can’t reach the server right now. Still trying…", Tokens.Type.META_PHONE, r.dangerInk)
    Row(horizontalArrangement = Arrangement.spacedBy(S.S3.dp)) {
        Button("Open link", primary = true, modifier = Modifier.weight(1f)) { actions.openLink(state.approvalUrl) }
        Button("Copy link", primary = false, modifier = Modifier.weight(1f)) { actions.copyLink(state.approvalUrl) }
    }
    Button("Cancel", primary = false, onClick = actions::cancel)
}

@Composable
private fun Body(text: String) {
    val r = LocalDesign.current.room
    Label(text, Tokens.Type.NOTES, r.ink2, maxLines = 6)
}

@Composable
fun Button(
    text: String,
    primary: Boolean,
    modifier: Modifier = Modifier,
    enabled: Boolean = true,
    danger: Boolean = false,
    onClick: () -> Unit,
) {
    val r = LocalDesign.current.room
    val shape = RoundedCornerShape(Tokens.Radius.BUTTON.dp)
    // Disabled, every button is the same quiet one: `wash` with `ink-3` text (guarded to 4.5:1) and a
    // `line-strong` edge, never a faded colour that drops below the text minimum.
    val colors = buttonColors(r, primary, danger, enabled)
    Box(
        modifier
            .fillMaxWidth()
            .height(P.TOUCH.dp)
            .clip(shape)
            .background(colors.bg)
            .then(colors.edge?.let { Modifier.border(Tokens.Elevation.HAIRLINE.dp, it, shape) } ?: Modifier)
            .clickable(enabled = enabled, onClick = onClick),
        contentAlignment = Alignment.Center,
    ) {
        Label(text, Tokens.Type.BUTTON, colors.ink)
    }
}

/** A button's background, text and edge (null: none). */
data class ButtonColors(val bg: Color, val ink: Color, val edge: Color?)

fun buttonColors(r: RoomColors, primary: Boolean, danger: Boolean, enabled: Boolean): ButtonColors = when {
    !enabled -> ButtonColors(r.wash, r.ink3, r.lineStrong)
    primary -> ButtonColors(r.accentFill, r.onAccent, null)
    danger -> ButtonColors(r.dangerBg, r.dangerInk, r.lineStrong)
    else -> ButtonColors(r.raised, r.ink, r.lineStrong)
}
