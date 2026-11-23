package app.todoornot.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.compositionLocalOf
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableLongStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.semantics.ProgressBarRangeInfo
import androidx.compose.ui.semantics.progressBarRangeInfo
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.dp
import app.todoornot.design.LocalDesign
import app.todoornot.design.Tokens
import app.todoornot.updates.UpdateState
import kotlinx.coroutines.delay

internal data class UpdateControls(
    val state: UpdateState,
    val check: () -> Unit = {},
    val download: () -> Unit = {},
    val install: () -> Unit = {},
    val settings: () -> Unit = {},
    val confirm: () -> Unit = {},
    val dismiss: () -> Unit = {},
    val openRequest: Int = 0,
    val opened: () -> Unit = {},
    val clock: () -> Long = System::currentTimeMillis,
)

internal val LocalUpdates = compositionLocalOf<UpdateControls?> { null }
private val S = Tokens.Space
private const val MINUTE_MS = 60_000L

internal fun checkedLabel(now: Long, checked: Long): String {
    if (checked <= 0) return "Not checked yet"
    val minutes = ((now - checked) / MINUTE_MS).coerceAtLeast(0)
    return when {
        minutes < 1 -> "Checked just now"
        minutes < 60 -> "Checked $minutes min ago"
        minutes < 24 * 60 -> "Checked ${minutes / 60} h ago"
        else -> "Checked ${minutes / (24 * 60)} d ago"
    }
}

@Composable
internal fun UpdatesSection(controls: UpdateControls) {
    val state = controls.state
    val r = LocalDesign.current.room
    var now by remember(state.checkedAt) { mutableLongStateOf(controls.clock()) }
    LaunchedEffect(state.checkedAt) {
        if (state.checkedAt > 0) while (true) {
            delay(MINUTE_MS)
            now = controls.clock()
        }
    }
    Column(Modifier.fillMaxWidth().padding(horizontal = S.S4.dp, vertical = S.S2.dp), verticalArrangement = Arrangement.spacedBy(S.S2.dp)) {
        Label("Updates", Tokens.Type.ROOM_TITLE_BAR, r.ink)
        Label("Current version: ${state.currentVersion}", Tokens.Type.NAV_SUB, r.ink2)
        Label(checkedLabel(now, state.checkedAt), Tokens.Type.NAV_SUB, r.ink3)
        val offer = state.offer
        val progress = state.progress
        val message = when {
            state.confirmation != null -> "Android needs you to confirm this update."
            state.installing -> "Preparing the installation…"
            state.permissionNeeded -> "To install updates, allow this app under Install unknown apps in Android settings. Then come back and tap Install update."
            state.checking -> "Checking for updates…"
            progress != null -> "Downloading update · $progress%"
            state.ready -> "Update ready. Save your work before installing."
            offer != null -> "Waiting for an unmetered connection. Download now to use your current connection."
            else -> state.status
        }
        message?.let { Label(it, Tokens.Type.BODY, r.ink2, maxLines = 6) }
        state.problem?.let { Label(it, Tokens.Type.BODY, r.dangerInk, maxLines = 6) }
        if (progress != null) {
            val fraction = progress / 100f
            Box(Modifier.fillMaxWidth().height(Tokens.Metrics.Phone.COMPACT_PROGRESS_HEIGHT.dp).background(r.lineStrong)
                .semantics { progressBarRangeInfo = ProgressBarRangeInfo(fraction, 0f..1f) }) {
                Box(Modifier.fillMaxWidth(fraction).height(Tokens.Metrics.Phone.COMPACT_PROGRESS_HEIGHT.dp).background(r.accentStrong))
            }
        }
        Button("Check now", primary = false, enabled = !state.checking && progress == null && !state.installing, onClick = controls.check)
        when {
            state.confirmation != null -> Button("Continue in Android", primary = true, onClick = controls.confirm)
            state.permissionNeeded -> Button("Open Android settings", primary = false, onClick = controls.settings)
        }
        if (offer != null && !state.installing && !state.checking && progress == null) {
            if (state.ready) Button("Install update (${offer.versionName})", primary = !state.permissionNeeded, onClick = controls.install)
            else Button("Download now", primary = true, onClick = controls.download)
        }
    }
}

@Composable
internal fun UpdatesSheet(controls: UpdateControls, onClose: () -> Unit) {
    BottomSheet("Updates", onClose) { overlay ->
        Column(Modifier.fillMaxWidth().weight(1f, fill = false).verticalScroll(rememberScrollState())) {
            UpdatesSection(controls)
            Box(Modifier.padding(start = S.S4.dp, end = S.S4.dp, bottom = S.S2.dp)) {
                Button("Close", primary = false, onClick = overlay::close)
            }
        }
    }
}

@Composable
internal fun UpdateReadyStrip(controls: UpdateControls, onOpen: () -> Unit, modifier: Modifier = Modifier) {
    val r = LocalDesign.current.room
    val state = controls.state
    if (!state.ready || state.dismissed || state.installing) return
    Row(modifier.fillMaxWidth().background(r.canvas).hairline(r.line, top = true)
        .padding(start = S.S4.dp, end = S.S1.dp), verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(S.S2.dp)) {
        Label("Update ready", Tokens.Type.BODY, r.ink, Modifier.weight(1f))
        IconButton("download", "View update", onClick = onOpen)
        IconButton("x", "Dismiss update indicator", onClick = controls.dismiss)
    }
}
