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
    fun discardEmpty(uri: String)
    /** Prepares the viewer off Main; the returned action launches it on Main. */
    fun prepareOpen(file: File, row: AttachmentRow): () -> Unit
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

internal class AttachmentTransferCore(
    val epoch: () -> ULong,
    val upload: (String, String, String, String, ULong) -> String,
    val download: (String, String, ULong) -> Unit,
    val await: suspend (String, ULong) -> Boolean,
) {
    constructor(engine: TodoEngine) : this(engine::attachmentEpoch, engine::uploadAttachment,
        engine::downloadAttachment, { id, epoch -> engine.awaitAttachment(id, epoch) })
}

/** Main-thread coordinator, scoped to a pairing, not a DetailPane. Different tasks can transfer
 * independently; a second tap on a busy task cannot duplicate its upload. */
internal class AttachmentTransfers(
    private val core: AttachmentTransferCore,
    private val docs: Documents?,
    private val scope: CoroutineScope,
    private val io: CoroutineDispatcher,
    private val selected: () -> String?,
    private val changed: () -> Unit,
) {
    private val states = java.util.concurrent.ConcurrentHashMap<String, TransferStatus>()
    private val jobs = mutableMapOf<String, Job>()
    private val destinations = mutableSetOf<String>()
    fun status(task: String?) = task?.let(states::get)
    fun notice(task: String, message: String) { if (task == selected() && task !in jobs) set(task, null, message, false) }
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
            // Even if the pairing just closed, clean up an empty destination left by the picker.
            if (action is FileAction.Save) scope.launch(NonCancellable + io) { docs.discardEmpty(action.uri) }
            return
        }
        val id = when (action) { is FileAction.Save -> action.id; is FileAction.Open -> action.row.id; else -> null }
        val progress = when (action) { is FileAction.Save -> "Downloading ${action.name}…"; is FileAction.Open -> "Downloading ${action.row.name}…"; else -> "Uploading…" }
        // Capture before staging, so explicit cancellation also invalidates work not yet in Rust.
        val epoch = core.epoch()
        scope.launch(start = CoroutineStart.UNDISPATCHED) {
            jobs[task] = coroutineContext.job
            if (action is FileAction.Save) destinations += action.uri
            set(task, id, progress, true)
            var file: File? = null
            var accepted: String? = null
            var saved = false
            var message = "Transfer cancelled."
            val check = { coroutineContext.ensureActive() }
            fun stage() = docs.temporary().also { file = it }
            try {
                when (action) {
                    is FileAction.Upload -> {
                        val uploaded = withContext(io) {
                            val tmp = stage()
                            val picked = docs.copyPicked(action.uri, tmp, check)
                            check()
                            core.upload(task, picked.name, picked.media, tmp.absolutePath, epoch).also {
                                // The IO result may be discarded if cancellation wins the hop to Main.
                                accepted = it
                                message = "File uploaded. It will appear after the next sync."
                            }
                        }
                        set(task, uploaded, "Waiting to sync…", true)
                        if (withContext(io) { core.await(uploaded, epoch) }) message = "File uploaded."
                    }
                    is FileAction.Save -> withContext(io) {
                        val tmp = stage()
                        core.download(action.id, tmp.absolutePath, epoch)
                        check()
                        docs.save(tmp, action.uri, check)
                        saved = true
                        message = "Saved ${action.name}"
                    }
                    is FileAction.Open -> {
                        val launch = withContext(io) {
                            val tmp = stage()
                            core.download(action.row.id, tmp.absolutePath, epoch)
                            check()
                            docs.prepareOpen(tmp, action.row)
                        }
                        // Back on Main: unpair cannot interleave between this check and launching the viewer.
                        check()
                        launch()
                        message = ""
                    }
                }
            } catch (e: CancellationException) { throw e }
            catch (_: Documents.TooLarge) { message = attachmentTooLarge() }
            catch (e: FfiException.Invalid) { if (accepted == null) message = e.detail }
            catch (_: Exception) {
                if (accepted == null) message = when (action) { is FileAction.Save -> "Couldn’t save ${action.name}."; is FileAction.Open -> "Couldn’t open ${action.row.name}."; else -> "Couldn’t upload the selected file." }
            } finally {
                try {
                    withContext(NonCancellable + io) {
                        file?.delete()
                        if (action is FileAction.Save && !saved) docs.discardEmpty(action.uri)
                    }
                } finally {
                    jobs.remove(task)
                    if (action is FileAction.Save) destinations -= action.uri
                    if (selected() == task) set(task, accepted ?: id, message, false)
                    else {
                        states.remove(task)
                        changed()
                    }
                }
            }
        }
    }
}
