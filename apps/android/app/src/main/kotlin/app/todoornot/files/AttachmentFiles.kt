package app.todoornot.files

import android.content.ContentResolver
import android.content.Context
import android.content.Intent
import android.net.Uri
import android.provider.OpenableColumns
import androidx.core.content.FileProvider
import app.todoornot.core.AttachmentRow
import app.todoornot.core.attachmentLimitBytes
import java.io.File
import java.io.IOException
import java.io.InputStream
import java.io.OutputStream

class AttachmentProvider : FileProvider()

/** Only completed files enter the FileProvider subtree. Keep private paths unrelated to input names. */
object AttachmentFiles {
    private fun opened(c: Context) = File(c.cacheDir, "opened-attachments")
    private fun staging(c: Context) = File(c.cacheDir, "attachment-staging")
    fun clear(c: Context) {
        opened(c).deleteRecursively()
        staging(c).deleteRecursively()
    }

    fun temporary(c: Context): File = File.createTempFile("transfer-", null, staging(c).apply { mkdirs() })

    private fun checkPickerUri(c: Context, uri: Uri) {
        if (uri.scheme != ContentResolver.SCHEME_CONTENT || uri.authority == "${c.packageName}.attachments") {
            throw IOException("Not a picked document")
        }
    }

    data class Picked(val name: String, val media: String)
    class TooLarge : IOException()

    fun copyPicked(c: Context, uri: Uri, file: File): Picked {
        checkPickerUri(c, uri)
        val resolver = c.contentResolver
        val limit = attachmentLimitBytes().toLong()
        val name = resolver.query(uri, arrayOf(OpenableColumns.DISPLAY_NAME, OpenableColumns.SIZE), null, null, null)?.use { row ->
            if (!row.moveToFirst()) return@use null
            val size = row.getColumnIndex(OpenableColumns.SIZE)
            if (size >= 0 && !row.isNull(size) && row.getLong(size) > limit) throw TooLarge()
            row.getColumnIndex(OpenableColumns.DISPLAY_NAME).takeIf { it >= 0 }?.let(row::getString)
        }?.substringAfterLast('/')?.substringAfterLast('\\')?.takeIf(String::isNotBlank) ?: "attachment"
        val media = resolver.getType(uri)?.takeIf(String::isNotBlank) ?: "application/octet-stream"
        (resolver.openInputStream(uri) ?: throw IOException()).use { input ->
            file.outputStream().use { output -> copyBounded(input, output, limit) }
        }
        return Picked(name, media)
    }

    /** The size column is optional and not authoritative. Never stage more bytes than the server accepts. */
    internal fun copyBounded(input: InputStream, output: OutputStream, limit: Long) {
        val buffer = ByteArray(DEFAULT_BUFFER_SIZE)
        var left = limit
        while (true) {
            val n = input.read(buffer)
            if (n < 0) return
            if (n > left) throw TooLarge()
            output.write(buffer, 0, n)
            left -= n
        }
    }

    fun save(c: Context, file: File, destination: Uri) {
        checkPickerUri(c, destination)
        (c.contentResolver.openOutputStream(destination, "wt") ?: throw IOException()).use { output ->
            file.inputStream().use { it.copyTo(output) }
        }
    }

    fun open(c: Context, file: File, row: AttachmentRow) {
        val media = requireNotNull(row.openType)
        val ready = File.createTempFile("view-", ".${row.name.substringAfterLast('.')}", opened(c).apply { mkdirs() })
        try {
            if (!file.renameTo(ready)) throw IOException()
            val uri = FileProvider.getUriForFile(c, "${c.packageName}.attachments", ready, row.name)
            c.startActivity(Intent(Intent.ACTION_VIEW).setDataAndType(uri, media).addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION))
        } catch (e: Exception) {
            ready.delete()
            throw e
        }
    }
}
