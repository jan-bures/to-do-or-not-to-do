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

    /**
     * Android fetches OCSP responses and CRLs over plain http when the platform verifier checks the server
     * certificate; blocking them made every certificate look revoked. Only those hosts are allowed.
     */
    private val revocationHosts = setOf("lencr.org", "pki.goog", "ssl.com", "sectigo.com", "usertrust.com", "digicert.com")

    private fun List<Element>.revocation() = filter { it.textContent.trim() in revocationHosts }

    @Test
    fun release_allows_plain_http_only_to_certificate_revocation_hosts() {
        val c = config("main")
        assertFalse(c.children("base-config").single().allowsCleartext())
        val domains = c.cleartextDomains()
        assertEquals(revocationHosts, domains.map { it.textContent.trim() }.toSet())
        val manifest = File("src/main/AndroidManifest.xml").readText()
        assertTrue(manifest.contains("android:networkSecurityConfig=\"@xml/network_security_config\""))
    }

    @Test
    fun debug_allows_plain_http_only_to_loopback_and_the_same_revocation_hosts() {
        val c = config("debug")
        assertFalse(c.children("base-config").single().allowsCleartext())
        val domains = c.cleartextDomains()
        assertEquals(
            setOf("localhost", "127.0.0.1") + revocationHosts,
            domains.map { it.textContent.trim() }.toSet(),
        )
        val loopback = domains - domains.revocation().toSet()
        assertTrue(loopback.none { it.getAttribute("includeSubdomains") == "true" })
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
