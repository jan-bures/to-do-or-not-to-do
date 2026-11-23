package app.todoornot

import app.todoornot.ui.checkedLabel
import app.todoornot.updates.ApkIdentity
import app.todoornot.updates.CHECK_INTERVAL_MS
import app.todoornot.updates.UpdateManifest
import app.todoornot.updates.VerificationFailure
import app.todoornot.updates.assetUrl
import app.todoornot.updates.httpsUrl
import app.todoornot.updates.indexUrl
import app.todoornot.updates.historyUrl
import app.todoornot.updates.fetchIndex
import app.todoornot.updates.MAX_RELEASE_HISTORY
import app.todoornot.updates.FeedProblem
import app.todoornot.updates.TaggedIndex
import app.todoornot.updates.checkDue
import app.todoornot.updates.mayDownload
import app.todoornot.updates.newer
import app.todoornot.updates.releaseRedirect
import app.todoornot.updates.sha256
import app.todoornot.updates.verifyApk
import app.todoornot.updates.verifyBytes
import java.io.ByteArrayOutputStream
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertNotEquals
import app.todoornot.updates.verifyIdentity
import java.io.File
import kotlinx.coroutines.runBlocking
import java.net.URI
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Rule
import org.junit.Test
import org.junit.rules.TemporaryFolder

class UpdatesTest {
    @get:Rule val temp = TemporaryFolder(File("build"))
    private val body = byteArrayOf(1, 2, 3)
    private val digest = sha256(body)
    private val old = "a".repeat(64)
    private val rotated = "b".repeat(64)
    private val sibling = "c".repeat(64)
    private val raw = """{"versionCode":12,"versionName":"0.1.0-build.12","size":3,"fileName":"todo-android-0.1.0-build.12.apk","minSdk":29,"sha256":"$digest","signingCertSha256":["$rotated"]}"""
    private val index get() = UpdateManifest.parse(raw)
    private val own = ApkIdentity("app.todoornot.android", 1, 29, setOf(old), setOf(old))
    private val candidate = own.copy(versionCode = 12, signers = setOf(rotated), history = setOf(old, rotated))

    @Test fun only_strictly_higher_version_codes_are_updates() {
        assertTrue(newer(336, 1))
        assertFalse(newer(336, 336))
        assertFalse(newer(335, 336))
    }

    @Test fun the_manifest_validates_exact_file_names_types_sizes_and_digests() {
        assertEquals(UpdateManifest(12, "0.1.0-build.12", 3, digest, "todo-android-0.1.0-build.12.apk", 29, setOf(rotated)), index)
        listOf(
            raw.replace("\"versionCode\":12", "\"versionCode\":\"12\""),
            raw.replace("\"size\":3", "\"size\":3.1"),
            raw.replace("\"size\":3", "\"size\":67108865"),
            raw.replace("build.12", "build.11"),
            raw.replace("todo-android-0.1.0-build.12.apk", "../todo-android-0.1.0-build.12.apk"),
            raw.replace("todo-android-0.1.0-build.12.apk", "https://todo.example.com/x.apk"),
            raw.replace("\"minSdk\":29", "\"minSdk\":26"),
            raw.replace(digest, "short"),
            raw.replace("[\"$rotated\"]", "[]"),
            raw.replace("[\"$rotated\"]", "[\"$rotated\",\"$rotated\"]"),
        ).forEach { assertTrue(it, runCatching { UpdateManifest.parse(it) }.isFailure) }
    }

    @Test fun apk_size_is_capped_and_the_failure_key_names_exact_bytes() {
        assertEquals(67_108_864L, UpdateManifest.parse(raw.replace("\"size\":3", "\"size\":67108864")).size)
        assertEquals("12:$digest", index.fingerprint)
        assertEquals(index.fingerprint, index.copy(versionName = "ignored change").fingerprint)
        assertNotEquals(index.fingerprint, index.copy(sha256 = old).fingerprint)
        assertNotEquals(index.fingerprint, index.copy(versionCode = 13).fingerprint)
    }

