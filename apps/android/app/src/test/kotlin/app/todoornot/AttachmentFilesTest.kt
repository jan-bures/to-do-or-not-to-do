package app.todoornot

import app.todoornot.files.AttachmentFiles
import java.io.ByteArrayOutputStream
import org.junit.Assert.*
import org.junit.Test

class AttachmentFilesTest {
    @Test fun picker_names_are_safe_leaf_names_bounded_in_utf8_and_keep_the_extension() {
        assertEquals("sample.pdf", AttachmentFiles.safeName("../sa\u202emp\nle.pdf"))
        assertEquals("attachment", AttachmentFiles.safeName(".."))
        val name = AttachmentFiles.safeName("ř🍵".repeat(200) + ".txt")
        assertTrue(name.endsWith(".txt"))
        assertTrue(name.toByteArray().size in 250..255)
        assertEquals(name, name.toByteArray().toString(Charsets.UTF_8))
        for (invalid in listOf(null, "", "audio", "text/", "image/*", "text/plain\r\nx: y")) {
            assertEquals("application/octet-stream", AttachmentFiles.safeMedia(invalid))
        }
        assertEquals("image/png", AttachmentFiles.safeMedia("image/png"))
    }

    @Test fun unknown_size_streams_are_limited_while_copying() {
        val out = ByteArrayOutputStream()
        val bytes = ByteArray(DEFAULT_BUFFER_SIZE + 5) { it.toByte() }
        AttachmentFiles.copyBounded(bytes.inputStream(), out, bytes.size.toLong())
        assertArrayEquals(bytes, out.toByteArray())
        out.reset()
        assertThrows(app.todoornot.model.Documents.TooLarge::class.java) {
            AttachmentFiles.copyBounded(bytes.inputStream(), out, bytes.size - 1L)
        }
        assertTrue(out.size() < bytes.size)
    }
}
