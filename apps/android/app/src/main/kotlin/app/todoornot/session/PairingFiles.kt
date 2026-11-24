package app.todoornot.session

import java.io.File
import java.io.IOException
import java.nio.channels.FileChannel
import java.nio.file.Files
import java.nio.file.LinkOption.NOFOLLOW_LINKS
import java.nio.file.StandardCopyOption.ATOMIC_MOVE
import java.nio.file.StandardOpenOption.CREATE_NEW
import java.nio.file.StandardOpenOption.WRITE
import java.security.SecureRandom

/**
 * Where each pairing keeps its cache, as on the desktop (`apps/desktop/src/config.rs`):
 *
 * - `pairings/<pairing ID>/todo.sqlite`, with the cache's log, its recovery record and any damaged copy;
 * - `pairings/<pairing ID>.signed-out`, beside the directory: this pairing was unpaired, remove its data.
 *
 * A pairing's data lives in a directory named after that pairing, so a new pairing never opens an old
 * cache, and cleaning up after an old pairing can't touch the current one. Unpairing records its intent
 * first, so a removal that fails or is cut short is finished at the next start ([sweep]). Only
 * directories named exactly like a pairing ID are ever removed.
 *
 * Links are never followed: if `pairings/` (or a pairing's directory) is a symlink or not a directory,
 * nothing is written, removed or opened through it.
 */
class PairingFiles(
    private val root: File,
    /** Removes one file or empty directory; tests replace it to make removal fail. */
    private val delete: (File) -> Boolean = File::delete,
) {
    val pairings: File get() = File(root, PAIRINGS)

    fun dir(pairingId: String): File = File(pairings, checked(pairingId))

    fun cache(pairingId: String): File = File(dir(pairingId), CACHE)

    private fun record(pairingId: String): File = File(pairings, checked(pairingId) + SIGNED_OUT)

    fun isSignedOut(pairingId: String): Boolean {
        val record = record(pairingId)
        return try {
            safePairings(create = false) != null && isRecord(record)
        } catch (_: IOException) {
            false // Nothing is read through a link; opening the cache reports the problem.
        }
    }

    /**
     * The cache file of [pairingId], with its directory created, for the engine to open. Throws when
     * `pairings/` or the pairing's directory is a link or not a directory.
     */
    @Throws(IOException::class)
    fun cacheToOpen(pairingId: String): File {
        safePairings(create = true)
        val dir = dir(pairingId).toPath()
        if (Files.isSymbolicLink(dir)) throw IOException("the pairing's directory is a link")
        if (!Files.exists(dir, NOFOLLOW_LINKS)) Files.createDirectory(dir)
        if (!Files.isDirectory(dir, NOFOLLOW_LINKS)) throw IOException("the pairing's directory isn't a directory")
        return cache(pairingId)
    }

    /**
     * Records that [pairingId]'s data is to be removed, flushed to disk before this returns. Unpairing
     * doesn't go ahead without it. The record is written to a new file (never through a link) and
     * moved into place.
     */
    @Throws(IOException::class)
    fun markSignedOut(pairingId: String) {
        safePairings(create = true)
        val target = record(pairingId).toPath()
        val tmp = File(pairings, target.fileName.toString() + ".tmp").toPath()
        if (Files.exists(tmp, NOFOLLOW_LINKS)) {
            // Left by an earlier try. Only a plain file is ours to replace.
            if (!Files.isRegularFile(tmp, NOFOLLOW_LINKS)) throw IOException("the unpair record's temporary file isn't a file")
            Files.delete(tmp)
        }
        try {
            FileChannel.open(tmp, CREATE_NEW, WRITE, NOFOLLOW_LINKS).use { it.force(true) }
            Files.move(tmp, target, ATOMIC_MOVE)
        } catch (e: IOException) {
            runCatching { Files.deleteIfExists(tmp) }
            throw IOException("the unpair record couldn't be written", e)
        }
    }

    /**
     * Removes [pairingId]'s directory, then its record. Returns false, leaving the record, when anything
     * couldn't be removed (or `pairings/` isn't safe to use): the next [sweep] tries again.
     */
    fun remove(pairingId: String): Boolean {
        val usable = try {
            safePairings(create = false)
        } catch (_: IOException) {
            return false
        } ?: return true
        val dir = File(usable, checked(pairingId))
        if (Files.exists(dir.toPath(), NOFOLLOW_LINKS) && !removeTree(dir)) return false
        val record = record(pairingId)
        return !Files.exists(record.toPath(), NOFOLLOW_LINKS) || delete(record)
    }

    /**
     * Finishes earlier unpairs: removes the data of every pairing with a record, except [keep] (the current
     * pairing). [beforeRemove] can hold a deletion until its credentials are gone. Returns the
     * pairings whose data couldn't be removed yet. Does nothing when `pairings/` isn't safe to use.
     */
    fun sweep(keep: String?, beforeRemove: (String) -> Boolean = { true }): List<String> {
        val dir = try {
            safePairings(create = false)
        } catch (_: IOException) {
            return emptyList()
        } ?: return emptyList()
        val names = dir.list() ?: return emptyList()
        return names.mapNotNull { it.removeSuffix(SIGNED_OUT).takeIf { id -> id != it && isPairingId(id) } }
            .filter { it != keep && isRecord(record(it)) }
            .filterNot { beforeRemove(it) && remove(it) }
    }

    /** Only a plain file is a record: a link (even a dangling one) or a directory by that name isn't. */
    private fun isRecord(f: File): Boolean = Files.isRegularFile(f.toPath(), NOFOLLOW_LINKS)

    /** Cache files from before pairings had their own directories, which the app leaves alone. */
    fun oldLayoutFiles(): List<String> =
        root.listFiles { f -> f.isFile && f.name.startsWith("todo.") }?.map { it.name }.orEmpty()

    /**
     * `pairings/` if it exists (or, with [create], once it does), null if it doesn't. Throws, touching
     * nothing, when it is a link or not a directory.
     */
    @Throws(IOException::class)
    private fun safePairings(create: Boolean): File? {
        val path = pairings.toPath()
        if (Files.isSymbolicLink(path)) throw IOException("the pairings directory is a link")
        if (!Files.exists(path, NOFOLLOW_LINKS)) {
            if (!create) return null
            Files.createDirectory(path)
        }
        if (!Files.isDirectory(path, NOFOLLOW_LINKS)) throw IOException("the pairings directory isn't a directory")
        return pairings
    }

    /** Never follows a link: a symlink is removed as a link. */
    private fun removeTree(f: File): Boolean {
        if (Files.isDirectory(f.toPath(), NOFOLLOW_LINKS)) {
            f.listFiles()?.forEach { if (!removeTree(it)) return false }
        }
        return delete(f) || !Files.exists(f.toPath(), NOFOLLOW_LINKS)
    }

    private fun checked(pairingId: String): String {
        require(isPairingId(pairingId)) { "not a pairing ID" }
        return pairingId
    }

    companion object {
        private const val PAIRINGS = "pairings"
        private const val CACHE = "todo.sqlite"
        private const val SIGNED_OUT = ".signed-out"
        private val ID = Regex("[0-9a-f]{32}")

        fun isPairingId(s: String): Boolean = ID.matches(s)

        /** A new random pairing ID: 128 bits as lowercase hex. */
        fun newPairingId(random: SecureRandom = SecureRandom()): String =
            ByteArray(16).also(random::nextBytes).joinToString("") { "%02x".format(it) }
    }
}
