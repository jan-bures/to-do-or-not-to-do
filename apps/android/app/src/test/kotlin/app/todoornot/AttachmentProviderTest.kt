package app.todoornot

import android.app.Application
import android.net.Uri
import androidx.core.content.FileProvider
import app.todoornot.files.AttachmentFiles
import org.junit.Assert.*
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.RuntimeEnvironment
import org.robolectric.annotation.Config
import java.io.File
import java.io.IOException

@RunWith(RobolectricTestRunner::class)
@Config(application = Application::class, sdk = [35])
class AttachmentProviderTest {
    @Test fun manifest_provider_grants_only_opened_files_and_is_not_a_picker_source() {
        val app = RuntimeEnvironment.getApplication()
        val authority = "${app.packageName}.attachments"
        val file = File(File(app.cacheDir, "opened-attachments").apply { mkdirs() }, "sample.txt").apply { writeText("sample") }
        // Robolectric uses the host java.io.File. AndroidX compares canonical paths with
        // root + literal '/', even on Windows. Supply that Android separator at the root boundary;
        // the provider must still resolve the actual manifest authority and XML cache subtree.
        fun androidPath(f: File) = if (File.separatorChar == '/') f else object : File(f.path) {
            override fun getCanonicalPath() = f.parentFile.canonicalPath + "/" + f.name
        }
        val uri = FileProvider.getUriForFile(app, authority, androidPath(file))
        assertEquals("content://$authority/opened/sample.txt", uri.toString())
        assertThrows(IllegalArgumentException::class.java) { FileProvider.getUriForFile(app, authority, androidPath(File(app.cacheDir, "private"))) }
        assertThrows(IOException::class.java) { AttachmentFiles.checkPickerUri(app, uri) }
        assertThrows(IOException::class.java) { AttachmentFiles.checkPickerUri(app, Uri.parse("content://0@$authority/opened/sample.txt")) }
    }
}
