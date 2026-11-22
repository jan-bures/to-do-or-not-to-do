package app.todoornot

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test
import org.w3c.dom.Element
import java.io.File
import javax.xml.parsers.DocumentBuilderFactory

/**
 * Plain http is a debug-only allowance for loopback. The Rust core enforces it (see `todo-ffi`'s tests);
 * these check the platform side: the network security config of each build and the flag the app passes.
 */
class CleartextTest {
    private fun config(sourceSet: String): Element {
        val file = File("src/$sourceSet/res/xml/network_security_config.xml")
        assertTrue("$file is missing", file.isFile)
        return DocumentBuilderFactory.newInstance().newDocumentBuilder().parse(file).documentElement
    }

    private fun Element.children(tag: String): List<Element> =
        (0 until childNodes.length).map { childNodes.item(it) }.filterIsInstance<Element>().filter { it.tagName == tag }

    private fun Element.allowsCleartext() = getAttribute("cleartextTrafficPermitted") == "true"

    private fun Element.cleartextDomains(): List<Element> =
        children("domain-config").filter { it.allowsCleartext() }.flatMap { it.children("domain") }

    /** Each allowed domain with whether it includes subdomains. */
    private fun Element.cleartextPolicy(): Set<Pair<String, Boolean>> =
        cleartextDomains().map { it.textContent.trim() to (it.getAttribute("includeSubdomains") == "true") }.toSet()

    /** Whether the policy lets plain http reach [host], the way Android matches domain-config entries. */
    private fun Set<Pair<String, Boolean>>.permits(host: String) =
        any { (domain, subdomains) -> host == domain || (subdomains && host.endsWith(".$domain")) }

    /**
     * Android fetches OCSP responses and CRLs over plain http when the platform verifier checks the server
     * certificate; blocking them made every certificate look revoked. Only those responder hosts are allowed.
     */
    private val revocationPolicy = setOf(
        "c.lencr.org" to true,
        "c.pki.goog" to false,
        "o.pki.goog" to false,
        "c.cf-b.ssl.com" to false,
        "c.cf-i.ssl.com" to false,
        "ocsps.ssl.com" to false,
        "crl.sectigo.com" to false,
        "ocsp.sectigo.com" to false,
    )

    private fun assertRevocationOnly(policy: Set<Pair<String, Boolean>>) {
        // The CRL host of a current Let's Encrypt certificate, which the pairing failure was about.
        assertTrue(policy.permits("ye1.c.lencr.org"))
        assertTrue(policy.permits("o.pki.goog"))
        for (sibling in listOf("lencr.org", "www.lencr.org", "pki.goog", "www.ssl.com", "sectigo.com", "todo.example.com")) {
            assertFalse("$sibling must stay HTTPS only", policy.permits(sibling))
        }
    }

    @Test
    fun release_allows_plain_http_only_to_certificate_revocation_hosts() {
        val c = config("main")
        assertFalse(c.children("base-config").single().allowsCleartext())
        assertEquals(revocationPolicy, c.cleartextPolicy())
        assertRevocationOnly(c.cleartextPolicy())
        val manifest = File("src/main/AndroidManifest.xml").readText()
        assertTrue(manifest.contains("android:networkSecurityConfig=\"@xml/network_security_config\""))
    }

    @Test
    fun debug_allows_plain_http_only_to_loopback_and_the_same_revocation_hosts() {
        val c = config("debug")
        assertFalse(c.children("base-config").single().allowsCleartext())
        assertEquals(setOf("localhost" to false, "127.0.0.1" to false) + revocationPolicy, c.cleartextPolicy())
        assertRevocationOnly(c.cleartextPolicy() - setOf("localhost" to false, "127.0.0.1" to false))
    }

    /** The release side is checked by Gradle (`verifyReleaseCleartextPolicy`), as unit tests run against debug. */
    @Test
    fun the_core_is_told_to_allow_loopback_http_only_in_the_debug_build_type() {
        assertEquals(BuildConfig.BUILD_TYPE == "debug", BuildConfig.ALLOW_LOOPBACK_HTTP)
        assertEquals(BuildConfig.ALLOW_LOOPBACK_HTTP, Cleartext.allowLoopbackHttp)
    }

    @Test
    fun only_the_debug_build_type_turns_the_allowance_on() {
        val script = File("build.gradle.kts").readText()
        val fields = Regex("""buildConfigField\("boolean", "ALLOW_LOOPBACK_HTTP", "(true|false)"\)""").findAll(script).toList()
        assertEquals(2, fields.size)
        // The default (defaultConfig) is off; the one place it's on is inside `debug { }`.
        val on = fields.single { it.groupValues[1] == "true" }
        val before = script.substring(0, on.range.first)
        assertTrue(before.trimEnd().endsWith("debug {"))
        assertEquals("false", fields.first().groupValues[1])
        assertTrue(script.indexOf("defaultConfig {") < fields.first().range.first)
    }
}
