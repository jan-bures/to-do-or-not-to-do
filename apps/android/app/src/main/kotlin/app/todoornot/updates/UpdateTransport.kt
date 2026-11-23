package app.todoornot.updates

import android.net.Network
import android.os.SystemClock
import java.io.File
import java.io.IOException
import java.io.InputStream
import java.io.StringReader
import java.net.URI
import java.nio.ByteBuffer
import java.nio.charset.CharacterCodingException
import javax.net.ssl.HttpsURLConnection
import javax.xml.parsers.SAXParserFactory
import org.xml.sax.Attributes
import org.xml.sax.InputSource
import org.xml.sax.SAXException
import org.xml.sax.SAXParseException
import org.xml.sax.helpers.DefaultHandler
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.coroutineScope
import kotlinx.coroutines.currentCoroutineContext
import kotlinx.coroutines.delay
import kotlinx.coroutines.ensureActive
import kotlinx.coroutines.launch

internal class FeedProblem(
    val messageForOwner: String, val retry: Boolean = false, val rememberFailure: Boolean = true,
) : IOException()

internal const val ANDROID_INDEX = "android-update.json"
internal const val MAX_RELEASE_HISTORY = 1024 * 1024
private const val MAX_CANDIDATES = 5
private const val ATOM = "http://www.w3.org/2005/Atom"
private val releaseId = Regex("tag:github\\.com,2008:Repository/[0-9]+/(v(?:0|[1-9][0-9]*)\\.(?:0|[1-9][0-9]*)\\.(?:0|[1-9][0-9]*)-build\\.([1-9][0-9]*))")
private const val MAX_REDIRECTS = 5
private val githubReleaseHosts = setOf("github.com", "objects.githubusercontent.com", "release-assets.githubusercontent.com")
private const val DOWNLOAD_TIMEOUT_MS = 3 * 60_000L
// Release history and indexes are small; don't let a slow redirect target hold the worker.
private const val METADATA_TIMEOUT_MS = 60_000L

private fun deadlineProblem() = FeedProblem("The download took too long. Try again.", retry = true)

private class DownloadDeadline(timeoutMs: Long = DOWNLOAD_TIMEOUT_MS) {
    private val until = SystemClock.elapsedRealtime() + timeoutMs
    fun remaining(): Long = (until - SystemClock.elapsedRealtime()).takeIf { it > 0 } ?: throw deadlineProblem()
}

internal fun historyUrl(base: String) = URI("${base.trimEnd('/')}/releases.atom")
internal fun indexUrl(base: String, tag: String) = URI("${base.trimEnd('/')}/releases/download/$tag/$ANDROID_INDEX")
internal fun assetUrl(base: String, release: UpdateManifest) =
    URI("${base.trimEnd('/')}/releases/download/v${release.versionName}/${release.fileName}")

internal fun httpsUrl(value: URI): Boolean = value.scheme == "https" && value.host != null &&
    value.userInfo == null && value.fragment == null

internal fun releaseRedirect(initial: URI, current: URI, location: String): URI? =
    runCatching { current.resolve(location) }.getOrNull()?.takeIf { next ->
        httpsUrl(next) && (next.host.equals(initial.host, ignoreCase = true) || next.host.lowercase() in githubReleaseHosts)
    }

/** The public assets never receive an Authorization header or have access to pairing state. */
private suspend fun <T> get(
    initial: URI, optional: Boolean = false, network: Network? = null, deadline: DownloadDeadline? = null,
    read: suspend (InputStream) -> T,
): T? {
    var address = initial
    repeat(MAX_REDIRECTS + 1) { hops ->
        currentCoroutineContext().ensureActive()
        deadline?.remaining()
        if (!httpsUrl(address)) throw FeedProblem("The update address isn't secure.")
        val url = address.toURL()
        val connection = (network?.openConnection(url) ?: url.openConnection()) as HttpsURLConnection
        try {
            connection.instanceFollowRedirects = false
            connection.useCaches = false
            val remaining = deadline?.remaining() ?: Long.MAX_VALUE
            connection.connectTimeout = minOf(remaining, 15_000).toInt()
            connection.readTimeout = minOf(remaining, 20_000).toInt()
            connection.setRequestProperty("Accept-Encoding", "identity")
            val response = coroutineScope {
                // Interrupt blocking I/O as well as checking elapsed time between chunks. A server
                // can otherwise keep a read alive by sending bytes more often than readTimeout.
                val watchdog = deadline?.let { launch(Dispatchers.IO) {
                    delay(remaining)
                    connection.disconnect()
                } }
                try {
                    val status = connection.responseCode
                    deadline?.remaining()
                    when {
                        status == 200 -> Response.Body(connection.inputStream.use { read(it) })
                        status == 404 && optional -> Response.Body(null)
                        status in listOf(301, 302, 303, 307, 308) -> {
                            if (hops == MAX_REDIRECTS) throw FeedProblem("The update server sent too many redirects.")
                            val next = connection.getHeaderField("Location")?.let { location ->
                                releaseRedirect(initial, address, location)
                            } ?: throw FeedProblem("The update server sent an invalid link.")
                            Response.Redirect(next)
                        }
                        status == 429 || status >= 500 -> throw FeedProblem("The update server is busy. Try again later.", retry = true)
                        else -> throw FeedProblem("The update server sent an unexpected reply.")
                    }
                } finally {
                    watchdog?.cancel()
                }
            }
            deadline?.remaining()
            when (response) {
                is Response.Body -> return response.value
                is Response.Redirect -> address = response.address
            }
        } catch (error: IOException) {
            deadline?.remaining()
            throw error
        } finally {
            connection.disconnect()
        }
    }
    throw FeedProblem("The update server sent too many redirects.")
}

