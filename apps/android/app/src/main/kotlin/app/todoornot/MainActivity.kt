package app.todoornot

import android.Manifest
import android.content.BroadcastReceiver
import android.content.ClipData
import android.content.ClipboardManager
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.net.Uri
import android.os.Build
import android.os.Bundle
import android.provider.Settings
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.SideEffect
import androidx.compose.runtime.derivedStateOf
import androidx.core.content.ContextCompat
import androidx.core.view.WindowCompat
import androidx.lifecycle.compose.LifecycleStartEffect
import androidx.lifecycle.lifecycleScope
import kotlinx.coroutines.flow.filterNotNull
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.launch
import kotlinx.coroutines.withTimeoutOrNull
import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.material3.adaptive.currentWindowAdaptiveInfoV2
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.ui.Modifier
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.ui.platform.LocalDensity
import androidx.window.layout.FoldingFeature
import androidx.window.layout.WindowInfoTracker
import app.todoornot.session.PrefsKeyValues
import app.todoornot.session.ThumbHandPreference
import app.todoornot.ui.PaneFold
import androidx.compose.runtime.setValue
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.window.core.layout.WindowSizeClass
import app.todoornot.core.RoomKey
import app.todoornot.design.TodoTheme
import app.todoornot.model.DevicePairingModel
import app.todoornot.model.PairingModel
import app.todoornot.model.TodoModel
import app.todoornot.push.PushStatus
import app.todoornot.reminders.AndroidAlarms
import app.todoornot.reminders.ReminderAccess
import app.todoornot.reminders.ReminderNeed
import app.todoornot.session.Host
import app.todoornot.session.signedOut
import app.todoornot.ui.CantOpenScreen
import app.todoornot.ui.LocalPushStatus
import app.todoornot.ui.LocalReminderAccess
import app.todoornot.ui.LocalPairingOwner
import app.todoornot.ui.LocalScanPairing
import app.todoornot.ui.MainScreen
import app.todoornot.ui.LocalUpdates
import app.todoornot.ui.UpdateControls
import app.todoornot.ui.UpdatesSheet
import app.todoornot.ui.LocalSheetWidth
import app.todoornot.ui.listPaneWidth
import app.todoornot.updates.SHOW_UPDATES
import app.todoornot.updates.UpdateMode
import app.todoornot.ui.PairingActions
import app.todoornot.ui.PairingScreen
import app.todoornot.widget.WIDGET_ADD
import app.todoornot.widget.WIDGET_ROOM
import app.todoornot.widget.widgetChoices
import app.todoornot.widget.widgetRoom
import kotlinx.coroutines.Job

class MainActivity : ComponentActivity() {
    private val graph get() = (application as TodoApplication).graph
    private var routeJob: Job? = null
    private var updateRequest by mutableStateOf(0)
    private var composerRoom by mutableStateOf<Pair<TodoModel, RoomKey>?>(null)

    /** Android's own question for notifications; whatever the answer, the alarms follow. */
    private val notificationPermission = registerForActivityResult(ActivityResultContracts.RequestPermission()) {
        graph.reminderWork({}) { graph.reconcileReminders() }
    }

    private val pairingScanner = PairingScanner(this) { graph.pairingScan }

