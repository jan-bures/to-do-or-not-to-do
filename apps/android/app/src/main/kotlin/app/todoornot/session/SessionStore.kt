package app.todoornot.session

import java.util.Base64

/** A key-value store for small strings (SharedPreferences in the app, a map in tests). */
interface KeyValues {
    fun get(key: String): String?

    /** Writes all `values` at once; a `null` value removes the key. */
    fun put(values: Map<String, String?>)
}

/**
 * A paired server: where it is, who this device is, and the device's token. [pairingId] names this
 * pairing's data directory ([PairingFiles]); it is empty until the pairing is saved.
 */
data class Session(val server: String, val deviceId: String, val token: String, val pairingId: String = "") {
    // Never print the token.
    override fun toString(): String = "Session(server=$server, deviceId=$deviceId, pairingId=$pairingId)"
}

/**
 * Keeps the pairing. The server address and device ID are stored as they are; the token only encrypted
 * with [cipher] (Android Keystore). A token that can't be decrypted (the key was wiped, the data was
 * altered) counts as signed out, so the app asks to pair again instead of crashing.
 */
class SessionStore(private val store: KeyValues, private val cipher: SecretCipher) {
    fun load(): Session? {
        val server = store.get(SERVER) ?: return null
        val device = store.get(DEVICE) ?: return null
        val sealed = store.get(TOKEN) ?: return null
        val token = runCatching {
            String(cipher.decrypt(Base64.getDecoder().decode(sealed)), Charsets.UTF_8)
        }.getOrNull() ?: return null
        return Session(server, device, token, store.get(PAIRING) ?: "")
    }

    fun save(session: Session) {
        val sealed = Base64.getEncoder().encodeToString(cipher.encrypt(session.token.toByteArray(Charsets.UTF_8)))
        store.put(mapOf(SERVER to session.server, DEVICE to session.deviceId, TOKEN to sealed, PAIRING to session.pairingId))
    }

    fun clear() {
        store.put(mapOf(SERVER to null, DEVICE to null, TOKEN to null, PAIRING to null))
    }

    private companion object {
        const val SERVER = "server"
        const val DEVICE = "device_id"
        const val TOKEN = "token_sealed"
        const val PAIRING = "pairing_id"
    }
}
