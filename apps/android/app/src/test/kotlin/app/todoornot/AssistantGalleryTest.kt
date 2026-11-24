package app.todoornot

import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.ui.unit.dp
import app.cash.paparazzi.DeviceConfig
import app.cash.paparazzi.Paparazzi
import app.todoornot.core.AssistantAccess
import app.todoornot.core.AssistantAccessRow
import app.todoornot.core.AssistantConnection
import app.todoornot.core.AssistantEffect
import app.todoornot.core.AssistantEffectKind
import app.todoornot.core.AssistantField
import app.todoornot.core.AssistantItem
import app.todoornot.core.AssistantPanelState
import app.todoornot.core.AssistantProposal
import app.todoornot.core.AssistantProposalStatus
import app.todoornot.core.AssistantServer
import app.todoornot.core.AssistantTurn
import app.todoornot.core.DateStyle
import app.todoornot.core.displayDate
import app.todoornot.model.Actions
import app.todoornot.model.AssistantUi
import app.todoornot.model.TodoModel
import app.todoornot.ui.AssistantOverlay
import app.todoornot.ui.LocalSheetAtRest
import app.todoornot.ui.MainScreen
import app.todoornot.ui.PaneFold
import com.android.resources.Density
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.cancel
import org.junit.Rule
import org.junit.Test

private val cover = DeviceConfig.PIXEL_5.copy(screenWidth = 824, screenHeight = 1920, xdpi = 320, ydpi = 320,
    density = Density.XHIGH, locale = "en-rUS")
private val inner = cover.copy(screenWidth = 1752, screenHeight = 1940)

class AssistantGalleryTest {
    @get:Rule val paparazzi = Paparazzi(deviceConfig = cover, theme = "android:Theme.Material.Light.NoActionBar", maxPercentDifference = 0.1)

    @Test fun assistant() {
        val scope = CoroutineScope(Dispatchers.Unconfined)
        try {
            fixtureEngine("groceries-reach").use { engine ->
                val model = TodoModel(engine, scope, Dispatchers.Unconfined, now = { FIXTURE_NOW }, start = engine.collection("Home"))
                val room = model.read()
                val streaming = AssistantTurn("streaming", "What’s due next in Home?",
                    "First, check the library return. The house tasks can wait until the weekend.",
                    listOf("Checked permitted lists · 5 ms", "Searched visible tasks · 3 ms", "Waiting for the model…"),
                    null, emptyList(), null, null, false)
                fun date(iso: String) = checkNotNull(displayDate(iso, FIXTURE_NOW, DateStyle.WEEKDAY))
                val edit = AssistantEffect(AssistantEffectKind.EDIT, "Review quarterly budget", "Home paperwork", "clay", "sample-task",
                    listOf(AssistantField("Due", date("2026-11-19"), date("2026-11-23"))))
                val longCreate = AssistantEffect(AssistantEffectKind.CREATE, "Vrátit knihy do knihovny a vyzvednout rezervovaný slovník",
                    "Errands", "rose", null, listOf(AssistantField("Due", null, date("2026-11-23")), AssistantField("Priority", null, "None"),
                        AssistantField("Status", null, "To do"), AssistantField("Position", null, "End of the list")))
                val change = streaming.copy(id = "proposal", prompt = "Move the budget review to Monday, and add a task to return the library books.",
                    prose = "I drafted these changes. Nothing has been saved yet.", stepSummary = "Checked 2 lists · Read 1 task · 1 hidden",
                    proposal = AssistantProposal("proposal", ByteArray(32), AssistantProposalStatus.PENDING, null,
                        listOf(AssistantItem(0u, true, listOf(edit)), AssistantItem(1u, true, listOf(longCreate)))))
                val base = AssistantPanelState(0uL, AssistantServer("https://todo.example.com", "qwen3-14b", true), false,
                    AssistantConnection.UNTESTED, "Private network", null,
                    listOf(
                        AssistantAccessRow("home", "Home", true, false, false, null),
                        AssistantAccessRow("errands", "Errands", false, true, true, AssistantAccess.PRIVATE_ONLY),
                        AssistantAccessRow("paperwork", "Home paperwork", false, true, false, AssistantAccess.HIDDEN),
                        AssistantAccessRow("reading", "Reading", false, false, false, AssistantAccess.ALL_MODELS),
                    ), 1u, false, emptyList(), null)
                val states = listOf(
                    "unconfigured" to base.copy(server = null, destination = null),
                    "empty" to base,
                    "streaming" to base.copy(streaming = true, turns = listOf(streaming)),
                    "proposal" to base.copy(turns = listOf(change.copy(proposal = change.proposal?.copy(
                        items = listOf(AssistantItem(0u, true, listOf(edit))))))),
                    "create" to base.copy(turns = listOf(change.copy(proposal = change.proposal?.copy(
                        items = listOf(AssistantItem(0u, true, listOf(longCreate))))))),
                    "batch" to base.copy(turns = listOf(change)),
                    "settings" to base.copy(connection = AssistantConnection.CONNECTED, support = "Streaming + task tools"),
                )
                for ((posture, config) in listOf("cover" to cover, "inner" to inner)) {
                    paparazzi.unsafeUpdateConfig(deviceConfig = config)
                    for ((name, panel) in states) for (dark in listOf(false, true)) {
                        val wide = config == inner
                        paparazzi.snapshot("${posture}_${name}_${if (dark) "dark" else "light"}") {
                            CompositionLocalProvider(LocalSheetAtRest provides true) {
                                MainScreen(room, Actions.None, dark, wide, assistant = AssistantUi(panel), initialAssistantOpen = true,
                                    fold = if (wide) PaneFold(436.dp, 440.dp) else null,
                                    initialAssistantOverlay = AssistantOverlay.SETTINGS.takeIf { name == "settings" })
                            }
                        }
                    }
                }
            }
        } finally { scope.cancel() }
    }
}
