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
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.unit.dp
import app.todoornot.design.Landscape
import app.todoornot.design.LocalDesign
import app.todoornot.design.Scene
import app.todoornot.design.TodoTheme
import app.todoornot.design.Tokens
import app.todoornot.design.style
import app.todoornot.model.PairingState

private val S = Tokens.Space
private val P = Tokens.Metrics.Phone

/** What the pairing screen can ask for. */
interface PairingActions {
    fun submit(address: String)
    fun cancel()
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
fun PairingScreen(state: PairingState, actions: PairingActions, dark: Boolean) {
    val waiting = state is PairingState.Waiting
    Welcome("Welcome", "Pair this phone", Scene(if (waiting) 1 else 0, 2, finished = false), dark) {
        when (state) {
            is PairingState.Entering -> Entering(state, actions)
            is PairingState.Starting -> {
                Body("Asking ${state.server} for a code…")
                Button("Cancel", primary = false, onClick = actions::cancel)
            }
            is PairingState.Waiting -> Waiting(state, actions)
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
        Button("Unpair this phone", primary = false) { confirm = true }
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
                    Label(eyebrow, Tokens.Type.EYEBROW_DAY_PHONE, r.ink2)
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
}

@Composable
private fun Waiting(state: PairingState.Waiting, actions: PairingActions) {
    val r = LocalDesign.current.room
    Body("Open this link on a device with your passkey and approve the code. This screen moves on by itself.")
    Column(
        Modifier
            .fillMaxWidth()
            .clip(RoundedCornerShape(Tokens.Radius.XL.dp))
            .background(r.raised)
            .border(Tokens.Elevation.HAIRLINE.dp, r.lineStrong, RoundedCornerShape(Tokens.Radius.XL.dp))
            .padding(S.S5.dp),
        verticalArrangement = Arrangement.spacedBy(S.S2.dp),
    ) {
        Label("Code", Tokens.Type.CAPS_LABEL, r.ink3)
        Label(state.userCode, Tokens.Type.ROOM_TITLE_PHONE.copy(role = app.todoornot.design.FontRole.MONO, weight = Tokens.Type.DUE.weight), r.ink)
        Spacer(Modifier.height(S.S1.dp))
        Label("Link", Tokens.Type.CAPS_LABEL, r.ink3)
        Label(state.approvalUrl, Tokens.Type.DUE, r.accentStrong, maxLines = 3)
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
    val bg = when {
        primary -> r.accentFill
        danger -> r.dangerBg
        else -> r.raised
    }
    val ink = when {
        primary -> r.onAccent
        danger -> r.dangerInk
        else -> r.ink
    }
    Box(
        modifier
            .fillMaxWidth()
            .height(P.TOUCH.dp)
            .clip(shape)
            .background(if (enabled) bg else bg.copy(alpha = 0.5f))
            .then(if (primary) Modifier else Modifier.border(Tokens.Elevation.HAIRLINE.dp, r.lineStrong, shape))
            .clickable(enabled = enabled, onClick = onClick),
        contentAlignment = Alignment.Center,
    ) {
        Label(text, Tokens.Type.BUTTON, if (enabled) ink else ink.copy(alpha = 0.7f))
    }
}
