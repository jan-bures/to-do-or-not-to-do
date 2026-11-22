package app.todoornot

import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.requiredSize
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.GenericShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.ui.Alignment
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.ColorFilter
import androidx.compose.ui.graphics.Shape
import androidx.compose.ui.res.painterResource
import kotlin.math.pow
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.layout.onSizeChanged
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.unit.IntSize
import androidx.compose.ui.unit.dp
import app.cash.paparazzi.DeviceConfig
import app.cash.paparazzi.Paparazzi
import app.todoornot.core.FfiException
import app.todoornot.core.RoomKey
import app.todoornot.core.SmartList
import app.todoornot.core.TodoEngine
import app.todoornot.design.LocalDesign
import app.todoornot.design.TodoTheme
import app.todoornot.design.Tokens
import app.todoornot.model.Actions
import app.todoornot.model.PairingState
import app.todoornot.model.TodoModel
import app.todoornot.model.Toast
import app.todoornot.model.UiState
import app.todoornot.session.EngineHost
import app.todoornot.ui.CantOpenScreen
import app.todoornot.ui.MainScreen
import app.todoornot.ui.NavRow
import app.todoornot.ui.PairingActions
import app.todoornot.ui.PairingScreen
import app.todoornot.ui.RoomPill
import app.todoornot.ui.UnpairCard
import com.android.resources.Density
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Rule
import org.junit.Test

/** A foldable's cover screen, 412 × 920 dp (the F2 cover mockup). */
private val COVER = DeviceConfig.PIXEL_5.copy(screenWidth = 824, screenHeight = 1840, xdpi = 320, ydpi = 320, density = Density.XHIGH)

/** The cover screen on its side, 920 × 412 dp: a short window. */
private val COVER_SIDEWAYS = COVER.copy(screenWidth = 1840, screenHeight = 824, orientation = com.android.resources.ScreenOrientation.LANDSCAPE)

/** A foldable's inner screen, 900 × 1000 dp (the F2 inner mockup). */
private val INNER = DeviceConfig.PIXEL_5.copy(screenWidth = 1800, screenHeight = 2000, xdpi = 320, ydpi = 320, density = Density.XHIGH)

/**
 * The screenshot gallery: the main states on the cover and inner screens, light and dark, drawn from the
 * invented fixture data through the real Rust view model. `./gradlew :app:recordPaparazziDebug` writes
 * them to `app/src/test/snapshots/images`; `verifyPaparazziDebug` compares against them.
 */
