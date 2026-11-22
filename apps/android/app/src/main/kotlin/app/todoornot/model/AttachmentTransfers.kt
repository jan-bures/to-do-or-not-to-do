package app.todoornot.model

import app.todoornot.core.AttachmentRow
import app.todoornot.core.FfiException
import app.todoornot.core.TodoEngine
import app.todoornot.core.attachmentTooLarge
import java.io.File
import kotlinx.coroutines.*

/** Only private staging files cross the native boundary. Platform document handles stay on Android. */
interface Documents {
    fun temporary(): File
    fun copyPicked(uri: String, file: File, check: () -> Unit): Picked
    fun save(file: File, uri: String, check: () -> Unit)
    fun discard(uri: String)
    fun open(file: File, row: AttachmentRow)
    data class Picked(val name: String, val media: String)
    class TooLarge : java.io.IOException()
}

data class TransferStatus(val attachment: String?, val text: String, val busy: Boolean)
sealed interface FileAction {
    val task: String
    data class Upload(override val task: String, val uri: String) : FileAction
    data class Save(override val task: String, val id: String, val name: String, val uri: String) : FileAction
    data class Open(override val task: String, val row: AttachmentRow) : FileAction
}

/** Main-thread coordinator, scoped to a pairing, not a DetailPane. Different tasks can transfer
 * independently; a second tap on a busy task cannot duplicate its upload. */
internal class AttachmentTransfers(
    private val engine: TodoEngine,
    private val docs: Documents?,
    private val scope: CoroutineScope,
    private val io: CoroutineDispatcher,
    private val changed: () -> Unit,
) {
    private val states = java.util.concurrent.ConcurrentHashMap<String, TransferStatus>()
    private val jobs = mutableMapOf<String, Job>()
    private val destinations = mutableSetOf<String>()
    fun status(task: String?) = task?.let(states::get)
    fun notice(task: String, message: String) { if (task !in jobs) set(task, null, message, false) }
    fun forget(task: String) { states.computeIfPresent(task) { _, state -> state.takeIf { it.busy } } }
    private fun set(task: String, id: String?, text: String, busy: Boolean) {
        states[task] = TransferStatus(id, text, busy)
        changed()
    }

    fun cancel() = jobs.values.toList().forEach { it.cancel() }

    fun start(action: FileAction) {
        val docs = docs ?: return
        val task = action.task
        if (action is FileAction.Save && action.uri in destinations) return
        if (task in jobs || !scope.isActive) {
            // SAF already created this destination. Clean it up even if the pairing just closed.
            if (action is FileAction.Save) scope.launch(NonCancellable + io) { docs.discard(action.uri) }
            return
        }
        val id = when (action) { is FileAction.Save -> action.id; is FileAction.Open -> action.row.id; else -> null }
        val progress = when (action) { is FileAction.Save -> "Downloading ${action.name}…"; is FileAction.Open -> "Downloading ${action.row.name}…"; else -> "Uploading…" }
        // Capture before staging, so stop also invalidates work not yet inside the native call.
        val epoch = engine.attachmentEpoch()
        scope.launch(start = CoroutineStart.UNDISPATCHED) {
            jobs[task] = coroutineContext.job
            if (action is FileAction.Save) destinations += action.uri
            set(task, id, progress, true)
            var file: File? = null
            var saved = false
            var message = "Transfer cancelled."
            val check = { coroutineContext.ensureActive() }
            try {
                val tmp = docs.temporary().also { file = it }
                message = when (action) {
                    is FileAction.Upload -> {
                        val accepted = withContext(io) {
                            val picked = docs.copyPicked(action.uri, tmp, check)
                            check()
                            engine.uploadAttachment(task, picked.name, picked.media, tmp.absolutePath, epoch)
                        }
                        set(task, accepted, "Waiting to sync…", true)
                        if (withContext(io) { engine.awaitAttachment(accepted, epoch) }) "File uploaded."
                        else "The uploaded file hasn’t synced to this view yet."
                    }
                    is FileAction.Save -> {
                        withContext(io) {
                            engine.downloadAttachment(action.id, tmp.absolutePath, epoch)
                            check()
                            docs.save(tmp, action.uri, check)
                        }
                        saved = true
                        "Saved ${action.name}"
                    }
                    is FileAction.Open -> {
                        withContext(io) { engine.downloadAttachment(action.row.id, tmp.absolutePath, epoch) }
                        // Back on Main: unpair cannot interleave between this check and launching the viewer.
                        check()
                        docs.open(tmp, action.row)
                        ""
                    }
                }
            } catch (e: CancellationException) { throw e }
            catch (_: Documents.TooLarge) { message = attachmentTooLarge() }
            catch (e: FfiException.Invalid) { message = e.detail }
            catch (_: Exception) {
                message = when (action) { is FileAction.Save -> "Couldn’t save ${action.name}."; is FileAction.Open -> "Couldn’t open ${action.row.name}."; else -> "Couldn’t upload the selected file." }
            } finally {
                try {
                    file?.delete()
                    if (action is FileAction.Save && !saved) withContext(NonCancellable + io) { docs.discard(action.uri) }
                } finally {
                    jobs.remove(task)
                    if (action is FileAction.Save) destinations -= action.uri
                    set(task, id, message, false)
                }
            }
        }
    }
}
