package app.todoornot.files

import android.content.ContentResolver
import android.content.Context
import android.content.Intent
import android.net.Uri
import android.provider.DocumentsContract
import android.provider.OpenableColumns
import androidx.core.content.FileProvider
import app.todoornot.core.AttachmentRow
import app.todoornot.core.attachmentLimitBytes
import app.todoornot.model.Documents
import java.io.File
import java.io.IOException
import java.io.InputStream
import java.io.OutputStream
import java.util.UUID

class AttachmentProvider : FileProvider()

/** Application context only: a transfer may outlive the Activity that opened the picker. */
class AttachmentFiles(private val app: Context) : Documents {
    override fun temporary(): File = File.createTempFile("transfer-", null, staging(app.cacheDir).apply { mkdirs() })

    override fun copyPicked(uri: String, file: File, check: () -> Unit): Documents.Picked {
        val source = checkPickerUri(app, Uri.parse(uri))
        val resolver = app.contentResolver
        val limit = attachmentLimitBytes().toLong()
        val name = resolver.query(source, arrayOf(OpenableColumns.DISPLAY_NAME, OpenableColumns.SIZE), null, null, null)?.use { row ->
            if (!row.moveToFirst()) return@use null
            val size = row.getColumnIndex(OpenableColumns.SIZE)
            if (size >= 0 && !row.isNull(size) && row.getLong(size) > limit) throw Documents.TooLarge()
            row.getColumnIndex(OpenableColumns.DISPLAY_NAME).takeIf { it >= 0 }?.let(row::getString)
        }
        val picked = Documents.Picked(safeName(name.orEmpty()), safeMedia(resolver.getType(source)))
        (resolver.openInputStream(source) ?: throw IOException()).use { input ->
            file.outputStream().use { output -> copyBounded(input, output, limit, check) }
        }
        return picked
    }

    override fun save(file: File, uri: String, check: () -> Unit) {
        val destination = checkPickerUri(app, Uri.parse(uri))
        (app.contentResolver.openOutputStream(destination, "wt") ?: throw IOException()).use { output ->
            file.inputStream().use { input -> copyBounded(input, output, Long.MAX_VALUE, check) }
        }
    }

    // ACTION_CREATE_DOCUMENT has already created a file. Best effort: a remote provider may be gone.
    override fun discard(uri: String) { runCatching {
        DocumentsContract.deleteDocument(app.contentResolver, checkPickerUri(app, Uri.parse(uri)))
    } }

    override fun open(file: File, row: AttachmentRow) {
        val media = requireNotNull(row.openType)
        // Atomic replacement, one copy per immutable reference; the display name is never a path.
        val ready = File(opened(app.cacheDir).apply { mkdirs() }, "${UUID.fromString(row.id)}.${row.name.substringAfterLast('.')}")
        if (!file.renameTo(ready)) throw IOException()
        val uri = FileProvider.getUriForFile(app, "${app.packageName}.attachments", ready, row.name)
        app.startActivity(Intent(Intent.ACTION_VIEW).setDataAndType(uri, media)
            .addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION or Intent.FLAG_ACTIVITY_NEW_TASK))
    }

    companion object {
        private fun opened(cache: File) = File(cache, "opened-attachments")
        private fun staging(cache: File) = File(cache, "attachment-staging")
        fun clear(cache: File) {
            opened(cache).deleteRecursively()
            staging(cache).deleteRecursively()
        }

        internal fun checkPickerUri(c: Context, uri: Uri): Uri {
            val authority = uri.authority?.substringAfterLast('@')
            if (uri.scheme != ContentResolver.SCHEME_CONTENT || authority.isNullOrEmpty() || authority == "${c.packageName}.attachments") {
                throw IOException("Not a picked document")
            }
            return uri
        }

        private val mime = Regex("[A-Za-z0-9!#$%&'+.^_`|~-]+/[A-Za-z0-9!#$%&'+.^_`|~-]+")
        internal fun safeMedia(raw: String?) = raw?.takeIf { it.length <= 255 && mime.matches(it) } ?: "application/octet-stream"
        internal fun safeName(raw: String): String {
            val leaf = raw.substringAfterLast('/').substringAfterLast('\\').filterNot {
                it.isISOControl() || Character.getType(it) == Character.FORMAT.toInt() || it == '\u2028' || it == '\u2029'
            }.trim().trimEnd('.', ' ').ifEmpty { "attachment" }
            if (leaf.toByteArray().size <= 255) return leaf
            val dot = leaf.lastIndexOf('.')
            val suffix = if (dot > 0 && leaf.substring(dot).toByteArray().size < 255) leaf.substring(dot) else ""
            val budget = 255 - suffix.toByteArray().size
            val stem = leaf.removeSuffix(suffix)
            return buildString {
                var n = 0
                var i = 0
                while (i < stem.length) {
                    val cp = stem.codePointAt(i)
                    n += when { cp <= 0x7f -> 1; cp <= 0x7ff -> 2; cp <= 0xffff -> 3; else -> 4 }
                    if (n > budget) break
                    appendCodePoint(cp)
                    i += Character.charCount(cp)
                }
                if (isEmpty()) append('_')
                append(suffix)
            }
        }

        /** The provider's size is optional/untrusted. Check cancellation and the bound before each write. */
        internal fun copyBounded(input: InputStream, output: OutputStream, limit: Long, check: () -> Unit = {}) {
            val buffer = ByteArray(DEFAULT_BUFFER_SIZE)
            var left = limit
            while (true) {
                check()
                val n = input.read(buffer)
                if (n < 0) return
                if (n > left) throw Documents.TooLarge()
                output.write(buffer, 0, n)
                left -= n
            }
        }
    }
}
