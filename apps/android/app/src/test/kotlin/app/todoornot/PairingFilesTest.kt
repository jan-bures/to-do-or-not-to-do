package app.todoornot

import app.todoornot.session.PairingFiles
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotEquals
import org.junit.Assert.assertThrows
import org.junit.Assert.assertTrue
import org.junit.Assume.assumeTrue
import org.junit.Test
import java.io.File
import java.io.IOException
import java.nio.file.Files
import kotlin.io.path.createTempDirectory

class PairingFilesTest {
    private val root: File = createTempDirectory("todo-files").toFile()
    private val files = PairingFiles(root)
    private val a = "0".repeat(31) + "a"
    private val b = "0".repeat(31) + "b"

    @After
    fun tearDown() {
        root.deleteRecursively()
    }

    private fun populate(id: String) = files.cache(id).apply { parentFile?.mkdirs(); writeText("cache") }

    @Test
    fun pairing_ids_are_random_hex_and_nothing_else_names_a_directory() {
        val id = PairingFiles.newPairingId()
        assertTrue(PairingFiles.isPairingId(id))
        assertNotEquals(id, PairingFiles.newPairingId())
        listOf("..", "../x", "ABC", "", "0".repeat(31), "g".repeat(32)).forEach { bad ->
            assertThrows(IllegalArgumentException::class.java) { files.dir(bad) }
        }
    }

    @Test
    fun the_sweep_removes_only_marked_pairings_and_never_the_current_one() {
        val keep = populate(a)
        val gone = populate(b)
        val stranger = File(files.pairings, "not-a-pairing").apply { mkdirs() }
        File(files.pairings, "not-a-pairing.signed-out").writeText("")
        files.markSignedOut(a)
        files.markSignedOut(b)

        assertEquals(emptyList<String>(), files.sweep(keep = a))
        assertTrue(keep.exists())
        assertTrue(files.isSignedOut(a))
        assertFalse(gone.exists())
        assertFalse(files.isSignedOut(b))
        assertTrue("only exact pairing IDs are ever removed", stranger.exists())
    }

    @Test
    fun a_pairing_without_a_record_is_left_alone() {
        val cache = populate(a)
        files.sweep(keep = null)
        assertTrue(cache.exists())
    }

    @Test
    fun a_removal_that_fails_keeps_its_record_for_the_next_sweep() {
        var stuck = true
        val sticky = PairingFiles(root) { f -> if (stuck && f.name == "todo.sqlite") false else f.delete() }
        val cache = populate(a)
        sticky.markSignedOut(a)
        assertFalse(sticky.remove(a))
        assertTrue(sticky.isSignedOut(a))
        assertEquals(listOf(a), sticky.sweep(keep = null))
        stuck = false
        assertEquals(emptyList<String>(), sticky.sweep(keep = null))
        assertFalse(cache.exists())
        assertFalse(sticky.isSignedOut(a))
    }

    @Test
    fun removal_takes_a_symlink_away_without_following_it() {
        val outside = createTempDirectory("todo-outside").toFile()
        try {
            val precious = File(outside, "keep.txt").apply { writeText("keep") }
            populate(a)
            val link = File(files.dir(a), "link")
            val linked = runCatching { java.nio.file.Files.createSymbolicLink(link.toPath(), outside.toPath()) }.isSuccess
            files.markSignedOut(a)
            assertTrue(files.remove(a))
            assertFalse(files.dir(a).exists())
            if (linked) assertTrue(precious.exists())
        } finally {
            outside.deleteRecursively()
        }
    }

    private fun link(link: File, target: File) =
        assumeTrue("symlinks can't be made here", runCatching { Files.createSymbolicLink(link.toPath(), target.toPath()) }.isSuccess)

    private fun withOutside(body: (File) -> Unit) {
        val outside = createTempDirectory("todo-outside").toFile()
        try {
            body(outside)
        } finally {
            outside.deleteRecursively()
        }
    }

    @Test
    fun a_linked_pairings_directory_is_refused_and_left_alone() = withOutside { outside ->
        val theirs = File(outside, a).apply { mkdirs() }
        val cache = File(theirs, "todo.sqlite").apply { writeText("not ours") }
        File(outside, "$a.signed-out").writeText("")
        link(files.pairings, outside)
        try {
            assertThrows(IOException::class.java) { files.markSignedOut(b) }
            assertThrows(IOException::class.java) { files.cacheToOpen(b) }
            assertFalse("a record behind the link isn't read", files.isSignedOut(a))
            assertFalse(files.remove(a))
            assertEquals(emptyList<String>(), files.sweep(keep = null))
            assertEquals("not ours", cache.readText())
            assertEquals(listOf(a, "$a.signed-out"), outside.list()?.sorted())
        } finally {
            Files.deleteIfExists(files.pairings.toPath())
        }
    }

    @Test
    fun a_pairings_file_instead_of_a_directory_is_refused() {
        files.pairings.writeText("a file")
        assertThrows(IOException::class.java) { files.markSignedOut(a) }
        assertThrows(IOException::class.java) { files.cacheToOpen(a) }
        assertFalse(files.isSignedOut(a))
        assertFalse(files.remove(a))
        assertEquals(emptyList<String>(), files.sweep(keep = null))
        assertEquals("a file", files.pairings.readText())
    }

    @Test
    fun a_linked_pairing_directory_is_never_opened() = withOutside { outside ->
        files.pairings.mkdirs()
        link(files.dir(a), outside)
        assertThrows(IOException::class.java) { files.cacheToOpen(a) }
        assertEquals(0, outside.list()?.size)
    }

    @Test
    fun the_record_is_never_written_through_a_linked_temporary_file() = withOutside { outside ->
        val precious = File(outside, "keep.txt").apply { writeText("keep") }
        files.pairings.mkdirs()
        val tmp = File(files.pairings, "$a.signed-out.tmp")
        link(tmp, precious)
        assertThrows(IOException::class.java) { files.markSignedOut(a) }
        assertEquals("keep", precious.readText())
        assertFalse(files.isSignedOut(a))
    }

    @Test
    fun a_leftover_temporary_record_is_replaced() {
        files.pairings.mkdirs()
        File(files.pairings, "$a.signed-out.tmp").writeText("half")
        files.markSignedOut(a)
        assertTrue(files.isSignedOut(a))
        assertFalse(File(files.pairings, "$a.signed-out.tmp").exists())
    }

    @Test
    fun a_record_that_isnt_a_plain_file_is_ignored() = withOutside { outside ->
        val dangling = populate(a)
        val linked = populate(b)
        val dir = populate("0".repeat(31) + "c")
        link(File(files.pairings, "$a.signed-out"), File(outside, "missing")) // Dangling.
        link(File(files.pairings, "$b.signed-out"), File(outside, "keep.txt").apply { writeText("keep") })
        File(files.pairings, "${"0".repeat(31)}c.signed-out").mkdirs()

        listOf(a, b, "0".repeat(31) + "c").forEach { assertFalse(it, files.isSignedOut(it)) }
        assertEquals(emptyList<String>(), files.sweep(keep = null))
        listOf(linked, dangling, dir).forEach { assertTrue(it.path, it.exists()) }
        assertEquals("keep", File(outside, "keep.txt").readText())
    }
}