private sealed interface Response<out T> {
    data class Body<T>(val value: T?) : Response<T>
    data class Redirect(val address: URI) : Response<Nothing>
}

private fun invalidInformation() = FeedProblem("The update information isn't valid.")

private class ReleaseHistory : DefaultHandler() {
    private var depth = 0
    private var entry = false
    private var id: StringBuilder? = null
    private var readingId = false
    val releases = mutableSetOf<Pair<Int, String>>()

    override fun startElement(uri: String, name: String, qualified: String, attrs: Attributes) {
        depth++
        when {
            depth > 64 || readingId -> throw SAXException()
            depth == 1 && (uri != ATOM || name != "feed") -> throw SAXException()
            depth == 2 && uri == ATOM && name == "entry" -> { entry = true; id = null }
            depth == 3 && entry && uri == ATOM && name == "id" -> {
                if (id != null) throw SAXException()
                id = StringBuilder()
                readingId = true
            }
        }
    }

    override fun characters(text: CharArray, start: Int, length: Int) {
        if (readingId) id?.append(text, start, length)
    }

    override fun endElement(uri: String, name: String, qualified: String) {
        if (depth == 3) readingId = false
        if (depth == 2 && entry) {
            val text = id?.toString()?.trim()?.takeIf { it.isNotEmpty() } ?: throw SAXException()
            releaseId.matchEntire(text)?.let { match ->
                match.groupValues[2].toIntOrNull()?.let { releases.add(it to match.groupValues[1]) }
            }
            entry = false
        }
        depth--
    }

    override fun resolveEntity(publicId: String?, systemId: String?): InputSource = throw SAXException()
    override fun error(error: SAXParseException) = throw error
}

internal fun releaseCandidates(xml: String): List<String> {
    // Strict UTF-8 decoding precedes this parser. Refuse DTDs, external entities, and nesting
    // beyond the needs of GitHub's Atom feed. Only direct entry IDs can identify releases.
    if (xml.length > MAX_RELEASE_HISTORY || "<!DOCTYPE" in xml) throw invalidInformation()
    val handler = ReleaseHistory()
    try {
        SAXParserFactory.newInstance().apply { isNamespaceAware = true }.newSAXParser()
            .parse(InputSource(StringReader(xml)), handler)
    } catch (_: SAXException) {
        throw invalidInformation()
    }
    return handler.releases.sortedWith(compareByDescending<Pair<Int, String>> { it.first }.thenByDescending { it.second })
        .take(MAX_CANDIDATES).map { it.second }
}

internal data class TaggedIndex(val tag: String, val raw: String)

internal suspend fun fetchIndex(
    base: String, read: suspend (URI, Int) -> String? = ::readOptionalText,
): TaggedIndex? {
    val history = read(historyUrl(base), MAX_RELEASE_HISTORY) ?: return null
    for (tag in releaseCandidates(history)) {
        val raw = read(indexUrl(base, tag), MAX_INDEX) ?: continue
        return TaggedIndex(tag, raw)
    }
    return null
}

private suspend fun readOptionalText(address: URI, limit: Int): String? {
    val deadline = DownloadDeadline(METADATA_TIMEOUT_MS)
    return get(address, optional = true, deadline = deadline) { input -> readText(input, limit, deadline) }
}

private suspend fun readText(input: InputStream, limit: Int, deadline: DownloadDeadline): String {
    val buffer = ByteArray(limit + 1)
    var used = 0
    while (used < buffer.size) {
        currentCoroutineContext().ensureActive()
        deadline.remaining()
        val count = input.read(buffer, used, buffer.size - used)
        if (count < 0) break
        used += count
    }
    if (used > limit) throw invalidInformation()
    return try {
        Charsets.UTF_8.newDecoder().decode(ByteBuffer.wrap(buffer, 0, used)).toString()
    } catch (_: CharacterCodingException) {
        throw invalidInformation()
    }
}

/** Retry a short/truncated transfer from the start, never promote partial or oversized bytes. */
internal suspend fun fetchApk(
    base: String, release: UpdateManifest, part: File, network: Network,
    allowed: () -> Boolean, progress: (Int) -> Unit,
) {
    val deadline = DownloadDeadline()
    get(assetUrl(base, release), network = network, deadline = deadline) { input ->
        part.outputStream().buffered().use { output ->
            val buffer = ByteArray(DEFAULT_BUFFER_SIZE)
            var size = 0L
            while (true) {
                currentCoroutineContext().ensureActive()
                deadline.remaining()
                if (!allowed()) throw FeedProblem("Waiting for an unmetered connection.", retry = true, rememberFailure = false)
                val count = input.read(buffer)
                if (count < 0) break
                size += count
                if (size > release.size) throw FeedProblem("The update failed its size check.")
                output.write(buffer, 0, count)
                progress((size * 100 / release.size).toInt())
            }
            if (size != release.size) throw FeedProblem("The download ended early. Try again.", retry = true)
        }
    }
}
