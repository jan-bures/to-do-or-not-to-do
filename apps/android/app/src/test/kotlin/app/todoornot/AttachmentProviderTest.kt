package app.todoornot

import android.app.Application
import android.content.ContentProvider
import android.content.ContentValues
import android.content.Intent
import android.content.pm.ProviderInfo
import android.database.MatrixCursor
import android.net.Uri
import android.os.Bundle
import android.provider.OpenableColumns
import androidx.core.content.FileProvider
import app.todoornot.core.AttachmentRow
import app.todoornot.files.AttachmentFiles
import app.todoornot.ui.saveAttachmentIntent
import org.junit.Assert.*
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.RuntimeEnvironment
import org.robolectric.Shadows.shadowOf
import org.robolectric.annotation.Config
import org.robolectric.shadows.ShadowContentResolver
import java.io.File
import java.io.IOException

@RunWith(RobolectricTestRunner::class)
@Config(application = Application::class, sdk = [35])
class AttachmentProviderTest {
    @Test fun open_renames_staging_file_and_shares_it_read_only_with_display_name() {
        val app = RuntimeEnvironment.getApplication()
        val docs = AttachmentFiles(app) { context, authority, file, name ->
            FileProvider.getUriForFile(context, authority, androidPath(file), name)
        }
        val id = "00000000-0000-0000-0000-000000000001"
        val row = AttachmentRow(id, "Notes.md", "6 B", "text", "text/plain")
        val staged = docs.temporary().apply { writeText("sample") }
        val launch = docs.prepareOpen(staged, row)
        assertFalse(staged.exists())
        assertEquals("sample", File(app.cacheDir, "opened-attachments/$id.md").readText())
        assertNull(shadowOf(app).nextStartedActivity)
        launch()
        val intent = shadowOf(app).nextStartedActivity
        assertEquals(Intent.ACTION_VIEW, intent.action)
        assertEquals("text/plain", intent.type)
        val authority = "${app.packageName}.attachments"
        val uri = requireNotNull(intent.data)
        assertEquals(authority, uri.authority)
        assertEquals("/opened/$id.md", uri.path)
        assertEquals("Notes.md", uri.getQueryParameter("displayName"))
        assertEquals(Intent.FLAG_GRANT_READ_URI_PERMISSION or Intent.FLAG_ACTIVITY_NEW_TASK, intent.flags)
        assertThrows(IllegalArgumentException::class.java) { FileProvider.getUriForFile(app, authority, androidPath(File(app.cacheDir, "private"))) }
        assertThrows(IOException::class.java) { AttachmentFiles.checkPickerUri(app, uri) }
        assertThrows(IOException::class.java) { AttachmentFiles.checkPickerUri(app, Uri.parse("content://0@$authority/opened/$id.md")) }
        val abandoned = docs.temporary()
        AttachmentFiles.clear(app.cacheDir)
        assertFalse(File(app.cacheDir, "opened-attachments/$id.md").exists())
        assertFalse(abandoned.exists())
    }

    @Test fun create_document_keeps_the_name_without_using_the_viewer_mime() {
        val intent = saveAttachmentIntent("Notes.md")
        assertEquals(Intent.ACTION_CREATE_DOCUMENT, intent.action)
        assertEquals("Notes.md", intent.getStringExtra(Intent.EXTRA_TITLE))
        assertEquals("application/octet-stream", intent.type)
    }

    @Test fun discard_only_deletes_documents_known_to_be_empty() {
        val app = RuntimeEnvironment.getApplication()
        val deleted = mutableListOf<Uri>()
        val provider = object : ContentProvider() {
            override fun onCreate() = true
            override fun query(uri: Uri, projection: Array<out String>?, selection: String?, args: Array<out String>?, order: String?) =
                MatrixCursor(arrayOf(OpenableColumns.SIZE)).apply {
                    addRow(arrayOf(when (uri.lastPathSegment) { "empty" -> 0L; "nonempty" -> 6L; else -> null }))
                }
            override fun call(method: String, arg: String?, extras: Bundle?): Bundle {
                deleted += requireNotNull(extras?.getParcelable("uri", Uri::class.java))
                return Bundle()
            }
            override fun getType(uri: Uri) = null
            override fun insert(uri: Uri, values: ContentValues?) = error("unused")
            override fun delete(uri: Uri, selection: String?, args: Array<out String>?) = error("unused")
            override fun update(uri: Uri, values: ContentValues?, selection: String?, args: Array<out String>?) = error("unused")
        }
        provider.attachInfo(app, ProviderInfo().apply {
            authority = "picker"
            exported = true
        })
        ShadowContentResolver.registerProviderInternal("picker", provider)
        val base = "content://picker/document/"
        val docs = AttachmentFiles(app)
        for (name in listOf("empty", "nonempty", "unknown")) docs.discardEmpty(base + name)
        assertEquals(listOf(Uri.parse(base + "empty")), deleted)
    }

    // Robolectric uses the host filesystem. FileProvider expects Android's '/' at the root boundary.
    private fun androidPath(file: File) = if (File.separatorChar == '/') file else object : File(file.path) {
        override fun getCanonicalPath() = requireNotNull(file.parentFile).canonicalPath + "/" + file.name
    }
}
