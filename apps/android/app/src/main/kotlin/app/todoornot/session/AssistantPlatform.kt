package app.todoornot.session

import android.content.Context
import android.net.ConnectivityManager
import android.net.NetworkCapabilities
import app.todoornot.core.AssistantPlatform

/** No task data or pairing token ever reaches the model transport through this bridge. */
internal class AndroidAssistantPlatform(context: Context, pairingId: String) : AssistantPlatform {
    private val connectivity = context.getSystemService(ConnectivityManager::class.java)
    private val cipher = KeystoreCipher(alias(pairingId))

    override fun sealKey(plain: ByteArray): ByteArray? = try {
        cipher.encrypt(plain)
    } catch (_: Exception) {
        null
    } finally {
        plain.fill(0)
    }

    override fun openKey(sealed: ByteArray): ByteArray? = try {
        cipher.decrypt(sealed)
    } catch (_: Exception) {
        null
    }

    // Check the VPN available to this app, not just the presence of a Tailscale install or any
    // carrier CGNAT address. Rust checks the address range again before every HTTP model request.
    override fun vpnAddresses(): List<String> = try {
        val network = connectivity?.activeNetwork
        if (network == null || connectivity.getNetworkCapabilities(network)?.hasTransport(NetworkCapabilities.TRANSPORT_VPN) != true) {
            emptyList()
        } else {
            connectivity.getLinkProperties(network)?.linkAddresses.orEmpty().mapNotNull {
                it.address.hostAddress?.substringBefore('%')
            }
        }
    } catch (_: SecurityException) {
        emptyList()
    }

    companion object {
        private fun alias(pairingId: String) = "assistant-api-key-$pairingId"

        /** On unpair, even if the OS couldn't remove the old pairing's directory. */
        fun forgetKey(pairingId: String) {
            // The device can already have erased the Keystore. Do not log credential errors.
            runCatching { KeystoreCipher(alias(pairingId)).destroy() }
        }
    }
}
