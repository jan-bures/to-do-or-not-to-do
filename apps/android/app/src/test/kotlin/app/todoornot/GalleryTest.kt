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
import app.todoornot.core.DeviceInvitation
import app.todoornot.core.FfiException
import app.todoornot.core.RoomKey
import app.todoornot.core.SmartList
import app.todoornot.core.TodoEngine
import app.todoornot.design.LocalDesign
import app.todoornot.design.TodoTheme
import app.todoornot.design.Tokens
import app.todoornot.model.Actions
import app.todoornot.model.MotionFrame
import app.todoornot.model.PairingState
import app.todoornot.model.TodoModel
import app.todoornot.model.UiState
import app.todoornot.session.EngineHost
import app.todoornot.model.drawerMenu
import app.todoornot.model.priorityMenu
import app.todoornot.model.roomMenu
import app.todoornot.model.stepMenu
import app.todoornot.model.taskMenu
import app.todoornot.ui.ActivityRoom
import app.todoornot.ui.AgentRow
import app.todoornot.core.ActivityDay
import app.todoornot.core.ActivityFooter
import app.todoornot.core.ActivityView
import app.todoornot.ui.MoveListCard
import app.todoornot.ui.RepeatCard
import app.todoornot.ui.TagsCard
import app.todoornot.ui.SwatchCard
import app.todoornot.core.ActivityRow
import app.todoornot.core.ColorTarget
import app.todoornot.core.CustomRepeat
import app.todoornot.core.RepeatChoice
import app.todoornot.core.RepeatUnit
import app.todoornot.ui.MoveTaskCard
import app.todoornot.ui.CantOpenScreen
import app.todoornot.ui.DeleteListCard
import app.todoornot.ui.DetailPane
import app.todoornot.ui.Button
import app.todoornot.ui.DeviceInviteContent
import app.todoornot.ui.DeviceReviewContent
import app.todoornot.ui.Label
import app.todoornot.ui.DialogBackdrop
import app.todoornot.ui.DialogCard
import app.todoornot.ui.DueDateCard
import app.todoornot.ui.MenuCard
import app.todoornot.ui.RenameCard
import app.todoornot.ui.ReminderAccessCard
import app.todoornot.ui.ReminderCard
import app.todoornot.reminders.ReminderNeed
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
/** The gallery is drawn in US English whatever the machine's locale (the date picker words its month). */
private const val LOCALE = "en-rUS"

private val COVER = DeviceConfig.PIXEL_5.copy(screenWidth = 824, screenHeight = 1840, xdpi = 320, ydpi = 320, density = Density.XHIGH, locale = LOCALE)

/** The cover screen on its side, 920 × 412 dp: a short window. */
private val COVER_SIDEWAYS = COVER.copy(screenWidth = 1840, screenHeight = 824, orientation = com.android.resources.ScreenOrientation.LANDSCAPE)

/** A foldable's inner screen, 900 × 1000 dp (the F2 inner mockup). */
private val INNER = DeviceConfig.PIXEL_5.copy(screenWidth = 1800, screenHeight = 2000, xdpi = 320, ydpi = 320, density = Density.XHIGH, locale = LOCALE)

/**
 * The screenshot gallery: the main states on the cover and inner screens, light and dark, drawn from the
 * invented fixture data through the real Rust view model. `./gradlew :app:recordPaparazziDebug` writes
 * them to `app/src/test/snapshots/images`; `verifyPaparazziDebug` compares against them.
 */
