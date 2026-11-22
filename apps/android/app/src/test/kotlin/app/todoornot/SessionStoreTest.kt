package app.todoornot

import app.todoornot.session.KeyValues
import app.todoornot.session.SecretCipher
import app.todoornot.session.Session
import app.todoornot.session.SessionStore
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

internal class MapValues : KeyValues {
    val map = HashMap<String, String>()
    override fun get(key: String): String? = map[key]
    override fun put(values: Map<String, String?>) {
        values.forEach { (k, v) -> if (v == null) map.remove(k) else map[k] = v }
    }
}

/** A stand-in for the Keystore: XOR with a key byte, plus a check byte so tampering is detected. */
internal class FakeCipher(var key: Byte = 0x5a) : SecretCipher {
    override fun encrypt(plain: ByteArray) = byteArrayOf(key) + plain.map { (it.toInt() xor key.toInt()).toByte() }
    override fun decrypt(sealed: ByteArray): ByteArray {
        require(sealed.isNotEmpty() && sealed[0] == key) { "wrong key" }
        return sealed.drop(1).map { (it.toInt() xor key.toInt()).toByte() }.toByteArray()
    }
}

class SessionStoreTest {
    private val session = Session("https://todo.example.com", "device-1", "tdd_secret-token")

    @Test
    fun the_token_is_stored_only_encrypted_and_reads_back() {
        val values = MapValues()
        val store = SessionStore(values, FakeCipher())
        store.save(session)
        assertFalse(values.map.values.any { it.contains("tdd_") || it.contains("secret") })
        assertEquals(session, store.load())
    }

    @Test
    fun a_token_that_cannot_be_decrypted_counts_as_signed_out() {
        val values = MapValues()
        val cipher = FakeCipher()
        SessionStore(values, cipher).save(session)
        cipher.key = 0x11 // the Keystore key was wiped or replaced
        assertNull(SessionStore(values, cipher).load())
    }

    @Test
    fun clearing_forgets_everything() {
        val values = MapValues()
        val store = SessionStore(values, FakeCipher())
        store.save(session)
        store.clear()
        assertNull(store.load())
        assertEquals(emptyMap<String, String>(), values.map)
    }

    @Test
    fun the_session_never_prints_its_token() {
        assertFalse(session.toString().contains("tdd_"))
    }

    @Test
    fun the_pairing_id_is_kept_and_cleared_with_the_rest() {
        val values = MapValues()
        val store = SessionStore(values, FakeCipher())
        val paired = session.copy(pairingId = "0".repeat(32))
        store.save(paired)
        assertEquals(paired, store.load())
        store.clear()
        assertNull(store.load())
        assertTrue(values.map.isEmpty())
    }
}