    private fun scanApproval(owner: DevicePairingModel) {
        val graph = graph // callbacks retained by the graph must not capture this Activity
        pairingScanner.launch(
            { code -> if (graph.devicePairing === owner) owner.review(code) },
            { if (graph.devicePairing === owner) owner.cameraDenied() },
        )
    }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        enableEdgeToEdge()
        val graph = graph
        // Share sheet (text): prefill the composer. Saving it is still the owner's tap.
        val shared = intent?.takeIf { it.action == Intent.ACTION_SEND }?.getStringExtra(Intent.EXTRA_TEXT)?.trim().orEmpty()
        if (savedInstanceState == null) openExternal(intent)
        val access = Access()
        setContent {
            CompositionLocalProvider(LocalReminderAccess provides access) { App(graph, shared) }
        }
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        openExternal(intent)
    }

    /** A reminder or widget opens a detail; the widget's + opens and focuses the room's real composer. */
    private fun openExternal(intent: Intent?) {
        if (intent?.action == SHOW_UPDATES) {
            updateRequest++
            return
        }
        routeJob?.cancel()
        composerRoom = null
        val task = intent?.getStringExtra(EXTRA_TASK)
        val room = intent?.takeIf { it.action == WIDGET_ADD }?.getStringExtra(WIDGET_ROOM).let(::widgetRoom)
        if (task == null && room == null) return
        routeJob = lifecycleScope.launch {
            val model = withTimeoutOrNull(MODEL_WAIT_MS) { graph.model.filterNotNull().first() } ?: return@launch
            if (task != null) {
                model.select(task)
            } else if (room != null && widgetChoices(model.ui.value.home).any { it.item.key == room }) {
                model.open(room)
                composerRoom = model to room
            }
        }
    }

    private fun clearComposerRequest() { composerRoom = null }

    /** Asks for what reminders need, each once ([ReminderAccess]); the answers are kept on this phone. */
    private inner class Access : ReminderAccess {
        private val prefs = getSharedPreferences("reminder_access", Context.MODE_PRIVATE)

        private fun asked(): Set<ReminderNeed> = ReminderNeed.entries.filter { prefs.getBoolean(it.name, false) }.toSet()

        private fun remember(need: ReminderNeed) {
            prefs.edit().putBoolean(need.name, true).apply()
        }

        override fun toAsk(): ReminderNeed? = ReminderAccess.next(
            Build.VERSION.SDK_INT,
            AndroidAlarms.notificationsAllowed(this@MainActivity),
            AndroidAlarms.exactAllowed(this@MainActivity),
            asked(),
        )

        override fun allow(need: ReminderNeed) {
            remember(need)
            when (need) {
                ReminderNeed.NOTIFICATIONS -> if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
                    notificationPermission.launch(Manifest.permission.POST_NOTIFICATIONS)
                }
                ReminderNeed.EXACT_ALARMS -> if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
                    runCatching {
                        startActivity(Intent(Settings.ACTION_REQUEST_SCHEDULE_EXACT_ALARM, Uri.parse("package:$packageName")))
                    }
                }
            }
        }

        override fun decline(need: ReminderNeed) = remember(need)
    }

    @Composable
    private fun App(graph: AppGraph, shared: String) {
        val dark = isSystemInDarkTheme()
        // Unfolded (medium width and up): list and detail side by side.
        val wide = currentWindowAdaptiveInfoV2().windowSizeClass.isWidthAtLeastBreakpoint(WindowSizeClass.WIDTH_DP_MEDIUM_LOWER_BOUND)
        val layout by remember { WindowInfoTracker.getOrCreate(this).windowLayoutInfo(this) }
            .collectAsStateWithLifecycle(initialValue = null)
        val density = LocalDensity.current
        val fold = layout?.displayFeatures?.filterIsInstance<FoldingFeature>()?.firstOrNull {
            it.isSeparating && it.orientation == FoldingFeature.Orientation.VERTICAL
        }?.bounds?.let { bounds -> with(density) { PaneFold(bounds.left.toDp(), bounds.right.toDp()) } }
        val thumbPreference = remember { ThumbHandPreference(PrefsKeyValues(this, "appearance")) }
        var hand by remember { mutableStateOf(thumbPreference.load()) }
        val loaded by graph.loaded.collectAsStateWithLifecycle()
        val model by graph.model.collectAsStateWithLifecycle()
        val host by graph.host.state.collectAsStateWithLifecycle()
        val m = model
        val h = host
        val ready = h as? Host.Ready
        val ui by m?.ui?.collectAsStateWithLifecycle() ?: remember { mutableStateOf(null) }
        val update by graph.updates.state.collectAsStateWithLifecycle()
        val updates = if (graph.updates.enabled) UpdateControls(
            update,
            check = { graph.requestUpdate(UpdateMode.CHECK_NOW) },
            download = { update.offer?.let { graph.requestUpdate(UpdateMode.DOWNLOAD_NOW, it.versionCode) } },
            install = { update.offer?.let { graph.installUpdate(it.versionCode) } },
            settings = { runCatching { startActivity(Intent(Settings.ACTION_MANAGE_UNKNOWN_APP_SOURCES, Uri.parse("package:$packageName"))) } },
            confirm = { graph.updates.openConfirmation(::startActivity) },
            dismiss = { update.offer?.let { graph.dismissUpdate(it.versionCode) } },
            openRequest = updateRequest,
            opened = { updateRequest = 0 },
        ) else null
        var standaloneUpdates by rememberSaveable { mutableStateOf(false) }
        val standalone = loaded && m == null && h !is Host.Ready
        LaunchedEffect(standalone, updates?.openRequest) {
            if (standalone && updates != null && updates.openRequest > 0) {
                standaloneUpdates = true
                updates.opened()
            }
        }
        val nightHeader by remember(m, ui, wide) { derivedStateOf {
            val s = ui ?: return@derivedStateOf false
            if (s.activity != null || (!wide && s.detail != null)) return@derivedStateOf false
            m?.motion?.dusk(s.room.key)?.nightInk ?: s.room.finished
        } }
        SideEffect {
            WindowCompat.getInsetsController(window, window.decorView).isAppearanceLightStatusBars = !dark && !nightHeader
        }
        app.todoornot.ui.AttachmentPickers(m?.takeIf { it.engine === ready?.engine }, ready?.session?.pairingId, graph::discardDocument) {
            when {
                m != null && ui != null -> {
                    val screen = requireNotNull(ui)
                    val assistant = graph.assistant?.takeIf { it.first === m.engine }?.second
                    val ai = assistant?.ui?.collectAsStateWithLifecycle()?.value
                    val push by graph.push.status.collectAsStateWithLifecycle()
                    HeaderClock(m)
                    val focus = composerRoom?.takeIf { it.first === m && it.second == screen.room.key && screen.activity == null }
                    CompositionLocalProvider(
                        LocalPairingOwner provides graph.devicePairing?.takeIf { m.engine === ready?.engine },
                        LocalScanPairing provides ::scanApproval,
                        LocalPushStatus provides (if (screen.status.signedOut) PushStatus() else push),
                        LocalUpdates provides updates,
                    ) {
                        MainScreen(
                            screen, m, dark, wide, server = ready?.session?.server, draft = shared, motion = m.motion,
                            composerFocus = if (focus != null) ::clearComposerRequest else null,
                            hand = hand, onHand = { hand = it; thumbPreference.save(it) }, fold = fold,
                            assistant = ai, assistantModel = assistant,
                        )
                    }
                }
                h is Host.CantOpen -> {
                    val problem by graph.unpairProblem.collectAsStateWithLifecycle()
                    CantOpenScreen(h.problem, h.session.server, dark, graph::retry, graph::unpair, problem)
                }
                !loaded || h is Host.Ready -> Unit // The cold-start window already uses the room's background.
                else -> {
                    val state by graph.pairing.state.collectAsStateWithLifecycle()
                    CompositionLocalProvider(LocalUpdates provides updates) {
                        PairingScreen(state, pairingActions(graph.pairing), dark, onOpenUpdates = { standaloneUpdates = true })
                    }
                }
            }
            if (standalone && standaloneUpdates && updates != null) TodoTheme(dark, "my-day") {
                BoxWithConstraints(Modifier.fillMaxSize()) {
                    CompositionLocalProvider(LocalSheetWidth provides listPaneWidth(maxWidth, wide, fold)) {
                        UpdatesSheet(updates) { standaloneUpdates = false }
                    }
                }
            }
        }
    }

    @Composable
    private fun HeaderClock(model: TodoModel) {
        LifecycleStartEffect(model) {
            val receiver = object : BroadcastReceiver() {
                override fun onReceive(context: Context, intent: Intent) {
                    lifecycleScope.launch { model.refreshHeaderClock(force = intent.action != Intent.ACTION_TIME_TICK) }
                }
            }
            val events = IntentFilter().apply {
                addAction(Intent.ACTION_TIME_TICK)
                addAction(Intent.ACTION_TIME_CHANGED)
                addAction(Intent.ACTION_TIMEZONE_CHANGED)
            }
            ContextCompat.registerReceiver(this@MainActivity, receiver, events, ContextCompat.RECEIVER_NOT_EXPORTED)
            lifecycleScope.launch { model.refreshHeaderClock(force = true) }
            onStopOrDispose { unregisterReceiver(receiver) }
        }
    }

    private fun pairingActions(p: PairingModel) = object : PairingActions {
        override fun submit(address: String) = p.submit(address)
        override fun cancel() = p.cancel()
        override fun redeem(address: String, invite: String) = p.redeem(address, invite)
        override fun confirmInvite() = p.confirmInvite()
        override fun retrySave() = p.retrySave()
        override fun discardSave() = p.discardSave()
        override fun scanInvite(address: String) {
            val graph = graph
            pairingScanner.launch(
                { code -> if (graph.host.state.value is Host.SignedOut) p.redeem(address, code) },
                { if (graph.host.state.value is Host.SignedOut) p.cameraDenied(address) },
            )
        }
        override fun openLink(url: String) {
            startActivity(Intent(Intent.ACTION_VIEW, Uri.parse(url)))
        }

        override fun copyLink(url: String) {
            getSystemService(ClipboardManager::class.java)?.setPrimaryClip(ClipData.newPlainText("Pairing link", url))
        }
    }

    companion object {
        /** The task a reminder's notification opens. */
        const val EXTRA_TASK = "app.todoornot.TASK"

        /** How long a notification's tap waits for the engine to open. */
        const val MODEL_WAIT_MS = 5_000L
    }
}
