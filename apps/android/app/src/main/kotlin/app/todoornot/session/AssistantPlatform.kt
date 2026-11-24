package app.todoornot.session

import app.todoornot.core.AssistantPlatform

/** No task data or pairing token ever reaches the model transport through this bridge. */
internal class AndroidAssistantPlatform(private val pairingId: String) : AssistantPlatform {
    private val cipher = KeystoreCipher(alias(pairingId))

    override fun sealKey(plain: ByteArray): ByteArray? = try {
        synchronized(keyLock) {
            if (pairingId in signedOut) null else runCatching { cipher.encrypt(plain) }.getOrNull()
        }
    } finally {
        plain.fill(0)
    }

    override fun openKey(sealed: ByteArray): ByteArray? = synchronized(keyLock) {
        if (pairingId in signedOut) null else runCatching { cipher.decrypt(sealed) }.getOrNull()
    }

    override fun destroyKey(): Boolean = synchronized(keyLock) {
        pairingId !in signedOut && destroy(cipher)
    }

    companion object {
        private val keyLock = Any()
        // Sign-out and key operations share a lock so an in-flight save cannot recreate a revoked key.
        // Cache successes; retry a failed deletion on the next signed-out status or sweep.
        private val signedOut = mutableMapOf<String, Boolean>()
        private fun alias(pairingId: String) = "assistant-api-key-$pairingId"
        private fun destroy(cipher: KeystoreCipher) = runCatching { cipher.destroy() }.isSuccess

        fun forgetKey(pairingId: String): Boolean = synchronized(keyLock) {
            if (signedOut[pairingId] == true) true
            else destroy(KeystoreCipher(alias(pairingId))).also { signedOut[pairingId] = it }
        }
    }
}