class GalleryTest {
    @get:Rule
    val paparazzi = Paparazzi(deviceConfig = COVER, theme = "android:Theme.Material.Light.NoActionBar", maxPercentDifference = 0.1)

    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Unconfined)

    @After
    fun tearDown() = scope.cancel()

    private fun state(
        engine: TodoEngine,
        room: RoomKey = RoomKey.Smart(SmartList.MY_DAY),
        select: String? = null,
        toast: Toast? = null,
        filter: String = "",
        fold: List<String> = emptyList(),
    ): UiState {
        val m = TodoModel(engine, scope, Dispatchers.Unconfined, now = { FIXTURE_NOW }, start = room)
        if (select != null) m.select(select)
        if (filter.isNotEmpty()) m.setFilter(filter)
        fold.forEach(m::toggleCollection)
        return m.read().copy(toast = toast)
    }

    private fun shot(name: String, device: DeviceConfig, content: @Composable () -> Unit) {
        paparazzi.unsafeUpdateConfig(deviceConfig = device)
        paparazzi.snapshot(name) { content() }
    }

    private fun main(
        name: String,
        device: DeviceConfig,
        state: UiState,
        dark: Boolean,
        drawer: Boolean = false,
        collapsed: Boolean = false,
        wide: Boolean = device === INNER,
    ) = shot(name, device) {
        MainScreen(state, Actions.None, dark, wide, server = "https://todo.example.com", drawerOpen = drawer, collapsed = collapsed)
    }

    @Test
    fun cover_my_day() {
        val s = state(fixtureEngine())
        main("cover_my_day_light", COVER, s, dark = false)
        main("cover_my_day_dark", COVER, s, dark = true)
    }

    @Test
    fun cover_toast_after_completing() {
        val s = state(fixtureEngine(), toast = Toast(1, "Completed “Koupit rohlíky a mléko”", reopen = "x"))
        main("cover_toast_light", COVER, s, dark = false)
    }

    @Test
    fun cover_detail() {
        val e = fixtureEngine()
        val admin = e.list("Admin")
        val s = state(e, admin, select = e.taskId(admin, "Renew passport appointment"))
        main("cover_detail_light", COVER, s, dark = false)
        main("cover_detail_dark", COVER, s, dark = true)
    }

    /** While typing (the keyboard can't be drawn here, so the collapse is forced): bar, tasks, composer. */
    @Test
    fun collapsed_header() {
        val s = state(fixtureEngine())
        main("cover_collapsed_light", COVER, s, dark = false, collapsed = true)
        main("cover_collapsed_dark", COVER, s, dark = true, collapsed = true)
        val e = fixtureEngine()
        val personal = e.collection("Personal")
        main("inner_collapsed_dark", INNER, state(e, personal, select = e.taskId(personal, "Renew passport appointment")), dark = true, collapsed = true)
    }

    /**
     * Dated and undated rows together: a dated row's chips follow the date and line up down the list, an
     * undated row's chips start flush with its title.
     */
    @Test
    fun dated_and_undated_rows() {
        val e = fixtureEngine()
        val errands = e.list("Errands")
        e.addTask(errands, "Buy stamps", FIXTURE_NOW)
        e.addTask(errands, "Water the plants", FIXTURE_NOW)
        val s = state(e, e.collection("Personal"))
        main("cover_mixed_dates_light", COVER, s, dark = false)
        main("inner_mixed_dates_dark", INNER, s, dark = true)
    }

    /** A short window (the cover screen on its side) collapses the header by itself. */
    @Test
    fun short_window() {
        main("cover_sideways_light", COVER_SIDEWAYS, state(fixtureEngine()), dark = false, wide = false)
    }

    /** At a large font scale the rows grow; the chips under the titles keep their room. */
    @Test
    fun large_text() {
        main("cover_large_text_light", COVER.copy(fontScale = 1.3f), state(fixtureEngine()), dark = false)
    }

    @Test
    fun cover_drawer() {
        main("cover_drawer_light", COVER, state(fixtureEngine()), dark = false, drawer = true)
        val e = fixtureEngine()
        val personal = (e.collection("Personal") as RoomKey.Collection).id
        main("cover_drawer_folded_dark", COVER, state(e, fold = listOf(personal)), dark = true, drawer = true)
    }

    /** The filter under the pills: the query language narrows the rows, the sun still counts the room. */
    @Test
    fun cover_filter() {
        main("cover_filter_light", COVER, state(fixtureEngine(), filter = "priority>=high"), dark = false)
        main("cover_filter_nothing_dark", COVER, state(fixtureEngine(), filter = "dentist tomorrow"), dark = true)
    }

    @Test
    fun inner_personal_with_detail() {
        val e = fixtureEngine()
        val personal = e.collection("Personal")
        val s = state(e, personal, select = e.taskId(personal, "Renew passport appointment"))
        main("inner_personal_dark", INNER, s, dark = true)
        main("inner_personal_light", INNER, s, dark = false)
    }

    @Test
    fun finished_day_hands_over_to_tomorrow() {
        val s = state(fixtureEngine("finished"))
        main("cover_finished_dark", COVER, s, dark = true)
        main("cover_finished_light", COVER, s, dark = false)
        main("inner_finished_light", INNER, s, dark = false)
    }

    @Test
    fun empty_list() {
        val e = fixtureEngine()
        val s = state(e, e.list("Reading list"))
        main("cover_empty_light", COVER, s, dark = false)
        main("cover_empty_dark", COVER, s, dark = true)
    }

    @Test
    fun pairing() {
        shot("pairing_address_light", COVER) {
            PairingScreen(PairingState.Entering(), PairingActions.None, dark = false)
        }
        shot("pairing_error_dark", COVER) {
            PairingScreen(
                PairingState.Entering("http://todo.example.com", "Use https://: this app only talks to servers over HTTPS."),
                PairingActions.None,
                dark = true,
            )
        }
        val waiting = PairingState.Waiting("https://todo.example.com", "WXYZ-2345", "https://todo.example.com/pair?code=WXYZ-2345", 0)
        shot("pairing_code_light", COVER) { PairingScreen(waiting, PairingActions.None, dark = false) }
        shot("pairing_code_dark", COVER) { PairingScreen(waiting, PairingActions.None, dark = true) }
    }

    @Test
    fun cant_open_the_cache() {
        val problem = EngineHost.problem(FfiException.Storage("disk full"))
        shot("cant_open_light", COVER) { CantOpenScreen(problem, "https://todo.example.com", dark = false, {}, {}) }
        shot("cant_open_dark", COVER) { CantOpenScreen(problem, "https://todo.example.com", dark = true, {}, {}) }
    }

    /** The unpair question: with unsent changes (Cancel first, the discard in danger colours), and without. */
    @Test
    fun unpair_question() {
        fun card(unsent: Int?, dark: Boolean, changed: Boolean = false) = @Composable {
            TodoTheme(dark, "my-day") {
                val r = LocalDesign.current.room
                Box(Modifier.fillMaxSize().background(r.toast.copy(alpha = 0.32f)).padding(24.dp), contentAlignment = Alignment.Center) {
                    UnpairCard(unsent, {}, {}, focusCancel = false, changed = changed)
                }
            }
        }
        shot("unpair_with_unsent_light", COVER, card(3, dark = false))
        shot("unpair_nothing_unsent_dark", COVER, card(0, dark = true))
        shot("unpair_count_changed_light", COVER, card(1, dark = false, changed = true))
    }

    /** Drawer rows and room pills look compact, but each is a full touch target. */
    @Test
    fun drawer_rows_and_room_pills_are_full_touch_targets() {
        val home = fixtureEngine().home(FIXTURE_NOW)
        val sizes = mutableMapOf<String, IntSize>()
        var touch = 0
        shot("touch_targets", COVER) {
            touch = with(LocalDensity.current) { Tokens.Metrics.Phone.TOUCH.dp.roundToPx() }
            TodoTheme(false, "my-day") {
                Column(Modifier.fillMaxSize().background(LocalDesign.current.room.canvas)) {
                    Box(Modifier.onSizeChanged { sizes["nav"] = it }) { NavRow(home.smart.first(), on = true, indent = false, onOpen = {}) }
                    Box(Modifier.onSizeChanged { sizes["sub"] = it }) { NavRow(home.groups.first().lists.first(), on = false, indent = true, onOpen = {}) }
                    Box(Modifier.onSizeChanged { sizes["pill"] = it }) { RoomPill(home.smart.first(), on = false) {} }
                }
            }
        }
        assertEquals(3, sizes.size)
        sizes.forEach { (what, size) -> assertTrue("$what is ${size.height} px, under $touch", size.height >= touch) }
    }

    /**
     * The launcher icon from its generated layers, as launchers cut it: a circle, a squircle (One UI's
     * default), a rounded square, the themed (monochrome) icon, and the full layers with the 66 dp safe
     * circle drawn on them.
     */
    @Test
    fun launcher_icon() {
        shot("launcher_icon", COVER) {
            TodoTheme(false, "my-day") {
                val r = LocalDesign.current.room
                Column(
                    Modifier.fillMaxSize().background(r.canvas).padding(24.dp),
                    verticalArrangement = Arrangement.spacedBy(24.dp),
                ) {
                    Row(horizontalArrangement = Arrangement.spacedBy(24.dp)) {
                        LauncherIcon(CircleShape)
                        LauncherIcon(Squircle)
                        LauncherIcon(RoundedCornerShape(12.dp))
                    }
                    Row(horizontalArrangement = Arrangement.spacedBy(24.dp)) {
                        LauncherIcon(Squircle, themed = true)
                        Box(Modifier.size(108.dp)) {
                            Image(painterResource(R.drawable.ic_launcher_background), null, Modifier.fillMaxSize())
                            Image(painterResource(R.drawable.ic_launcher_foreground), null, Modifier.fillMaxSize())
                            Box(Modifier.size(66.dp).align(Alignment.Center).border(1.dp, r.danger, CircleShape))
                        }
                    }
                }
            }
        }
    }
}