    @Test fun staging_verifies_the_exact_bytes_copied_and_does_not_write_excess_bytes() {
        val exact = ByteArrayOutputStream()
        assertNull(verifyBytes(body.inputStream(), index, exact))
        assertArrayEquals(body, exact.toByteArray())
        assertEquals(VerificationFailure.HASH, verifyBytes(byteArrayOf(1, 2, 4).inputStream(), index, ByteArrayOutputStream()))
        assertEquals(VerificationFailure.SIZE, verifyBytes(byteArrayOf(1, 2).inputStream(), index, ByteArrayOutputStream()))
        val oversized = ByteArrayOutputStream()
        assertEquals(VerificationFailure.SIZE, verifyBytes(byteArrayOf(1, 2, 3, 4).inputStream(), index, oversized))
        assertTrue(oversized.size() <= index.size)
    }

    @Test fun asset_urls_are_public_https_and_version_pinned() {
        val base = "https://todo.example.com/owner/repo"
        assertEquals("$base/releases.atom", historyUrl("$base/").toString())
        assertEquals("$base/releases/download/v0.1.0-build.12/android-update.json", indexUrl("$base/", "v0.1.0-build.12").toString())
        assertEquals("$base/releases/download/v0.1.0-build.12/todo-android-0.1.0-build.12.apk", assetUrl(base, index).toString())
        assertTrue(httpsUrl(URI("https://todo.example.com/download?temporary=1")))
        assertFalse(httpsUrl(URI("http://todo.example.com/download")))
        assertFalse(httpsUrl(URI("https://credential@todo.example.com/download")))
    }

    @Test fun redirects_allow_the_configured_host_and_known_github_release_hosts() {
        val initial = URI("https://TODO.EXAMPLE.COM/owner/repo/releases.atom")
        var current = initial
        for ((location, expectedHost) in listOf(
            "/redirect" to "TODO.EXAMPLE.COM",
            "https://GITHUB.COM:443/owner/repo/releases/download/tag/asset" to "GITHUB.COM",
            "//objects.githubusercontent.com/asset" to "objects.githubusercontent.com",
            "https://release-assets.githubusercontent.com/asset?temporary=1" to "release-assets.githubusercontent.com",
            "https://todo.example.com/asset" to "todo.example.com",
        )) {
            current = requireNotNull(releaseRedirect(initial, current, location))
            assertEquals(expectedHost, current.host)
        }
    }

    @Test fun redirects_reject_other_hosts_ports_and_non_https_hops_even_after_a_trusted_hop() {
        val initial = URI("https://todo.example.com/owner/repo/releases.atom")
        val current = URI("https://release-assets.githubusercontent.com/asset")
        for (location in listOf(
            "https://untrusted.example.com/asset",
            "https://release-assets.githubusercontent.com.example.com/asset",
            "https://raw.githubusercontent.com/asset",
            "http://release-assets.githubusercontent.com/asset",
            "https://you@github.com/asset",
            "https://github.com:8443/x",
            "https://todo.example.com:8443/x",
            "https://GITHUB.COM./x",
            "https://192.0.2.1/x",
            "https://[2001:db8::1]/x",
        )) {
            assertNull(location, releaseRedirect(initial, current, location))
        }
    }

    @Test fun redirects_allow_an_ip_literal_only_when_it_matches_the_configured_host() {
        for ((base, host) in listOf("192.0.2.1" to "192.0.2.1", "[2001:DB8::1]" to "[2001:db8::1]")) {
            val initial = URI("https://$base/fork/releases.atom")
            val current = URI("https://github.com/asset")
            val target = URI("https://$host:443/x")
            assertEquals(target, releaseRedirect(initial, current, target.toString()))
            assertNull(releaseRedirect(initial, current, "https://$host:8443/x"))
        }
    }

    private fun history(vararg heights: Int) = """<feed xmlns="http://www.w3.org/2005/Atom">${heights.joinToString("") {
        "<entry><id>tag:github.com,2008:Repository/123/v0.1.0-build.$it</id></entry>"
    }}</feed>"""