class GalleryTest {
    @get:Rule
    val paparazzi = Paparazzi(deviceConfig = COVER, theme = "android:Theme.Material.Light.NoActionBar", maxPercentDifference = 0.1)

    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Unconfined)

    /** The JVM's own default locale, which formats some dates too: US English here, put back after. */
    private val locale: java.util.Locale = java.util.Locale.getDefault().also { java.util.Locale.setDefault(java.util.Locale.US) }

    @After
    fun tearDown() {
        scope.cancel()
        java.util.Locale.setDefault(locale)
    }

    private fun state(
        engine: TodoEngine,
        room: RoomKey = RoomKey.Smart(SmartList.MY_DAY),
        select: String? = null,
        filter: String = "",
        fold: List<String> = emptyList(),
    ): UiState {
        val m = TodoModel(engine, scope, Dispatchers.Unconfined, now = { FIXTURE_NOW }, start = room)
        if (select != null) m.select(select)
        if (filter.isNotEmpty()) m.setFilter(filter)
        fold.forEach(m::toggleCollection)
        return m.read()
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
        motion: MotionFrame = MotionFrame.Rest,
    ) = shot(name, device) {
        MainScreen(state, Actions.None, dark, wide, server = "https://todo.example.com", drawerOpen = drawer, collapsed = collapsed, motion = motion)
    }

    /**
     * A model whose motion runs on a clock the test sets: [toggle] runs at 0, then the frames at [frames]
     * (each releasing what's due and reading the screen again), and the last one stays on screen.
     */
    private fun moving(engine: TodoEngine, toggle: (TodoModel) -> Unit, vararg frames: Long): TodoModel {
        var time = 0L
        val m = TodoModel(engine, scope, Dispatchers.Unconfined, now = { FIXTURE_NOW }, clock = { time })
        toggle(m)
        for (t in frames) {
            time = t
            m.motion.freeze(t)
        }
        return m
    }

    private fun motionShot(name: String, m: TodoModel, dark: Boolean = false) =
        main(name, COVER, m.ui.value, dark, motion = m.motion)

    /**
     * Frozen frames of the motion (`docs/design-language.md`, Motion): completing a task holds its row
     * struck through, then collapses it as the rows below close up; only then does the count change and
     * the sun travel. Completing the last task of My Day brings dusk, with tomorrow entering under it.
     */
    @Test
    fun motion_frames() {
        val m = Tokens.Motion
        val landed = (m.CHECK_TICK_DELAY + m.CHECK_TICK).toLong()
        val exit = landed + m.COMPLETE_HOLD
        val release = exit + m.ROW_COLLAPSE
        val complete = { model: TodoModel ->
            model.setDone(model.ui.value.room.tasks.first { it.title == "Koupit rohlíky a mléko" }.id, true)
        }
        motionShot("motion_fill_light", moving(fixtureEngine(), complete, (m.CHECK_FILL / 2).toLong()))
        motionShot("motion_hold_light", moving(fixtureEngine(), complete, landed + m.COMPLETE_HOLD / 2))
        motionShot("motion_collapse_half_light", moving(fixtureEngine(), complete, exit + m.ROW_COLLAPSE / 2))
        motionShot("motion_sun_half_light", moving(fixtureEngine(), complete, release, release + m.SUN_TRAVEL / 2))
        motionShot("motion_sun_half_dark", moving(fixtureEngine(), complete, release, release + m.SUN_TRAVEL / 2), dark = true)

        // The finished day with one task reopened: completing it brings dusk.
        fun lastOfTheDay(): TodoEngine = fixtureEngine("finished").also { e ->
            e.setDone(e.room(RoomKey.Smart(SmartList.MY_DAY), FIXTURE_NOW).completed.first().id, false)
        }
        val last = { model: TodoModel -> model.setDone(model.ui.value.room.tasks.first().id, true) }
        motionShot("motion_dusk_half_light", moving(lastOfTheDay(), last, release, release + m.DUSK / 2))
        motionShot("motion_dusk_handover_light", moving(lastOfTheDay(), last, release, release + (m.DUSK * 0.72f).toLong()))
        motionShot("motion_dusk_handover_dark", moving(lastOfTheDay(), last, release, release + (m.DUSK * 0.72f).toLong()), dark = true)
    }

    @Test
    fun cover_my_day() {
        val s = state(fixtureEngine())
        main("cover_my_day_light", COVER, s, dark = false)
        main("cover_my_day_dark", COVER, s, dark = true)
    }

    @Test
    fun cover_toast_after_completing() {
        val e = fixtureEngine()
        val id = e.taskId(RoomKey.Smart(SmartList.MY_DAY), "Koupit rohlíky a mléko")
        motionShot("cover_toast_light", moving(e, { m -> m.setDone(id, true) }, 0L))
    }

    @Test
    fun attachments() {
        val engine = fixtureEngine()
        val room = RoomKey.Smart(SmartList.MY_DAY)
        val id = engine.taskId(room, "Call the dentist to reschedule")
        val empty = engine.taskId(room, "Review sync-engine pull request")
        for (dark in listOf(false, true)) {
            val theme = if (dark) "dark" else "light"
            val list = state(engine, room, select = id)
            val detail = requireNotNull(list.detail)
            val long = detail.attachments.first().copy(name = "Dokumenty k žádosti o změnu termínu příští kontroly.pdf")
            val varied = list.copy(detail = detail.copy(attachments = listOf(long) + detail.attachments.drop(1)))
            main("attachments_list_$theme", COVER, list, dark)
            main("attachments_offline_$theme", COVER, varied.copy(status = list.status.copy(state = app.todoornot.core.SyncState.OFFLINE)), dark)
            main("attachments_busy_$theme", COVER, varied.copy(attachment = app.todoornot.model.TransferStatus(long.id, "Downloading ${long.name}…", true)), dark)
            main("attachments_notice_$theme", COVER, varied.copy(attachment = app.todoornot.model.TransferStatus(long.id, "This attachment has no stored content.", false)), dark)
            main("attachments_empty_$theme", COVER, state(engine, room, select = empty), dark)
        }
    }

    @Test
    fun cover_detail() {
        val e = fixtureEngine()
        val admin = e.list("Admin")
        val s = state(e, admin, select = e.taskId(admin, "Renew passport appointment"))
        main("cover_detail_light", COVER, s, dark = false)
        main("cover_detail_dark", COVER, s, dark = true)
    }

    @Test
    fun toggle_states() {
        val e = fixtureEngine()
        val admin = e.list("Admin")
        val id = e.taskId(admin, "Renew passport appointment")
        val original = requireNotNull(state(e, admin, select = id).detail)
        fun capture(isOn: Boolean): UiState {
            e.setMyDay(id, isOn, FIXTURE_NOW)
            e.setImportant(id, isOn)
            val s = state(e, admin, select = id)
            // An event submitted to the engine can have the host's real time. Keep the caption fixed.
            return s.copy(detail = requireNotNull(s.detail).copy(subtitle = original.subtitle))
        }
        val off = capture(false)
        val on = capture(true)
        for (dark in listOf(false, true)) {
            val theme = if (dark) "dark" else "light"
            main("toggle_states_off_$theme", INNER, off, dark)
            main("toggle_states_on_$theme", INNER, on, dark)
            val detail = requireNotNull(on.detail)
            overlaid("toggle_states_menu_$theme", COVER, on, dark) {
                Box(Modifier.fillMaxSize().padding(top = Tokens.Metrics.Phone.APP_BAR.dp, end = Tokens.Space.S2.dp), contentAlignment = Alignment.TopEnd) {
                    MenuCard(taskMenu(on.room, id, detail.done, detail.myDay, detail.priority.toInt()), {})
                }
            }
        }
    }

    @Test
    fun editing_notes_and_sources() {
        val e = fixtureEngine()
        val admin = e.list("Admin")
        val id = e.taskId(admin, "Renew passport appointment")
        val s = state(e, admin, select = id)
        val m = TodoModel(e, scope, Dispatchers.Unconfined, now = { FIXTURE_NOW }, start = admin, listen = {})
        m.select(id)
        m.notesDraft(id).edit("Bring the form.\nAsk for a copy.")
        shot("notes_editor", COVER) { MainScreen(m.ui.value, m, false, false) }
        // One source opens in the browser; the other has no destination and no arrow.
        val detail = requireNotNull(s.detail)
        val otherMail = e.room(RoomKey.Smart(SmartList.ALL), FIXTURE_NOW).tasks.asSequence()
            .flatMap { e.task(it.id, FIXTURE_NOW)!!.sources }.first { it.url == null }
        val links = detail.copy(steps = emptyList(), fields = emptyList(), sources = detail.sources + otherMail)
        main("source_links", COVER, s.copy(detail = links), dark = false)
    }

    @Test
    fun parsed_quick_add_date() {
        val m = TodoModel(fixtureEngine(), scope, Dispatchers.Unconfined, now = { FIXTURE_NOW }, listen = {})
        shot("quick_add_date", COVER) {
            MainScreen(m.ui.value, m, false, false, draft = "Water the plants zítra !!", collapsed = true)
        }
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

    /** At a large font scale the title and chips stay legible, including descenders and diacritics. */
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
        shot("pairing_code_light", COVER) { PairingScreen(waiting, PairingActions.None, dark = false, fixedRemainingSeconds = 285) }
        shot("pairing_code_dark", COVER) { PairingScreen(waiting, PairingActions.None, dark = true, fixedRemainingSeconds = 285) }
        val invite = DeviceInvitation("fixture", "ABCDEF-GHJKMN", "https://todo.example.com/pair/invite#" + "A".repeat(43), 300u)
        for (dark in listOf(false, true)) {
            val theme = if (dark) "dark" else "light"
            shot("device_invite_$theme", COVER) {
                TodoTheme(dark, "my-day") {
                    DialogBackdrop {
                        DialogCard("Pair a new device") {
                            Label("Pair a new device", Tokens.Type.DETAIL_TITLE, LocalDesign.current.room.ink)
                            DeviceInviteContent(invite, 300, false) {}
                            Button("Close", primary = false) {}
                        }
                    }
                }
            }
            shot("device_approve_$theme", COVER) {
                TodoTheme(dark, "my-day") {
                    DialogBackdrop {
                        DialogCard("Approve a device") {
                            Label("Approve a device", Tokens.Type.DETAIL_TITLE, LocalDesign.current.room.ink)
                            DeviceReviewContent("New phone", "ABCD-EFGH", "203.0.113.7", false, {}, {})
                            Button("Close", primary = false) {}
                        }
                    }
                }
            }
        }
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
                    Box(Modifier.onSizeChanged { sizes["agent"] = it }) { AgentRow(home.agents.first(), on = false) {} }
                    Box(Modifier.onSizeChanged { sizes["pill"] = it }) { RoomPill(home.smart.first(), on = false) {} }
                }
            }
        }
        assertEquals(4, sizes.size)
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

    /**
     * An overlay over a screen, at rest, as it would sit there: popups and dialogs are windows of their
     * own, which the gallery can't draw, so their cards are drawn in place over the screen.
     */
    private fun overlaid(
        name: String,
        device: DeviceConfig,
        state: UiState,
        dark: Boolean,
        drawer: Boolean = false,
        wide: Boolean = device === INNER,
        overlay: @Composable () -> Unit,
    ) = shot(name, device) {
        Box(Modifier.fillMaxSize()) {
            MainScreen(state, Actions.None, dark, wide, server = "https://todo.example.com", drawerOpen = drawer)
            TodoTheme(dark, state.room.hue) { overlay() }
        }
    }

    private fun admin(): Triple<TodoEngine, RoomKey, String> {
        val e = fixtureEngine()
        val admin = e.list("Admin")
        return Triple(e, admin, e.taskId(admin, "Renew passport appointment"))
    }

    /** The due date picker over the task, on its date, drawn as for a repeating task: the note says what Clear does. */
    @Test
    fun due_date_picker() {
        for (dark in listOf(false, true)) {
            val (e, admin, id) = admin()
            val s = state(e, admin, select = id)
            val detail = requireNotNull(s.detail)
            overlaid("due_date_picker_${if (dark) "dark" else "light"}", COVER, s, dark) {
                DialogBackdrop {
                    DueDateCard(detail.due, s.today, repeats = true, onSave = {}, onCancel = {}, markToday = false)
                }
            }
        }
        // An undated task opens on today's month, with nothing to clear.
        val (e, admin, _) = admin()
        val undated = e.addTask(admin, "Book the car service", FIXTURE_NOW)
        val s = state(e, admin, select = undated)
        overlaid("due_date_picker_undated_light", COVER, s, dark = false) {
            DialogBackdrop { DueDateCard(null, s.today, repeats = false, onSave = {}, onCancel = {}, markToday = false) }
        }
        // A short window (the cover screen on its side): no headline, the calendar scrolls, the buttons stay.
        val (e2, admin2, id2) = admin()
        val s2 = state(e2, admin2, select = id2)
        overlaid("due_date_picker_sideways_light", COVER_SIDEWAYS, s2, dark = false, wide = false) {
            DialogBackdrop { DueDateCard(s2.detail?.due, s2.today, repeats = true, onSave = {}, onCancel = {}, markToday = false) }
        }
    }

    /**
     * The Reminder row, set (the passport task's, today at 18:00) and empty (a new task's), with its
     * chevron: it always opens the reminder picker.
     */
    @Test
    fun reminder_row() {
        for (dark in listOf(false, true)) {
            val suffix = if (dark) "dark" else "light"
            val (e, admin, id) = admin()
            main("reminder_row_set_$suffix", COVER, state(e, admin, select = id), dark)
            val fresh = e.addTask(admin, "Book the car service", FIXTURE_NOW)
            main("reminder_row_empty_$suffix", COVER, state(e, admin, select = fresh), dark)
        }
    }

    /**
     * The reminder picker over the task: the date picker and the 24-hour time input, on the reminder's day
     * and time, with Remove; for a task without one, on the next whole hour, with nothing to remove; and in
     * a short window, where it scrolls under the buttons.
     */
    @Test
    fun reminder_picker() {
        val zone = java.time.ZoneOffset.ofHours(1)
        for (dark in listOf(false, true)) {
            val (e, admin, id) = admin()
            val s = state(e, admin, select = id)
            val detail = requireNotNull(s.detail)
            overlaid("reminder_picker_${if (dark) "dark" else "light"}", COVER, s, dark) {
                DialogBackdrop {
                    ReminderCard(detail.reminder?.atMs, detail.due, FIXTURE_NOW.epochMs, zone, onSave = {}, onCancel = {}, markToday = false)
                }
            }
        }
        val (e, admin, _) = admin()
        val fresh = e.addTask(admin, "Book the car service", FIXTURE_NOW)
        val s = state(e, admin, select = fresh)
        overlaid("reminder_picker_new_light", COVER, s, dark = false) {
            DialogBackdrop { ReminderCard(null, null, FIXTURE_NOW.epochMs, zone, onSave = {}, onCancel = {}, markToday = false) }
        }
        val (e2, admin2, id2) = admin()
        val s2 = state(e2, admin2, select = id2)
        overlaid("reminder_picker_sideways_light", COVER_SIDEWAYS, s2, dark = false, wide = false) {
            DialogBackdrop { ReminderCard(s2.detail?.reminder?.atMs, s2.detail?.due, FIXTURE_NOW.epochMs, zone, onSave = {}, onCancel = {}, markToday = false) }
        }
    }

    /** Why reminders need notifications (and exact alarms), asked after the first reminder is set. */
    @Test
    fun reminder_permission() {
        for (dark in listOf(false, true)) {
            val (e, admin, id) = admin()
            val s = state(e, admin, select = id)
            overlaid("reminder_permission_${if (dark) "dark" else "light"}", COVER, s, dark) {
                DialogBackdrop { ReminderAccessCard(ReminderNeed.NOTIFICATIONS, onAllow = {}, onNotNow = {}) }
            }
        }
        val (e, admin, id) = admin()
        overlaid("reminder_permission_exact_light", COVER, state(e, admin, select = id), dark = false) {
            DialogBackdrop { ReminderAccessCard(ReminderNeed.EXACT_ALARMS, onAllow = {}, onNotNow = {}) }
        }
    }

    /** The priority picker under the Priority row: four levels, the current one ticked. */
    @Test
    fun priority_picker() {
        for (dark in listOf(false, true)) {
            val (e, admin, id) = admin()
            val s = state(e, admin, select = id)
            val detail = requireNotNull(s.detail)
            overlaid("priority_picker_${if (dark) "dark" else "light"}", COVER, s, dark) {
                Box(Modifier.fillMaxSize().padding(start = 24.dp, top = 600.dp)) {
                    MenuCard(priorityMenu(detail.id, detail.priority.toInt()), {})
                }
            }
        }
    }

    /**
     * A step's actions: one step being renamed in place, another's menu open (rename, move, delete); and
     * after deleting one, the toast with Undo, drawn by the detail pane itself on the cover screen.
     */
    @Test
    fun step_actions() {
        for (dark in listOf(false, true)) {
            val (e, admin, id) = admin()
            val s = state(e, admin, select = id)
            val detail = requireNotNull(s.detail)
            shot("step_actions_${if (dark) "dark" else "light"}", COVER) {
                TodoTheme(dark, s.room.hue) {
                    Box(Modifier.fillMaxSize()) {
                        DetailPane(s, Actions.None, standalone = true, stepEditing = detail.steps[1].id)
                        Box(Modifier.fillMaxSize().padding(start = 146.dp, top = 303.dp)) {
                            MenuCard(stepMenu(detail.steps, detail.steps[2].id), {})
                        }
                    }
                }
            }
        }
        val (e, admin, id) = admin()
        val m = TodoModel(e, scope, Dispatchers.Unconfined, now = { FIXTURE_NOW }, clock = { 0L }, start = admin, listen = {})
        m.select(id)
        m.deleteStep(requireNotNull(m.ui.value.detail).steps[1].id)
        val s = m.ui.value
        shot("step_deleted_light", COVER) {
            TodoTheme(false, s.room.hue) {
                DetailPane(s, m, standalone = true)
            }
        }
    }

    @Test
    fun task_actions() {
        for (dark in listOf(false, true)) {
            val theme = if (dark) "dark" else "light"
            val (e, admin, id) = admin()
            e.addTask(admin, "File the receipt", FIXTURE_NOW)
            val m = TodoModel(e, scope, Dispatchers.Unconfined, now = { FIXTURE_NOW }, start = admin, listen = {})
            m.select(id)
            val s = m.ui.value
            val detail = requireNotNull(s.detail)
            overlaid("task_menu_$theme", COVER, s, dark) {
                Box(Modifier.fillMaxSize().padding(top = Tokens.Metrics.Phone.APP_BAR.dp, end = Tokens.Space.S2.dp), contentAlignment = Alignment.TopEnd) {
                    MenuCard(taskMenu(s.room, id, detail.done, detail.myDay, detail.priority.toInt()), {})
                }
            }
            val groups = e.moveDestinations(id, "")
            overlaid("move_task_$theme", COVER, s, dark) {
                DialogBackdrop { MoveTaskCard("", {}, groups, {}, {}) }
            }
            m.deleteTask(id)
            main("task_deleted_$theme", COVER, m.ui.value, dark)
        }
    }

    /** A list's long-press menu in the drawer, and the room's ⋯ menu in a list. */
    @Test
    fun list_menu() {
        for (dark in listOf(false, true)) {
            val e = fixtureEngine()
            val s = state(e)
            val errands = e.list("Errands")
            overlaid("list_menu_${if (dark) "dark" else "light"}", COVER, s, dark, drawer = true) {
                Box(Modifier.fillMaxSize().padding(start = Tokens.Space.S3.dp, bottom = Tokens.Space.S4.dp), contentAlignment = Alignment.BottomStart) {
                    MenuCard(drawerMenu(s.home, errands), {})
                }
            }
        }
        val e = fixtureEngine()
        val errands = e.list("Errands")
        val s = state(e, errands)
        overlaid("room_menu_light", COVER, s, dark = false) {
            Box(Modifier.fillMaxSize().padding(top = 48.dp, end = 10.dp), contentAlignment = Alignment.TopEnd) {
                MenuCard(roomMenu(s.home, errands, s.showCompleted), {})
            }
        }
        overlaid("rename_list_light", COVER, s, dark = false) {
            DialogBackdrop { RenameCard("Rename list", "Errands", {}, {}, focus = false) }
        }
    }

    /** Deleting a list asks first, in full: which list, how many tasks, and that Undo brings them back. */
    @Test
    fun delete_confirmation() {
        for (dark in listOf(false, true)) {
            val e = fixtureEngine()
            val errands = e.list("Errands")
            val s = state(e, errands)
            val question = requireNotNull(e.deleteListQuestion((errands as RoomKey.List).id))
            overlaid("delete_list_${if (dark) "dark" else "light"}", COVER, s, dark) {
                DialogBackdrop { DeleteListCard(question, {}, {}, focusCancel = false) }
            }
        }
        // Asked again: the list changed while the question was open.
        val e = fixtureEngine()
        val errands = e.list("Errands") as RoomKey.List
        val s = state(e, errands)
        overlaid("delete_list_changed_light", COVER, s, dark = false) {
            DialogBackdrop { DeleteListCard(requireNotNull(e.deleteListQuestion(errands.id)), {}, {}, focusCancel = false, changed = true) }
        }
    }

    @Test
    fun organize() {
        for (dark in listOf(false, true)) {
            val suffix = if (dark) "dark" else "light"
            val e = fixtureEngine()
            val list = e.list("Errands") as RoomKey.List
            val id = e.addTask(list, "Water herbs", FIXTURE_NOW)
            e.setRepeat(id, RepeatChoice.Custom(CustomRepeat(RepeatUnit.WEEK, 2u, 5u)), FIXTURE_NOW)
            e.addWebLink(id, "todo.example.com/garden")
            e.tagTaskNamed(id, "garden")
            val tag = e.task(id, FIXTURE_NOW)!!.tags.first { it.name == "garden" }
            val target = ColorTarget.Tag(tag.id)
            e.setSwatch(target, "fern")
            val read = state(e, list, select = id)
            val detail = read.detail!!.copy(subtitle = "Created 21 Nov")
            val s = read.copy(detail = detail)
            main("rows_$suffix", COVER, s, dark)
            val change = e.removeLink(detail.sources.single().id)!!
            val toastState = s.copy(toast = app.todoornot.model.Toast(1, "Removed link", organize = change), detail = detail.copy(sources = emptyList()))
            main("detail_toast_$suffix", COVER, toastState, dark)
            main("drawer_toast_$suffix", COVER, toastState.copy(detail = null), dark, drawer = true)
            main("landscape_toast_$suffix", COVER_SIDEWAYS, toastState, dark, wide = true)
            fun card(name: String, content: @Composable () -> Unit) = overlaid("${name}_$suffix", COVER, s, dark) { DialogBackdrop { content() } }
            card("repeat") { RepeatCard(detail, false, {}, {}, {}) { e.previewRepeat(id, it, FIXTURE_NOW) } }
            card("custom_repeat") { RepeatCard(detail, true, {}, {}, {}) { e.previewRepeat(id, it, FIXTURE_NOW) } }
            card("tags") { TagsCard(detail.tags, {}, {}, {}, {}, e::tagNamed) }
            card("create_tag") { TagsCard(detail.tags, {}, {}, {}, {}, e::tagNamed, initialText = "seedlings") }
            card("tag_options") { app.todoornot.ui.TagOptions(tag, {}, {}) }
            card("swatches") { SwatchCard(target, "fern", {}, {}) }
            card("rename_tag") { RenameCard("Rename tag", "garden", {}, {}, focus = false) }
            card("new_collection") { RenameCard("New collection", "Projects", {}, {}, focus = false) }
            card("add_link") { RenameCard("Add a web link", "todo.example.com/garden", {}, {}, focus = false, webAddress = true) }
            card("move_list") { MoveListCard(s.home, list, {}, {}) }
            val collection = e.collection("Personal") as RoomKey.Collection
            for ((noun, t) in listOf("tag" to target, "collection" to ColorTarget.Collection(collection.id))) {
                val q = requireNotNull(e.organizeQuestion(t))
                card("confirm_$noun") { DeleteListCard(q, {}, {}, noun = noun, focusCancel = false) }
            }
            val days = listOf(
                ActivityDay("Today", listOf(ActivityRow("c", 3u, "Added Water herbs", "You", "Just now", null, "Undo"),
                    ActivityRow("b", 2u, "Moved Check the seedlings", "Garden", "1 min ago", "Undoing…", null))),
                ActivityDay("Yesterday", listOf(ActivityRow("a", 1u, "Added Prepare the beds", "You", "1 day ago", "Couldn't undo: it was changed again.", "Undo"))),
            )
            val feed = ActivityView(null, days, 0u, "Refresh activity •", false, ActivityFooter.None)
            shot("activity_$suffix", COVER) { TodoTheme(dark, s.room.hue) { ActivityRoom(s.copy(feed = feed), Actions.None, {}) } }
        }
    }

    @Test
    fun empty_activity() {
        val feed = ActivityView(null, emptyList(), 0u, "Refresh activity", false, ActivityFooter.Empty("No activity yet."))
        val s = state(fixtureEngine()).copy(feed = feed)
        for (dark in listOf(false, true)) {
            val suffix = if (dark) "dark" else "light"
            shot("activity_empty_$suffix", COVER) { TodoTheme(dark, s.room.hue) { ActivityRoom(s, Actions.None, {}) } }
        }
    }

    /** Today's agents under Activity, then folded collections followed by a loose list. */
    @Test
    fun agent_summary() {
        for (dark in listOf(false, true)) {
            val e = fixtureEngine()
            e.createList("Loose list", RoomKey.Smart(SmartList.MY_DAY))
            val folded = e.home(FIXTURE_NOW).groups.mapNotNull { (it.collection?.key as? RoomKey.Collection)?.id }
            main("agent_summary_${if (dark) "dark" else "light"}", COVER, state(e, fold = folded), dark, drawer = true)
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
