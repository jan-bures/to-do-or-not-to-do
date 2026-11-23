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
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.ColumnScope
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.width
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.remember
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
import app.todoornot.core.SyncState
import app.todoornot.core.TaskDrop
import app.todoornot.core.TodoEngine
import app.todoornot.core.quickAddMonth
import app.todoornot.design.LocalDesign
import app.todoornot.design.TodoTheme
import app.todoornot.design.Tokens
import app.todoornot.model.Actions
import app.todoornot.model.MotionFrame
import app.todoornot.model.MenuAction
import app.todoornot.model.PairingState
import app.todoornot.model.TodoModel
import app.todoornot.model.UiState
import app.todoornot.session.EngineHost
import app.todoornot.model.priorityMenu
import app.todoornot.model.stepMenu
import app.todoornot.model.taskMenu
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
import app.todoornot.core.RepeatPreset
import app.todoornot.core.RepeatUnit
import app.todoornot.ui.MergeConfirmCard
import app.todoornot.ui.MergeTaskCard
import app.todoornot.ui.MoveTaskCard
import app.todoornot.ui.CantOpenScreen
import app.todoornot.ui.DeleteListCard
import app.todoornot.ui.DetailPane
import app.todoornot.ui.Button
import app.todoornot.ui.DeviceInviteActions
import app.todoornot.ui.DeviceInviteContent
import app.todoornot.ui.DeviceReviewActions
import app.todoornot.ui.DeviceReviewContent
import app.todoornot.ui.Label
import app.todoornot.session.ThumbHand
import app.todoornot.ui.ActionSheetContent
import app.todoornot.ui.BottomSheet
import app.todoornot.push.PushStatus
import app.todoornot.ui.LocalPushStatus
import app.todoornot.ui.LocalUpdates
import app.todoornot.ui.UpdateControls
import app.todoornot.updates.UpdateManifest
import app.todoornot.updates.UpdateState
import app.todoornot.ui.LocalSheetAtEnd
import app.todoornot.ui.LocalSheetAtRest
import app.todoornot.ui.LocalSheetWidth
import app.todoornot.ui.LocalSwipeFrame
import app.todoornot.ui.OverlayState
import app.todoornot.ui.SwipeFrame
import app.todoornot.ui.TaskActionsSheet
import app.todoornot.ui.listPaneWidth
import app.todoornot.ui.BulkPicker
import app.todoornot.ui.BulkSelectionPicker
import app.todoornot.ui.overlay
import app.todoornot.ui.DialogBackdrop
import app.todoornot.ui.DialogCard
import app.todoornot.ui.DateCalendar
import app.todoornot.ui.MenuCard
import app.todoornot.ui.RenameCard
import app.todoornot.ui.ReminderAccessCard
import app.todoornot.ui.ReminderCard
import app.todoornot.reminders.ReminderNeed
import app.todoornot.ui.MainScreen
import app.todoornot.ui.DragFrame
import app.todoornot.ui.LocalTaskDragFrame
import app.todoornot.ui.LocalSidebarDragFrame
import app.todoornot.ui.SidebarDragFrame
import app.todoornot.core.SidebarDrop
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
import app.todoornot.core.ListView
import app.todoornot.core.WorkflowStage
import app.todoornot.ui.StartDateSheet
import kotlinx.coroutines.runBlocking
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
private val FOLD7_COVER = COVER.copy(screenWidth = 824, screenHeight = 1920)
private val FOLD7_INNER = INNER.copy(screenWidth = 1752, screenHeight = 1940)

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
        wide: Boolean = device === INNER || device === FOLD7_INNER,
        motion: MotionFrame = MotionFrame.Rest,
        hand: ThumbHand = ThumbHand.RIGHT,
        switcherFilter: String = "",
        settings: Boolean = false,
        roomMenu: Boolean = false,
        reach: Boolean = false,
        push: PushStatus = PushStatus(),
        sidebarDrag: SidebarDragFrame? = null,
    ) = shot(name, device) {
        CompositionLocalProvider(LocalSheetAtRest provides true, LocalPushStatus provides push, LocalSidebarDragFrame provides sidebarDrag) {
            val actions = remember { object : Actions by Actions.None { override fun composerNow() = FIXTURE_NOW } }
            MainScreen(state, actions, dark, wide, server = "https://todo.example.com", drawerOpen = drawer,
                collapsed = collapsed, motion = motion, hand = hand, switcherFilter = switcherFilter,
                settingsOpen = settings, roomMenuOpen = roomMenu, reachExpanded = reach)
        }
    }

    @Test
    fun app_update_states() {
        val update = UpdateManifest(337, "0.1.0-build.337", 12_000_000, "a".repeat(64), "todo-android-0.1.0-build.337.apk", 29, setOf("b".repeat(64)))
        val checked = 10_000_000L
        val base = UpdateState(currentVersion = "0.1.0-build.336", checkedAt = checked)
        val states = listOf(
            "current" to base.copy(status = "You're up to date."),
            "downloading" to base.copy(offer = update, progress = 42),
            "ready" to base.copy(offer = update, ready = true),
            "error" to base.copy(problem = "The update wasn't signed with a trusted key for this app."),
            "permission" to base.copy(offer = update, ready = true, permissionNeeded = true),
            "dock" to base.copy(offer = update, ready = true),
        )
        fixtureEngine().use { engine ->
            val room = state(engine)
            for ((name, render) in states) for ((posture, device) in listOf("cover" to FOLD7_COVER, "inner" to FOLD7_INNER)) {
                for (dark in listOf(false, true)) {
                    shot("updates_${name}_${posture}_${if (dark) "dark" else "light"}", device) {
                        CompositionLocalProvider(
                            LocalSheetAtRest provides true,
                            LocalUpdates provides UpdateControls(render, clock = { checked + 8 * 60_000 }),
                        ) {
                            MainScreen(room, Actions.None, dark, wide = device == FOLD7_INNER, server = "https://todo.example.com",
                                drawerOpen = name != "dock", settingsOpen = name != "dock")
                        }
                    }
                }
            }
        }
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

    @Test
    fun workflow_board() {
        val engine = fixtureEngine("workflow")
        val room = engine.list("Home renovation")
        val hallway = engine.taskId(room, "Strip wallpaper and prime hallway walls")
        val model = TodoModel(engine, scope, Dispatchers.Unconfined, now = { FIXTURE_NOW }, start = room)
        val board = model.read()
        model.setRoomView(ListView.LIST, board.generation)
        val list = model.read()
        model.select(hallway)
        val detail = model.read()
        val actions = object : Actions by Actions.None { override fun composerNow() = FIXTURE_NOW }
        for ((posture, device) in listOf("cover" to FOLD7_COVER, "inner" to FOLD7_INNER)) {
            for (dark in listOf(false, true)) {
                val theme = if (dark) "dark" else "light"
                fun workflowShot(screen: String, state: UiState, settings: Boolean = false, draft: String = "", starts: Boolean = false) =
                    shot("${posture}_${screen}_$theme", device) {
                        val wide = device === FOLD7_INNER
                        CompositionLocalProvider(LocalSheetAtRest provides true) {
                            BoxWithConstraints(Modifier.fillMaxSize()) {
                                MainScreen(state, actions, dark, wide = wide, draft = draft,
                                    listSettingsOpen = settings, boardInitialStage = WorkflowStage.IN_PROGRESS)
                                if (starts) TodoTheme(dark, state.room.hue) {
                                    val width = listPaneWidth(maxWidth, wide, null)?.let { maxWidth - it - Tokens.Elevation.HAIRLINE.dp }
                                    CompositionLocalProvider(LocalSheetWidth provides width, LocalSheetAtEnd provides true) {
                                        StartDateSheet(requireNotNull(state.detail), actions) {}
                                    }
                                }
                            }
                        }
                    }
                workflowShot("board", board)
                workflowShot("list_status", list)
                workflowShot("list_settings", list, settings = true)
                workflowShot("detail_status_starts", detail)
                workflowShot("start_date_sheet", detail, starts = true)
                workflowShot("quick_add_status", list, draft = "Check seals mon..fri status:wip !!")
                if (device === FOLD7_INNER) workflowShot("board_detail", detail.copy(view = ListView.BOARD, board = board.board))
            }
        }
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
        val e = fixtureEngine()
        val day = RoomKey.Smart(SmartList.MY_DAY)
        e.setDue(e.taskId(day, "Call the dentist to reschedule"), "2026-11-20")
        val s = state(e).quietlySynced()
        assertEquals(2u, s.room.overdueCount)
        main("cover_my_day_light", FOLD7_COVER, s, dark = false)
        main("cover_my_day_dark", FOLD7_COVER, s, dark = true)
    }

    private fun UiState.quietlySynced() = copy(status = status.copy(state = SyncState.ONLINE, pending = 0u))

    @Test
    fun cover_list_zero_progress() {
        val e = fixtureEngine("groceries")
        val s = state(e, e.list("Groceries")).quietlySynced()
        assertEquals(0u, s.room.done)
        assertEquals(15u, s.room.total)
        assertEquals("18:00", s.room.nextDueToday)
        main("cover_list_zero_progress_light", FOLD7_COVER, s, dark = false)
        main("cover_list_zero_progress_dark", FOLD7_COVER, s, dark = true)
    }

    @Test
    fun cover_header_wraps_at_large_text() {
        val e = fixtureEngine("groceries")
        val list = e.list("Groceries") as RoomKey.List
        e.renameList(list.id, "Weekly groceries")
        val s = state(e, list).quietlySynced()
        main("cover_header_wraps_at_large_text_light", FOLD7_COVER.copy(fontScale = 1.5f), s, dark = false)
        main("cover_header_wraps_at_large_text_dark", FOLD7_COVER.copy(fontScale = 1.5f), s, dark = true)
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
        for (dark in listOf(false, true)) {
            val theme = if (dark) "dark" else "light"
            main("cover_detail_$theme", FOLD7_COVER, s, dark)
            main("cover_detail_left_$theme", FOLD7_COVER, s, dark, hand = ThumbHand.LEFT)
        }
    }

    @Test
    fun row_swipe_frames() {
        val state = state(fixtureEngine())
        for (dark in listOf(false, true)) {
            val theme = if (dark) "dark" else "light"
            val distance = Tokens.Metrics.Phone.SWIPE_THRESHOLD + Tokens.Space.S4
            for ((label, frame) in listOf("complete" to SwipeFrame(state.room.tasks.first().id, distance),
                "my_day" to SwipeFrame(state.room.tasks[1].id, -distance))) {
                shot("cover_row_swipe_${label}_$theme", FOLD7_COVER) {
                    CompositionLocalProvider(LocalSwipeFrame provides frame) { MainScreen(state, Actions.None, dark, false) }
                }
            }
        }
    }

    @Test
    fun task_activity() {
        val engine = fixtureEngine("task-activity")
        val room = RoomKey.Smart(SmartList.COMPLETED)
        val s = state(engine, room, select = engine.taskId(room, "Review the color proofs"))
        for (dark in listOf(false, true)) {
            val theme = if (dark) "dark" else "light"
            main("task_activity_cover_$theme", COVER, s, dark)
            main("task_activity_inner_$theme", INNER, s, dark)
        }
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
            return s.copy(detail = requireNotNull(s.detail).copy(subtitle = original.subtitle, activity = original.activity))
        }
        val off = capture(false)
        val on = capture(true)
        for (dark in listOf(false, true)) {
            val theme = if (dark) "dark" else "light"
            main("toggle_states_off_$theme", INNER, off, dark)
            main("toggle_states_on_$theme", INNER, on, dark)
            overlaid("toggle_states_menu_$theme", FOLD7_COVER, on, dark) {
                TaskActionsSheet(on, Actions.None, requireNotNull(on.detail)) {}
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
    fun quick_add_pickers() {
        val m = TodoModel(fixtureEngine(), scope, Dispatchers.Unconfined, now = { FIXTURE_NOW }, listen = {})
        val selected = "2026-11-22"
        val month = quickAddMonth(FIXTURE_NOW, selected, null)
        val s = Tokens.Space
        val p = Tokens.Metrics.Phone
        for (dark in listOf(false, true)) {
            val theme = if (dark) "dark" else "light"
            for (surface in listOf("chips", "calendar", "priority")) {
                shot("quick_add_${surface}_$theme", FOLD7_COVER) {
                    Box(Modifier.fillMaxSize()) {
                        MainScreen(m.ui.value, m, dark, false, draft = "Water the plants zítra !!", collapsed = true)
                        if (surface != "chips") TodoTheme(dark, m.ui.value.room.hue) {
                            Box(Modifier.fillMaxSize().navigationBarsPadding()) {
                                val anchor = Modifier.align(Alignment.BottomEnd).padding(
                                    end = if (surface == "calendar") s.S4.dp else s.S5.dp,
                                    bottom = (p.BOTTOM_BAR + p.TOUCH + s.S3).dp,
                                )
                                if (surface == "calendar") DateCalendar(month, selected, {}, {}, anchor)
                                else MenuCard(priorityMenu(2) { MenuAction.QuickPriority(it) }, {}, anchor, title = "Choose priority")
                            }
                        }
                    }
                }
            }
        }
    }

    /** While typing (the keyboard can't be drawn here, so the collapse is forced): bar, tasks, composer. */
    @Test
    fun collapsed_header() {
        val s = state(fixtureEngine())
        main("cover_collapsed_light", FOLD7_COVER, s, dark = false, collapsed = true)
        main("cover_collapsed_dark", FOLD7_COVER, s, dark = true, collapsed = true)
        val e = fixtureEngine()
        val personal = e.collection("Personal")
        main("inner_collapsed_dark", FOLD7_INNER, state(e, personal, select = e.taskId(personal, "Renew passport appointment")), dark = true, collapsed = true)
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
    fun cover_room_switcher() {
        val e = fixtureEngine()
        val personal = (e.collection("Personal") as RoomKey.Collection).id
        for (dark in listOf(false, true)) {
            val theme = if (dark) "dark" else "light"
            val expanded = state(e, fold = if (dark) listOf(personal) else emptyList()).let {
                it.copy(status = it.status.copy(state = SyncState.ONLINE), syncedAt = FIXTURE_NOW)
            }
            val inner = state(e, e.collection("Personal"), select = e.taskId(e.list("Admin"), "Renew passport appointment"))
            main("cover_switcher_$theme", FOLD7_COVER, expanded, dark, drawer = true)
            main("cover_switcher_filtered_$theme", FOLD7_COVER, expanded.copy(collapsed = setOf(personal)), dark, drawer = true, switcherFilter = "errands")
            main("inner_switcher_$theme", FOLD7_INNER, inner, dark, drawer = true)
        }
    }

    @Test
    fun sidebar_drag() {
        fixtureEngine().use { e ->
            val s = state(e, e.list("Planning"))
            val work = s.home.groups.first { it.collection?.name == "Work" }
            val personal = s.home.groups.first { it.collection?.name == "Personal" }
            val planning = work.lists.first { it.name == "Planning" }
            val workHead = requireNotNull(work.collection)
            val personalHead = requireNotNull(personal.collection)
            val personalId = (personalHead.key as RoomKey.Collection).id
            for (dark in listOf(false, true)) {
                val theme = if (dark) "dark" else "light"
                main("sidebar_drag_list_$theme", FOLD7_COVER, s, dark, drawer = true, sidebarDrag =
                    SidebarDragFrame(planning, SidebarDrop.Beside(work.lists.first { it.name == "Events" }.key, false), 116f, 110f))
                main("sidebar_drag_target_$theme", FOLD7_COVER, s.copy(collapsed = setOf(personalId)), dark, drawer = true,
                    sidebarDrag = SidebarDragFrame(planning, SidebarDrop.Into(personalId), 116f, 205f))
                main("sidebar_drag_collection_$theme", FOLD7_COVER, s, dark, drawer = true, sidebarDrag =
                    SidebarDragFrame(personalHead, SidebarDrop.Beside(workHead.key, false), 116f, 0f))
            }
        }
    }

    @Test
    fun switcher_footer_offline() {
        val state = state(fixtureEngine())
        val offline = state.copy(status = state.status.copy(state = SyncState.OFFLINE, pending = 3u))
        main("cover_switcher_footer_offline", FOLD7_COVER, offline, dark = false, drawer = true)
    }

    @Test
    fun cover_reach_and_left_hand() {
        val e = fixtureEngine()
        val groceries = fixtureEngine("groceries-reach")
        val reach = state(groceries, groceries.list("Groceries")).quietlySynced()
        assertEquals(5u, reach.room.done)
        for (dark in listOf(false, true)) {
            val theme = if (dark) "dark" else "light"
            main("cover_reach_pull_$theme", FOLD7_COVER, reach, dark, reach = true)
            main("cover_handedness_left_$theme", FOLD7_COVER, state(e), dark, hand = ThumbHand.LEFT)
            main("cover_switcher_left_$theme", FOLD7_COVER, state(e), dark, hand = ThumbHand.LEFT, drawer = true)
        }
    }

    /** The dock filter narrows the rows; the sun still counts the whole room. */
    @Test
    fun cover_filter() {
        main("cover_filter_light", FOLD7_COVER, state(fixtureEngine(), filter = "priority>=high"), dark = false)
        main("cover_filter_nothing_dark", FOLD7_COVER, state(fixtureEngine(), filter = "dentist tomorrow"), dark = true)
    }

    @Test
    fun inner_personal_with_detail() {
        val e = fixtureEngine()
        val personal = e.collection("Personal")
        val s = state(e, personal, select = e.taskId(personal, "Renew passport appointment"))
        main("inner_personal_dark", FOLD7_INNER, s, dark = true)
        main("inner_personal_light", FOLD7_INNER, s, dark = false)
    }

    @Test
    fun finished_day_hands_over_to_tomorrow() {
        val s = state(fixtureEngine("finished"))
        main("cover_finished_dark", FOLD7_COVER, s, dark = true)
        main("cover_finished_light", FOLD7_COVER, s, dark = false)
        main("inner_finished_light", INNER, s, dark = false)
    }

    @Test
    fun empty_list() {
        val e = fixtureEngine()
        val s = state(e, e.list("Reading list"))
        main("cover_empty_light", FOLD7_COVER, s, dark = false)
        main("cover_empty_dark", FOLD7_COVER, s, dark = true)
    }

    @Test
    fun discarding_pairing() {
        shot("pairing_discarding_light", COVER) {
            PairingScreen(PairingState.Discarding, PairingActions.None, dark = false)
        }
    }

    @Test
    fun pairing() {
        for (dark in listOf(false, true)) {
            val theme = if (dark) "dark" else "light"
            shot("pairing_address_$theme", FOLD7_COVER) { PairingScreen(PairingState.Entering(), PairingActions.None, dark) }
            shot("pairing_inner_$theme", FOLD7_INNER) { PairingScreen(PairingState.Entering(), PairingActions.None, dark) }
        }
        shot("pairing_error_dark", COVER) {
            PairingScreen(
                PairingState.Entering("http://todo.example.com", "Use https://: this app only talks to servers over HTTPS."),
                PairingActions.None,
                dark = true,
            )
        }
        shot("pairing_save_failed_light", COVER) {
            PairingScreen(PairingState.SaveFailed, PairingActions.None, dark = false)
        }
        val waiting = PairingState.Waiting("https://todo.example.com", "WXYZ-2345", "https://todo.example.com/pair?code=WXYZ-2345", 0)
        shot("pairing_code_light", FOLD7_COVER) { PairingScreen(waiting, PairingActions.None, dark = false, fixedRemainingSeconds = 285) }
        shot("pairing_code_dark", FOLD7_COVER) { PairingScreen(waiting, PairingActions.None, dark = true, fixedRemainingSeconds = 285) }
        val invite = DeviceInvitation("fixture", "ABCDEF-GHJKMN", "https://todo.example.com/pair/invite#" + "A".repeat(43), 300u)
        for (dark in listOf(false, true)) {
            val theme = if (dark) "dark" else "light"
            shot("device_invite_$theme", COVER) {
                TodoTheme(dark, "my-day") {
                    DialogBackdrop {
                        DialogCard("Pair a new device") {
                            Label("Pair a new device", Tokens.Type.DETAIL_TITLE, LocalDesign.current.room.ink)
                            DeviceInviteContent(invite, 300)
                            DeviceInviteActions(invite, false) {}
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
                            DeviceReviewContent("New phone", "ABCD-EFGH", "203.0.113.7")
                            DeviceReviewActions(false, {}, {})
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
                DialogBackdrop { UnpairCard(unsent, {}, {}, focusCancel = false, changed = changed) }
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
        wide: Boolean = device === INNER || device === FOLD7_INNER,
        overlay: @Composable () -> Unit,
    ) = shot(name, device) {
        Box(Modifier.fillMaxSize()) {
            CompositionLocalProvider(LocalSheetAtRest provides true) {
                MainScreen(state, Actions.None, dark, wide, server = "https://todo.example.com", drawerOpen = drawer)
            }
            CompositionLocalProvider(LocalSheetAtRest provides true) {
                TodoTheme(dark, state.room.hue) { overlay() }
            }
        }
    }

    private fun sheet(name: String, state: UiState, dark: Boolean, device: DeviceConfig = FOLD7_COVER,
        content: @Composable ColumnScope.(OverlayState) -> Unit,
    ) = overlaid(name, device, state, dark) {
        BoxWithConstraints(Modifier.fillMaxSize()) {
            val width = listPaneWidth(maxWidth, device === FOLD7_INNER, null)?.let { maxWidth - it - Tokens.Elevation.HAIRLINE.dp }
            CompositionLocalProvider(LocalSheetWidth provides width, LocalSheetAtEnd provides true) {
                BottomSheet(name.substringBeforeLast('_').replace('_', ' '), {}, content = content)
            }
        }
    }

    @Test
    fun settings_menu() {
        val state = state(fixtureEngine())
        for (dark in listOf(false, true)) {
            val theme = if (dark) "dark" else "light"
            val status = if (dark) SyncState.SYNCING else SyncState.ONLINE
            val ui = state.copy(status = state.status.copy(state = status))
            val lastWake = java.time.LocalDateTime.of(2026, 11, 21, 15, 10)
                .atZone(java.time.ZoneId.systemDefault()).toInstant()
            val push = if (dark) PushStatus(PushStatus.State.ON, lastWake)
                else PushStatus(PushStatus.State.NOT_CONFIGURED)
            main("settings_menu_$theme", FOLD7_COVER, ui, dark, drawer = true, settings = true,
                hand = if (dark) ThumbHand.LEFT else ThumbHand.RIGHT, push = push)
        }
    }

    private fun admin(): Triple<TodoEngine, RoomKey, String> {
        val e = fixtureEngine()
        val admin = e.list("Admin")
        return Triple(e, admin, e.taskId(admin, "Renew passport appointment"))
    }

    private fun freshCarService(engine: TodoEngine, room: RoomKey): UiState {
        val id = engine.addTask(room, "Book the car service", FIXTURE_NOW)
        val s = state(engine, room, select = id)
        val detail = requireNotNull(s.detail)
        // An engine write uses the host's clock. Show the new task at the gallery's fixed instant.
        return s.copy(detail = detail.copy(subtitle = "Created 21.11.2026", activity = detail.activity.map { it.copy(relative = "Just now") }))
    }

    /** The warning above the Due row of a repeating task; no warning on an undated task. */
    @Test
    fun detail_due_calendar() {
        val (e, admin, id) = admin()
        val original = requireNotNull(state(e, admin, select = id).detail)
        e.setRepeat(id, RepeatChoice.Preset(RepeatPreset.DAILY), FIXTURE_NOW)
        val dated = state(e, admin, select = id).let { s ->
            val detail = requireNotNull(s.detail)
            s.copy(detail = detail.copy(
                subtitle = original.subtitle,
                activity = original.activity,
                notes = detail.notes + "\nBring a copy of the form.\nCheck which entrance is open.\nWrite down the appointment number.",
            ))
        }
        val undated = freshCarService(e, admin)
        for (dark in listOf(false, true)) {
            val theme = if (dark) "dark" else "light"
            for ((label, state) in listOf("dated" to dated, "undated" to undated)) {
                val detail = requireNotNull(state.detail)
                val date = detail.due
                sheet("detail_due_calendar_${label}_$theme", state, dark) { overlay ->
                    DateCalendar(quickAddMonth(FIXTURE_NOW, date, null), date, {}, {},
                        dismiss = overlay::close, repeats = detail.repeats, inSheet = true, focusCalendar = false)
                }
            }
            val detail = requireNotNull(dated.detail)
            sheet("inner_detail_due_$theme", dated, dark, FOLD7_INNER) { overlay ->
                DateCalendar(quickAddMonth(FIXTURE_NOW, detail.due, null), detail.due, {}, {},
                    dismiss = overlay::close, repeats = true, inSheet = true, focusCalendar = false)
            }
            sheet("cover_detail_due_sideways_$theme", dated, dark, COVER_SIDEWAYS) { overlay ->
                DateCalendar(quickAddMonth(FIXTURE_NOW, detail.due, null), detail.due, {}, {},
                    dismiss = overlay::close, repeats = true, inSheet = true, focusCalendar = false)
            }
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
            val set = state(e, admin, select = id)
            main("reminder_row_set_$suffix", COVER, set, dark)
            val nextWeek = java.time.Instant.parse("2026-11-23T07:30:00Z").toEpochMilli()
            e.setReminder(id, requireNotNull(set.detail?.reminder), nextWeek, FIXTURE_NOW)
            main("reminder_row_dated_$suffix", COVER, state(e, admin, select = id), dark)
            main("reminder_row_empty_$suffix", COVER, freshCarService(e, admin), dark)
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
            sheet("reminder_picker_${if (dark) "dark" else "light"}", s, dark) {
                ReminderCard(detail.reminder?.atMs, detail.due, FIXTURE_NOW.epochMs, zone, onSave = {}, onCancel = {}, markToday = false, inSheet = true)
            }
        }
        val (e, admin, _) = admin()
        val s = freshCarService(e, admin)
        sheet("reminder_picker_new_light", s, dark = false) {
            ReminderCard(null, null, FIXTURE_NOW.epochMs, zone, onSave = {}, onCancel = {}, markToday = false, inSheet = true)
        }
        val (e2, admin2, id2) = admin()
        val s2 = state(e2, admin2, select = id2)
        sheet("reminder_picker_sideways_light", s2, dark = false, COVER_SIDEWAYS) {
            ReminderCard(s2.detail?.reminder?.atMs, s2.detail?.due, FIXTURE_NOW.epochMs, zone, onSave = {}, onCancel = {}, markToday = false, inSheet = true)
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
            sheet("priority_picker_${if (dark) "dark" else "light"}", s, dark) {
                ActionSheetContent(priorityMenu(detail.id, detail.priority.toInt()), "Priority", {})
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
            val theme = if (dark) "dark" else "light"
            shot("step_editing_$theme", FOLD7_COVER) {
                TodoTheme(dark, s.room.hue) { DetailPane(s, Actions.None, standalone = true, stepEditing = detail.steps[1].id) }
            }
            sheet("step_actions_$theme", s, dark) {
                ActionSheetContent(stepMenu(detail.steps, detail.steps[2].id), "Step actions", {})
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
            overlaid("task_menu_$theme", FOLD7_COVER, s, dark) { TaskActionsSheet(s, m, detail) {} }
            sheet("inner_detail_actions_$theme", s, dark, FOLD7_INNER) {
                ActionSheetContent(taskMenu(s.room, id, detail.done, detail.myDay, detail.priority.toInt()), "Task actions", {})
            }
            val groups = e.moveDestinations(id, "")
            sheet("move_task_$theme", s, dark) { MoveTaskCard("", {}, groups, {}, {}, inSheet = true) }
            m.deleteTask(id)
            main("task_deleted_$theme", COVER, m.ui.value, dark)
        }
    }

    @Test
    fun multi_selection() {
        for (dark in listOf(false, true)) {
            val theme = if (dark) "dark" else "light"
            val e = fixtureEngine()
            val m = TodoModel(e, scope, Dispatchers.Unconfined, now = { FIXTURE_NOW }, listen = {})
            val s = m.ui.value
            s.room.tasks.take(3).forEach { m.toggleBulk(it.id, s.generation) }
            check(m.ui.value.bulkIds.size == 3)
            main("multi_select_$theme", FOLD7_COVER, m.ui.value, dark)
            val destinations = runBlocking { m.bulkDestinations(m.ui.value.bulkIds, "") }
            for (picker in BulkPicker.entries) overlaid("bulk_${picker.name.lowercase()}_$theme", FOLD7_COVER, m.ui.value, dark) {
                CompositionLocalProvider(LocalSheetAtRest provides true) {
                    BulkSelectionPicker(picker, m.ui.value, m, destinations) {}
                }
            }
        }
    }

    @Test
    fun task_drag() {
        val (engine, room, _) = admin()
        engine.addTask(room, "Check the workshop agenda", FIXTURE_NOW)
        engine.addTask(room, "Print the room signs", FIXTURE_NOW)
        val m = TodoModel(engine, scope, Dispatchers.Unconfined, now = { FIXTURE_NOW }, start = room, listen = {})
        val start = m.ui.value
        start.room.tasks.take(2).forEach { m.toggleBulk(it.id, start.generation) }
        val selected = m.ui.value
        val first = start.room.tasks.first()
        val last = start.room.tasks.last()
        val destination = engine.list("Errands") as RoomKey.List
        for (dark in listOf(false, true)) {
            val theme = if (dark) "dark" else "light"
            val frames = listOf(
                "drag_reorder" to DragFrame(first.title, selected.bulkIds, TaskDrop.Slot(last.id, true), 180f, 445f),
                "drag_destination" to DragFrame(first.title, selected.bulkIds, TaskDrop.List(destination.id), 170f, 680f),
            )
            for ((label, frame) in frames) shot("${label}_$theme", COVER) {
                CompositionLocalProvider(LocalTaskDragFrame provides frame, LocalSheetAtRest provides true) {
                    MainScreen(selected, Actions.None, dark, wide = false, drawerOpen = frame.target is TaskDrop.List)
                }
            }
        }
    }

    @Test
    fun merge_tasks() {
        for (dark in listOf(false, true)) {
            val theme = if (dark) "dark" else "light"
            val (e, admin, source) = admin()
            val m = TodoModel(e, scope, Dispatchers.Unconfined, now = { FIXTURE_NOW }, start = admin, listen = {})
            m.select(source)
            val s = m.ui.value
            val candidates = e.mergeCandidates(source, "review")
            val target = candidates.first { it.title == "Prepare Q4 roadmap review" }
            sheet("merge_task_picker_$theme", s, dark) { MergeTaskCard("review", {}, candidates, {}, {}, inSheet = true) }
            val question = requireNotNull(e.mergeQuestion(source, target.id))
            sheet("merge_task_confirm_$theme", s, dark) { MergeConfirmCard(question, {}, {}, focus = false, inSheet = true) }
        }
    }

    /** A list's long-press sheet in the switcher, and a room's ⋯ sheet. */
    @Test
    fun list_menu() {
        val e = fixtureEngine()
        val errands = e.list("Errands")
        val s = state(e, errands)
        val item = s.home.groups.flatMap { it.lists }.first { it.key == errands }
        for (dark in listOf(false, true)) {
            val theme = if (dark) "dark" else "light"
            shot("list_menu_$theme", FOLD7_COVER) {
                CompositionLocalProvider(LocalSheetAtRest provides true) {
                    MainScreen(s, Actions.None, dark, wide = false, drawerOpen = true, navMenuOpen = item)
                }
            }
            main("room_menu_$theme", FOLD7_COVER, s, dark, roomMenu = true)
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
            val current = requireNotNull(read.detail)
            val detail = current.copy(subtitle = "Created 21.11.2026", activity = current.activity.map { it.copy(relative = "Just now") })
            val s = read.copy(detail = detail)
            main("rows_$suffix", COVER, s, dark)
            val change = e.removeLink(detail.sources.single().id)!!
            val toastState = s.copy(toast = app.todoornot.model.Toast(1, "Removed link", organize = change), detail = detail.copy(sources = emptyList()))
            main("detail_toast_$suffix", COVER, toastState, dark)
            main("drawer_toast_$suffix", COVER, toastState.copy(detail = null), dark, drawer = true)
            main("landscape_toast_$suffix", COVER_SIDEWAYS, toastState, dark, wide = true)
            fun card(name: String, content: @Composable () -> Unit) = overlaid("${name}_$suffix", COVER, s, dark) { DialogBackdrop { content() } }
            sheet("repeat_$suffix", s, dark) { RepeatCard(detail, false, {}, {}, {}, inSheet = true) { e.previewRepeat(id, it, FIXTURE_NOW) } }
            sheet("custom_repeat_$suffix", s, dark) { RepeatCard(detail, true, {}, {}, {}, inSheet = true) { e.previewRepeat(id, it, FIXTURE_NOW) } }
            sheet("tags_$suffix", s, dark) { TagsCard(detail.tags, {}, {}, {}, {}, e::tagNamed, inSheet = true) }
            sheet("create_tag_$suffix", s, dark) { TagsCard(detail.tags, {}, {}, {}, {}, e::tagNamed, initialText = "seedlings", inSheet = true) }
            sheet("tag_options_$suffix", s, dark) { app.todoornot.ui.TagOptions(tag, {}, {}, inSheet = true) }
            sheet("swatches_$suffix", s, dark) { SwatchCard(target, "fern", {}, {}, inSheet = true) }
            sheet("rename_tag_$suffix", s, dark) { RenameCard("Rename tag", "garden", {}, {}, focus = false, inSheet = true) }
            card("new_collection") { RenameCard("New collection", "Projects", {}, {}, focus = false) }
            card("add_link") { RenameCard("Add a web link", "todo.example.com/garden", {}, {}, focus = false, webAddress = true) }
            sheet("move_list_$suffix", s.copy(detail = null), dark) { MoveListCard(s.home, list, {}, {}, inSheet = true) }
            val collection = e.collection("Personal") as RoomKey.Collection
            for ((noun, t) in listOf("tag" to target, "collection" to ColorTarget.Collection(collection.id))) {
                val q = requireNotNull(e.organizeQuestion(t))
                if (noun == "tag") sheet("confirm_tag_$suffix", s, dark) { DeleteListCard(q, {}, {}, noun = noun, focusCancel = false, inSheet = true) }
                else card("confirm_$noun") { DeleteListCard(q, {}, {}, noun = noun, focusCancel = false) }
            }
            val days = listOf(
                ActivityDay("Today", listOf(ActivityRow("c", 3u, "Added Water herbs", "You", "Just now", null, "Undo"),
                    ActivityRow("b", 2u, "Moved Check the seedlings", "Garden", "1 min ago", "Undoing…", null)), isToday = true),
                ActivityDay("Yesterday", listOf(ActivityRow("a", 1u, "Added Prepare the beds", "You", "1 day ago", "Couldn't undo: it was changed again.", "Undo")), isToday = false),
            )
            val feed = ActivityView(null, days, 0u, "Refresh activity •", false, ActivityFooter.None)
            main("activity_$suffix", FOLD7_COVER, s.copy(activity = "", feed = feed), dark)
            val agent = s.home.agents.first()
            main("activity_inner_$suffix", FOLD7_INNER, s.copy(activity = agent.id, feed = feed.copy(agent = agent.mark.name)), dark)
        }
    }

    @Test
    fun empty_activity() {
        val feed = ActivityView(null, emptyList(), 0u, "Refresh activity", false, ActivityFooter.Empty("No activity yet."))
        val s = state(fixtureEngine()).copy(feed = feed)
        for (dark in listOf(false, true)) {
            val suffix = if (dark) "dark" else "light"
            main("activity_empty_$suffix", FOLD7_COVER, s.copy(activity = ""), dark)
        }
    }

    @Test
    fun trash() {
        for (dark in listOf(false, true)) {
            val suffix = if (dark) "dark" else "light"
            val populated = state(fixtureEngine("trash"), RoomKey.Trash)
            main("trash_$suffix", FOLD7_COVER, populated, dark)
            main("trash_inner_$suffix", FOLD7_INNER, populated, dark)
            main("trash_empty_$suffix", FOLD7_COVER, state(fixtureEngine(), RoomKey.Trash), dark)
        }
    }

    /** Today's agents under Activity, then Trash, folded collections and a loose list. */
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
