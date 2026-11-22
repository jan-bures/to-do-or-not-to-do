package app.todoornot.ui

import android.app.Activity
import android.content.Intent
import android.net.Uri
import androidx.activity.compose.LocalActivityResultRegistryOwner
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.*
import androidx.compose.runtime.*
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.semantics.LiveRegionMode
import androidx.compose.ui.semantics.liveRegion
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.dp
import app.todoornot.core.AttachmentRow
import app.todoornot.core.FfiException
import app.todoornot.core.SyncState
import app.todoornot.core.TaskDetail
import app.todoornot.core.attachmentTooLarge
import app.todoornot.design.Icon
import app.todoornot.design.LocalDesign
import app.todoornot.design.Tokens
import app.todoornot.files.AttachmentFiles
import app.todoornot.model.Actions
import app.todoornot.model.OrganizeEdit
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext

@Composable
internal fun Attachments(detail: TaskDetail, sync: SyncState, actions: Actions) {
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    var busy by remember { mutableStateOf(false) }
    var notice by remember { mutableStateOf<String?>(null) }
    // Remember the exact destination through activity recreation. The section is keyed by task ID.
    var saving by rememberSaveable { mutableStateOf<String?>(null) }
    val offline = sync == SyncState.OFFLINE

    fun transfer(progress: String, failure: String, work: suspend (java.io.File) -> String?) {
        if (busy) return
        busy = true
        notice = progress
        scope.launch {
            try {
                val file = AttachmentFiles.temporary(context)
                try { notice = work(file) } finally { file.delete() }
            } catch (e: CancellationException) { throw e }
            catch (_: AttachmentFiles.TooLarge) { notice = attachmentTooLarge() }
            catch (e: FfiException.Invalid) { notice = e.detail }
            catch (_: Exception) { notice = failure }
            finally { busy = false }
        }
    }

    fun download(row: AttachmentRow, destination: Uri?) {
        transfer("Downloading ${row.name}…", "Couldn't ${if (destination == null) "open" else "save"} ${row.name}.") { file ->
            actions.downloadAttachment(row.id, file)
            if (destination != null) {
                withContext(Dispatchers.IO) { AttachmentFiles.save(context, file, destination) }
                "Saved ${row.name}"
            } else {
                AttachmentFiles.open(context, file, row)
                null
            }
        }
    }

    val registry = LocalActivityResultRegistryOwner.current
    val save = if (registry != null) rememberLauncherForActivityResult(ActivityResultContracts.StartActivityForResult()) { result ->
        val row = detail.attachments.find { it.id == saving }
        saving = null
        val destination = result.data?.data
        if (result.resultCode == Activity.RESULT_OK && row != null && destination != null) download(row, destination)
    } else null
    val pick = if (registry != null) rememberLauncherForActivityResult(ActivityResultContracts.OpenDocument()) { uri ->
        if (uri != null) transfer("Uploading…", "Couldn’t upload the selected files.") { file ->
            val picked = withContext(Dispatchers.IO) { AttachmentFiles.copyPicked(context, uri, file) }
            actions.uploadAttachment(detail.id, picked.name, picked.media, file)
            "File uploaded."
        }
    } else null

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
            if (row.openType != null) IconButton("external-link", "Open “${row.name}”", enabled = !offline && !busy) { download(row, null) }
            IconButton("download", "Save “${row.name}”", enabled = !offline && !busy) {
                saving = row.id
                try {
                    save?.launch(Intent(Intent.ACTION_CREATE_DOCUMENT).addCategory(Intent.CATEGORY_OPENABLE)
                        .setType(row.openType ?: "application/octet-stream").putExtra(Intent.EXTRA_TITLE, row.name))
                } catch (_: android.content.ActivityNotFoundException) { notice = "Couldn't save ${row.name}." }
            }
            IconButton("trash-2", "Remove “${row.name}”") { actions.organize(OrganizeEdit.RemoveAttachment(row.id)) }
        }
    }
    Row(Modifier.heightIn(min = Tokens.Metrics.Phone.TOUCH.dp).clickable(enabled = !busy && !offline) {
        try { pick?.launch(arrayOf("*/*")) } catch (_: android.content.ActivityNotFoundException) { notice = "Couldn’t open the file picker." }
    }, verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(space.S2.dp)) {
        val ink = if (offline || busy) r.ink3 else r.accentStrong
        Icon("plus", ink)
        Label("Add file", Tokens.Type.BUTTON_SMALL, ink)
    }
    (if (offline && !busy) "Connect to upload or download attachments." else notice)?.let {
        Label(it, Tokens.Type.META_PHONE, r.ink3, Modifier.semantics { liveRegion = LiveRegionMode.Polite }, maxLines = Int.MAX_VALUE)
    }
}
