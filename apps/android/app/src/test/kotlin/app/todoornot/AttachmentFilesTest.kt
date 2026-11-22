package app.todoornot

import app.todoornot.files.AttachmentFiles
import java.io.ByteArrayOutputStream
import org.junit.Assert.*
import org.junit.Test

class AttachmentFilesTest {
    @Test fun unknown_size_streams_are_limited_while_copying() {
        val out = ByteArrayOutputStream()
        val bytes = ByteArray(DEFAULT_BUFFER_SIZE + 5) { it.toByte() }
        AttachmentFiles.copyBounded(bytes.inputStream(), out, bytes.size.toLong())
        assertArrayEquals(bytes, out.toByteArray())
        out.reset()
        assertThrows(AttachmentFiles.TooLarge::class.java) {
            AttachmentFiles.copyBounded(bytes.inputStream(), out, bytes.size - 1L)
        }
        assertTrue(out.size() < bytes.size)
    }
}
