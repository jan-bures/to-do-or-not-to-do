package app.todoornot.updates

import java.io.File
import java.security.MessageDigest
import org.json.JSONObject

internal const val CHECK_HOURS = 6L
internal const val CHECK_INTERVAL_MS = CHECK_HOURS * 60 * 60 * 1_000
internal const val MAX_INDEX = 16 * 1024
private const val MAX_APK = 128L * 1024 * 1024
private val digestPattern = Regex("[a-f0-9]{64}")

internal fun checkDue(now: Long, lastAttempt: Long) =
    lastAttempt <= 0 || now < lastAttempt || now - lastAttempt >= CHECK_INTERVAL_MS

internal fun mayDownload(unmetered: Boolean, userAsked: Boolean) = unmetered || userAsked
internal fun newer(candidate: Long, installed: Long) = candidate > installed

internal data class UpdateManifest(
    val versionCode: Long,
    val versionName: String,
    val size: Long,
    val sha256: String,
    val fileName: String,
    val minSdk: Int,
    val signingCertSha256: Set<String>,
) {
    companion object {
        fun parse(raw: String): UpdateManifest {
            require(raw.length <= MAX_INDEX)
            val data = JSONObject(raw)
            // JSONObject's typed getters coerce strings and round fractions; these fields must be integers.
            fun number(key: String): Long = when (val value = data.get(key)) {
                is Int -> value.toLong()
                is Long -> value
                else -> throw IllegalArgumentException("Expected integer: $key")
            }
            fun text(key: String): String = data.get(key) as? String ?: throw IllegalArgumentException("Expected text: $key")
            val code = number("versionCode").also { require(it in 1..2_100_000_000) }
            val name = text("versionName").also { require(Regex("(?:0|[1-9][0-9]*)\\.(?:0|[1-9][0-9]*)\\.(?:0|[1-9][0-9]*)-build\\.$code").matches(it)) }
            val file = text("fileName").also { require(it == "todo-android-$name.apk") }
            val size = number("size").also { require(it in 1..MAX_APK) }
            val sdk = number("minSdk").also { require(it in 29..100) }
            val hash = text("sha256").also { require(digestPattern.matches(it)) }
            val certs = data.getJSONArray("signingCertSha256").let { list ->
                require(list.length() in 1..16)
                List(list.length()) { index -> (list.get(index) as? String).orEmpty().also { require(digestPattern.matches(it)) } }
                    .toSet().also { require(it.size == list.length()) }
            }
            return UpdateManifest(code, name, size, hash, file, sdk.toInt(), certs)
        }
    }
}

internal data class ApkIdentity(
    val packageName: String,
    val versionCode: Long,
    val minSdk: Int,
    val signers: Set<String>,
    val history: Set<String>,
)

internal enum class VerificationFailure { SIZE, HASH, PACKAGE, VERSION, SDK, SIGNATURE }

/** A forward rotation must name the installed current signer in the APK's verified history.
 * Merely sharing an ancestor permits a sibling lineage, or a return to an older key. */
internal fun verifyIdentity(manifest: UpdateManifest, installed: ApkIdentity, candidate: ApkIdentity?): VerificationFailure? = when {
    candidate == null || candidate.packageName != installed.packageName -> VerificationFailure.PACKAGE
    !newer(candidate.versionCode, installed.versionCode) || candidate.versionCode != manifest.versionCode -> VerificationFailure.VERSION
    candidate.minSdk != manifest.minSdk -> VerificationFailure.SDK
    candidate.signers.isEmpty() || candidate.signers != manifest.signingCertSha256 -> VerificationFailure.SIGNATURE
    candidate.signers == installed.signers -> null
    installed.signers.size == 1 && candidate.signers.size == 1 && candidate.history.containsAll(installed.signers) -> null
    else -> VerificationFailure.SIGNATURE
}

internal fun sha256(bytes: ByteArray): String = MessageDigest.getInstance("SHA-256").digest(bytes).hex()

// java.util.HexFormat is not available on all supported Android versions.
private fun ByteArray.hex(): String = joinToString("") { "%02x".format(it) }

internal fun verifyApk(
    file: File, manifest: UpdateManifest, installed: ApkIdentity, archive: () -> ApkIdentity?,
): VerificationFailure? {
    if (file.length() != manifest.size) return VerificationFailure.SIZE
    val digest = MessageDigest.getInstance("SHA-256")
    file.inputStream().buffered().use { input ->
        val buffer = ByteArray(DEFAULT_BUFFER_SIZE)
        while (true) {
            val count = input.read(buffer)
            if (count < 0) break
            digest.update(buffer, 0, count)
        }
    }
    if (digest.digest().hex() != manifest.sha256) return VerificationFailure.HASH
    return verifyIdentity(manifest, installed, archive())
}
