package app.todoornot.updates

import android.net.Network
import java.io.File
import java.io.IOException
import java.io.InputStream
import java.net.URI
import javax.net.ssl.HttpsURLConnection
import kotlinx.coroutines.currentCoroutineContext
import kotlinx.coroutines.ensureActive

internal class FeedProblem(val messageForOwner: String, val retry: Boolean = false) : IOException()

internal const val ANDROID_INDEX = "android-update.json"
private const val MAX_REDIRECTS = 5

internal fun indexUrl(base: String) = URI("${base.trimEnd('/')}/releases/latest/download/$ANDROID_INDEX")
internal fun assetUrl(base: String, release: UpdateManifest) =
    URI("${base.trimEnd('/')}/releases/download/v${release.versionName}/${release.fileName}")

internal fun httpsUrl(value: URI): Boolean = value.scheme == "https" && value.host != null &&
    value.userInfo == null && value.fragment == null

/** The public assets never receive an Authorization header or have access to pairing state. */
private suspend fun <T> get(
    initial: URI, optional: Boolean = false, network: Network? = null, read: suspend (InputStream) -> T,
): T? {
    var address = initial
    repeat(MAX_REDIRECTS + 1) {
        currentCoroutineContext().ensureActive()
        if (!httpsUrl(address)) throw FeedProblem("The update address isn't secure.")
        val url = address.toURL()
        val connection = (network?.openConnection(url) ?: url.openConnection()) as HttpsURLConnection
        try {
            connection.instanceFollowRedirects = false
            connection.useCaches = false
            connection.connectTimeout = 15_000
            connection.readTimeout = 20_000
            connection.setRequestProperty("Accept-Encoding", "identity")
            val status = connection.responseCode
            when {
                status == 200 -> return connection.inputStream.use { read(it) }
                status == 404 && optional -> return null
                status in listOf(301, 302, 303, 307, 308) -> {
                    address = connection.getHeaderField("Location")?.let { location ->
                        runCatching { address.resolve(location) }.getOrNull()
                    } ?: throw FeedProblem("The update server sent an invalid link.")
                }
                status == 429 || status >= 500 -> throw FeedProblem("The update server is busy. Try again later.", retry = true)
                else -> throw FeedProblem("The update server sent an unexpected reply.")
            }
        } finally {
            connection.disconnect()
        }
    }
    throw FeedProblem("The update server sent too many redirects.")
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
    get(assetUrl(base, release), network = network) { input ->
        part.outputStream().buffered().use { output ->
            val buffer = ByteArray(DEFAULT_BUFFER_SIZE)
            var size = 0L
            while (true) {
                currentCoroutineContext().ensureActive()
                if (!allowed()) throw FeedProblem("Waiting for an unmetered connection.", retry = true)
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
