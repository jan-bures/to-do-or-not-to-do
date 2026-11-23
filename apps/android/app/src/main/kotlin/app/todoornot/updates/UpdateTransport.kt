package app.todoornot.updates

import android.net.Network
import android.os.SystemClock
import java.io.File
import java.io.IOException
import java.io.InputStream
import java.net.URI
import javax.net.ssl.HttpsURLConnection
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
private const val MAX_REDIRECTS = 5
private const val DOWNLOAD_TIMEOUT_MS = 3 * 60_000L

private fun deadlineProblem() = FeedProblem("The download took too long. Try again.", retry = true)

private class DownloadDeadline {
    private val until = SystemClock.elapsedRealtime() + DOWNLOAD_TIMEOUT_MS
    fun remaining(): Long = (until - SystemClock.elapsedRealtime()).takeIf { it > 0 } ?: throw deadlineProblem()
}

internal fun indexUrl(base: String) = URI("${base.trimEnd('/')}/releases/latest/download/$ANDROID_INDEX")
internal fun assetUrl(base: String, release: UpdateManifest) =
    URI("${base.trimEnd('/')}/releases/download/v${release.versionName}/${release.fileName}")

internal fun httpsUrl(value: URI): Boolean = value.scheme == "https" && value.host != null &&
    value.userInfo == null && value.fragment == null

/** The public assets never receive an Authorization header or have access to pairing state. */
private suspend fun <T> get(
    initial: URI, optional: Boolean = false, network: Network? = null, deadline: DownloadDeadline? = null,
    read: suspend (InputStream) -> T,
): T? {
    var address = initial
    repeat(MAX_REDIRECTS + 1) {
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
                            val next = connection.getHeaderField("Location")?.let { location ->
                                runCatching { address.resolve(location) }.getOrNull()
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

internal suspend fun fetchIndex(base: String): String? = get(indexUrl(base), optional = true) { input ->
    val buffer = ByteArray(MAX_INDEX + 1)
    var used = 0
    while (used < buffer.size) {
        currentCoroutineContext().ensureActive()
        val count = input.read(buffer, used, buffer.size - used)
        if (count < 0) break
        used += count
    }
    if (used > MAX_INDEX) throw FeedProblem("The update information isn't valid.")
    String(buffer, 0, used, Charsets.UTF_8)
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