    @Test fun android_discovery_skips_desktop_only_tags_and_caps_missing_manifest_probes() = runBlocking {
        val base = "https://todo.example.com/owner/repo"
        val xml = history(12, 9, 11, 15, 13, 14, 14)
        for (available in listOf(true, false)) {
            val calls = mutableListOf<URI>()
            val found = fetchIndex(base) { url, limit ->
                calls += url
                if (url == historyUrl(base)) {
                    assertEquals(MAX_RELEASE_HISTORY, limit)
                    xml
                } else if (available && url == indexUrl(base, "v0.1.0-build.12")) raw else null
            }
            val expected = if (available) listOf(15, 14, 13, 12) else listOf(15, 14, 13, 12, 11)
            assertEquals(listOf(historyUrl(base)) + expected.map { indexUrl(base, "v0.1.0-build.$it") }, calls)
            assertEquals(if (available) TaggedIndex("v0.1.0-build.12", raw) else null, found)
        }
    }

    @Test fun android_discovery_rejects_malformed_or_oversized_atom() = runBlocking {
        val base = "https://todo.example.com/owner/repo"
        for (bad in listOf("<feed>", "<feed/>", "<!DOCTYPE feed>" + history(12), " ".repeat(MAX_RELEASE_HISTORY + 1))) {
            val result = runCatching { fetchIndex(base) { url, _ ->
                assertEquals(historyUrl(base), url)
                bad
            } }
            assertTrue(result.exceptionOrNull() is FeedProblem)
        }
    }

    @Test fun verification_covers_size_hash_identity_version_sdk_and_signature() {
        val file = temp.newFile("download.apk").apply { writeBytes(body) }
        assertNull(verifyApk(file, index, own) { candidate })
        assertEquals(VerificationFailure.SIZE, verifyApk(file, index.copy(size = 2), own) { error("don't parse") })
        assertEquals(VerificationFailure.HASH, verifyApk(file, index.copy(sha256 = old), own) { error("don't parse") })
        assertEquals(VerificationFailure.PACKAGE, verifyApk(file, index, own) { null })
        assertEquals(VerificationFailure.PACKAGE, verifyIdentity(index, own, candidate.copy(packageName = "app.other")))
        assertEquals(VerificationFailure.VERSION, verifyIdentity(index, own, candidate.copy(versionCode = 1)))
        assertEquals(VerificationFailure.VERSION, verifyIdentity(index, own, candidate.copy(versionCode = 13)))
        assertEquals(VerificationFailure.SDK, verifyIdentity(index, own, candidate.copy(minSdk = 30)))
        assertEquals(VerificationFailure.SIGNATURE, verifyIdentity(index, own, candidate.copy(history = setOf(rotated))))
        assertEquals(VerificationFailure.SIGNATURE, verifyIdentity(index.copy(signingCertSha256 = setOf(old)), own, candidate))
    }

    @Test fun a_debug_key_can_rotate_forward_but_not_reverse_or_to_a_sibling() {
        assertNull(verifyIdentity(index, own, candidate))
        assertNull(verifyIdentity(index.copy(signingCertSha256 = setOf(old)), own, own.copy(versionCode = 12)))
        val installed = candidate.copy(versionCode = 11)
        assertEquals(VerificationFailure.SIGNATURE, verifyIdentity(index.copy(signingCertSha256 = setOf(old)), installed, own.copy(versionCode = 12)))
        val fork = candidate.copy(signers = setOf(sibling), history = setOf(old, sibling))
        assertEquals(VerificationFailure.SIGNATURE, verifyIdentity(index.copy(signingCertSha256 = setOf(sibling)), installed, fork))
    }

    @Test fun startup_gate_and_network_policy_are_independent_of_manual_actions() {
        assertTrue(checkDue(100, 0))
        assertFalse(checkDue(100 + CHECK_INTERVAL_MS - 1, 100))
        assertTrue(checkDue(100 + CHECK_INTERVAL_MS, 100))
        assertTrue(checkDue(90, 100)) // clock set back
        assertFalse(mayDownload(unmetered = false, userAsked = false))
        assertTrue(mayDownload(unmetered = false, userAsked = true))
        assertTrue(mayDownload(unmetered = true, userAsked = false))
        assertEquals("Not checked yet", checkedLabel(1_000, 0))
        assertEquals("Checked 8 min ago", checkedLabel(481_000, 1_000))
    }
}
