package app.todoornot

import app.todoornot.core.AttachmentRow
import app.todoornot.model.AttachmentTransferCore
import app.todoornot.model.AttachmentTransfers
import app.todoornot.model.Documents
import app.todoornot.model.FileAction
import app.todoornot.model.TransferStatus
import java.io.File
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.awaitCancellation
import kotlinx.coroutines.test.StandardTestDispatcher
import kotlinx.coroutines.test.runCurrent
import kotlinx.coroutines.test.runTest
import org.junit.Assert.*
import org.junit.Rule
import org.junit.Test
import org.junit.rules.TemporaryFolder

@OptIn(ExperimentalCoroutinesApi::class)
class AttachmentTransfersTest {
    @get:Rule val folder = TemporaryFolder()

    private inner class Files : Documents {
        var staged: File? = null
        val destination = folder.newFile()
        var saved: () -> Unit = {}
        val discarded = mutableListOf<String>()

        override fun temporary() = folder.newFile().also { staged = it }
        override fun copyPicked(uri: String, file: File, check: () -> Unit) = Documents.Picked("sample.txt", "text/plain")
        override fun save(file: File, uri: String, check: () -> Unit) {
            check()
            file.copyTo(destination, overwrite = true)
            saved()
        }
        override fun discardEmpty(uri: String) { discarded += uri }
        override fun prepareOpen(file: File, row: AttachmentRow): () -> Unit = error("unexpected Open")
    }

    private fun core(await: suspend (String, ULong) -> Boolean = { _, _ -> true }) = AttachmentTransferCore(
        epoch = { 0u },
        upload = { _, _, _, _, _ -> "attachment" },
        download = { _, path, _ -> File(path).writeText("sample") },
        await = await,
    )

    @Test fun cancel_during_post_upload_await_reports_success_not_a_retry() = runTest {
        val docs = Files()
        var waiting = false
        val transfers = AttachmentTransfers(core { id, _ ->
            assertEquals("attachment", id)
            waiting = true
            awaitCancellation()
        }, docs, this, StandardTestDispatcher(testScheduler), { "task" }, {})
        transfers.start(FileAction.Upload("task", "picked"))
        assertNull("staging must not run inline on Main", docs.staged)
        runCurrent()
        assertTrue(waiting)
        assertEquals(TransferStatus("attachment", "Waiting to sync…", true), transfers.status("task"))
        transfers.cancel()
        runCurrent()
        assertEquals(TransferStatus("attachment", "File uploaded. It will appear after the next sync.", false), transfers.status("task"))
        assertFalse(docs.staged!!.exists())
    }

    @Test fun cancelling_at_the_end_of_save_keeps_the_complete_destination() = runTest {
        val docs = Files()
        val transfers = AttachmentTransfers(core(), docs, this, StandardTestDispatcher(testScheduler), { "task" }, {})
        docs.saved = transfers::cancel // The copy has completed but the continuation hasn't returned to Main.
        transfers.start(FileAction.Save("task", "attachment", "sample.txt", "destination"))
        runCurrent()
        assertEquals("sample", docs.destination.readText())
        assertTrue(docs.discarded.isEmpty())
        assertEquals(TransferStatus("attachment", "Saved sample.txt", false), transfers.status("task"))
        assertFalse(docs.staged!!.exists())
    }

    @Test fun finishing_for_a_task_that_is_no_longer_selected_leaves_no_notice() = runTest {
        var selected = "task"
        val transfers = AttachmentTransfers(core(), Files(), this, StandardTestDispatcher(testScheduler), { selected }, {})
        transfers.start(FileAction.Upload("task", "picked"))
        selected = "other"
        transfers.forget("task")
        assertTrue(transfers.status("task")!!.busy)
        runCurrent()
        selected = "task"
        assertNull(transfers.status("task"))
    }
}
