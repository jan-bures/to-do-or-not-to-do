package app.todoornot

import android.content.ClipData
import android.content.ClipboardManager
import android.content.Intent
import android.net.Uri
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.material3.adaptive.currentWindowAdaptiveInfoV2
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.window.core.layout.WindowSizeClass
import app.todoornot.model.PairingModel
import app.todoornot.session.Host
import app.todoornot.ui.CantOpenScreen
import app.todoornot.ui.MainScreen
import app.todoornot.ui.PairingActions
import app.todoornot.ui.PairingScreen

class MainActivity : ComponentActivity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        enableEdgeToEdge()
        val graph = (application as TodoApplication).graph
        // Share sheet (text): prefill the composer. Saving it is still the owner's tap.
        val shared = intent?.takeIf { it.action == Intent.ACTION_SEND }?.getStringExtra(Intent.EXTRA_TEXT)?.trim().orEmpty()
        setContent { App(graph, shared) }
    }

    @Composable
    private fun App(graph: AppGraph, shared: String) {
        val dark = isSystemInDarkTheme()
        // Unfolded (medium width and up): list and detail side by side.
        val wide = currentWindowAdaptiveInfoV2().windowSizeClass.isWidthAtLeastBreakpoint(WindowSizeClass.WIDTH_DP_MEDIUM_LOWER_BOUND)
        val model by graph.model.collectAsStateWithLifecycle()
        val host by graph.host.state.collectAsStateWithLifecycle()
        val m = model
        val h = host
        when {
            m != null -> {
                val ui by m.ui.collectAsStateWithLifecycle()
                MainScreen(ui, m, dark, wide, server = (h as? Host.Ready)?.session?.server, draft = shared, motion = m.motion)
            }
            h is Host.CantOpen -> {
                val problem by graph.unpairProblem.collectAsStateWithLifecycle()
                CantOpenScreen(h.problem, h.session.server, dark, graph::retry, graph::unpair, problem)
            }
            else -> {
                val state by graph.pairing.state.collectAsStateWithLifecycle()
                PairingScreen(state, pairingActions(graph.pairing), dark)
            }
        }
    }

    private fun pairingActions(p: PairingModel) = object : PairingActions {
        override fun submit(address: String) = p.submit(address)
        override fun cancel() = p.cancel()
        override fun openLink(url: String) {
            startActivity(Intent(Intent.ACTION_VIEW, Uri.parse(url)))
        }

        override fun copyLink(url: String) {
            getSystemService(ClipboardManager::class.java)?.setPrimaryClip(ClipData.newPlainText("Pairing link", url))
        }
    }
}