/** An adaptive icon at 72 dp: the middle 72 of its 108 dp layers, cut by [shape]. */
@Composable
private fun LauncherIcon(shape: Shape, themed: Boolean = false) {
    val r = LocalDesign.current.room
    Box(Modifier.size(72.dp).clip(shape).background(if (themed) r.ink else Color.Transparent)) {
        if (themed) {
            Image(
                painterResource(R.drawable.ic_launcher_monochrome),
                null,
                Modifier.requiredSize(108.dp),
                colorFilter = ColorFilter.tint(r.sky),
            )
        } else {
            Image(painterResource(R.drawable.ic_launcher_background), null, Modifier.requiredSize(108.dp))
            Image(painterResource(R.drawable.ic_launcher_foreground), null, Modifier.requiredSize(108.dp))
        }
    }
}

/** A superellipse (n = 5), close to the squircle One UI cuts icons with. */
private val Squircle = GenericShape { size, _ ->
    val n = 5.0
    val steps = 96
    for (i in 0..steps) {
        val t = 2 * Math.PI * i / steps
        val c = Math.cos(t)
        val s = Math.sin(t)
        val x = (Math.abs(c).pow(2 / n) * Math.signum(c) + 1) / 2 * size.width
        val y = (Math.abs(s).pow(2 / n) * Math.signum(s) + 1) / 2 * size.height
        if (i == 0) moveTo(x.toFloat(), y.toFloat()) else lineTo(x.toFloat(), y.toFloat())
    }
    close()
}
