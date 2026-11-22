package app.todoornot.ui

import android.app.Activity
import android.content.ActivityNotFoundException
import android.content.Intent
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.*
import androidx.compose.runtime.*
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.semantics.LiveRegionMode
import androidx.compose.ui.semantics.liveRegion
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.dp
import app.todoornot.core.AttachmentRow
import app.todoornot.core.SyncState
import app.todoornot.design.Icon
import app.todoornot.design.LocalDesign
import app.todoornot.design.Tokens
import app.todoornot.model.*

private class Pickers(val add: (String) -> Unit, val save: (String, AttachmentRow) -> Unit)
private val LocalPickers = staticCompositionLocalOf { Pickers({}, { _, _ -> }) }

// The viewer MIME is not a filename hint: text/plain would cause DocumentsUI to append .txt to .md.
internal fun saveAttachmentIntent(name: String): Intent = Intent(Intent.ACTION_CREATE_DOCUMENT)
    .addCategory(Intent.CATEGORY_OPENABLE).setType("application/octet-stream").putExtra(Intent.EXTRA_TITLE, name)

/** Register once above both pane layouts. A result belongs to the original pairing, task and file,
 * not to the detail (if any) on screen after the picker or a configuration change. */
@Composable
internal fun AttachmentPickers(actions: Actions?, pairing: String?, discard: (String) -> Unit, content: @Composable () -> Unit) {
    var source by rememberSaveable { mutableStateOf<Array<String>?>(null) }
    var destination by rememberSaveable { mutableStateOf<Array<String>?>(null) }
    val pick = rememberLauncherForActivityResult(ActivityResultContracts.OpenDocument()) { uri ->
        val pending = source
        source = null
        if (uri != null && pending != null && pairing == pending[0]) actions?.transfer(FileAction.Upload(pending[1], uri.toString()))
    }
    val create = rememberLauncherForActivityResult(ActivityResultContracts.StartActivityForResult()) { result ->
        val pending = destination
        destination = null
        val uri = result.data?.data?.toString()
        if (uri != null && result.resultCode == Activity.RESULT_OK) {
            if (actions == null || pending == null || pairing != pending[0]) discard(uri)
            else actions.transfer(FileAction.Save(pending[1], pending[2], pending[3], uri))
        }
    }
    val pickers = Pickers(add = { task ->
        if (pairing != null) {
            source = arrayOf(pairing, task)
            try { pick.launch(arrayOf("*/*")) } catch (_: ActivityNotFoundException) {
                source = null; actions?.attachmentNotice(task, "Couldn’t open the file picker.")
            }
        }
    }, save = { task, row ->
        if (pairing != null) {
            destination = arrayOf(pairing, task, row.id, row.name)
            try { create.launch(saveAttachmentIntent(row.name)) }
            catch (_: ActivityNotFoundException) { destination = null; actions?.attachmentNotice(task, "Couldn’t save ${row.name}.") }
        }
    })
    CompositionLocalProvider(LocalPickers provides pickers, content = content)
}

@Composable
internal fun Attachments(state: UiState, actions: Actions) {
    val detail = state.detail ?: return
    val transfer = state.attachment
    val offline = state.status.state == SyncState.OFFLINE
    val available = !offline && !state.signedOut && transfer?.busy != true
    val canAdd = available && state.attachmentTaskConfirmed
    val pickers = LocalPickers.current
    val r = LocalDesign.current.room
    val space = Tokens.Space
    Caps("Attachments")
    for (row in detail.attachments) {
        Row(Modifier.fillMaxWidth().hairline(r.line), verticalAlignment = Alignment.CenterVertically) {
            Icon(row.icon, r.ink3)
            Column(Modifier.weight(1f).padding(horizontal = space.S2.dp)) {
                Label(row.name, Tokens.Type.NAV, r.ink)
                Label(row.size, Tokens.Type.COUNT, r.ink3)
            }
            if (row.openType != null) IconButton("external-link", "Open “${row.name}”", enabled = available) { actions.transfer(FileAction.Open(detail.id, row)) }
            IconButton("download", "Save “${row.name}”", enabled = available) { pickers.save(detail.id, row) }
            IconButton("trash-2", "Remove “${row.name}”") { actions.organize(OrganizeEdit.RemoveAttachment(row.id)) }
        }
    }
    Row(Modifier.heightIn(min = Tokens.Metrics.Phone.TOUCH.dp).clickable(enabled = canAdd) { pickers.add(detail.id) },
        verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(space.S2.dp)) {
        val ink = if (canAdd) r.accentStrong else r.ink3
        Icon("plus", ink)
        Label("Add file", Tokens.Type.BUTTON_SMALL, ink)
    }
    val text = when {
        offline -> "Connect to upload or download attachments."
        transfer != null -> transfer.text
        !state.attachmentTaskConfirmed -> "Waiting to sync…"
        else -> ""
    }
    if (text.isNotEmpty()) Label(text, Tokens.Type.META_PHONE, r.ink3, Modifier.semantics { liveRegion = LiveRegionMode.Polite }, maxLines = Int.MAX_VALUE)
}
