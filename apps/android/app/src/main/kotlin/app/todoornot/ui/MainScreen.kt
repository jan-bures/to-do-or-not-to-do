package app.todoornot.ui

import androidx.activity.compose.BackHandler
import androidx.compose.foundation.background
import androidx.compose.animation.AnimatedContent
import androidx.compose.animation.core.animate
import androidx.compose.animation.core.tween
import app.todoornot.design.cubic
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.togetherWith
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.statusBars
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.layout.width
import androidx.compose.ui.semantics.hideFromAccessibility
import androidx.compose.ui.semantics.clearAndSetSemantics
import androidx.compose.ui.unit.Dp
import app.todoornot.core.ListView
import app.todoornot.core.NavItem
import app.todoornot.model.MenuEntry
import app.todoornot.model.drawerMenu
import app.todoornot.session.ThumbHand
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.SideEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.ui.platform.LocalFocusManager
import app.todoornot.core.AssistantProposalStatus
import app.todoornot.model.AssistantModel
import app.todoornot.model.AssistantUi
import kotlinx.coroutines.launch
import androidx.compose.runtime.saveable.Saver
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.drawBehind
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.unit.dp
import app.todoornot.core.RoomKey
import app.todoornot.core.ColorTarget
import app.todoornot.model.OrganizeEdit
import app.todoornot.design.LocalDesign
import app.todoornot.design.TodoTheme
import app.todoornot.design.Tokens
import app.todoornot.model.Actions
import app.todoornot.model.MenuAction
import app.todoornot.model.MotionFrame
import app.todoornot.model.UiState
import app.todoornot.model.renameTitle
import app.todoornot.model.swappedWith
import app.todoornot.core.WorkflowStage
import app.todoornot.model.featuresFor
import kotlinx.coroutines.delay

data class PaneFold(val start: Dp, val end: Dp)

internal fun listPaneWidth(width: Dp, wide: Boolean, fold: PaneFold?): Dp? {
    if (!wide) return null
    val edge = Tokens.Metrics.Phone.TOUCH.dp
    val divider = Tokens.Elevation.HAIRLINE.dp
    val separating = fold?.takeIf { it.start >= edge && it.end >= it.start && it.end <= width - edge }
    return separating?.start ?: minOf(Tokens.Metrics.Phone.INNER_LIST.dp, (width - divider) / 2)
}

/** Keeps a list's or a collection's room across a recreation (a rename dialog's target). */
private val OrganizeSaver = Saver<MenuAction?, String>(
    save = { action -> when (action) {
        is MenuAction.MoveList -> "move:${action.room.id}"
        is MenuAction.ListSettings -> "settings:${action.room.id}"
        is MenuAction.Colour -> when (val room = action.room) {
            is RoomKey.List -> "list:${room.id}"
            is RoomKey.Collection -> "collection:${room.id}"
            else -> ""
        }
        else -> ""
    } },
    restore = { saved -> val id = saved.substringAfter(':'); when (saved.substringBefore(':')) {
        "move" -> MenuAction.MoveList(RoomKey.List(id))
        "settings" -> MenuAction.ListSettings(RoomKey.List(id))
        "list" -> MenuAction.Colour(RoomKey.List(id))
        "collection" -> MenuAction.Colour(RoomKey.Collection(id))
        else -> null
    } },
)

private val RoomSaver = Saver<RoomKey?, String>(
    save = { k ->
        when (k) {
            is RoomKey.List -> "list:" + k.id
            is RoomKey.Collection -> "collection:" + k.id
            else -> ""
        }
    },
    restore = { s ->
        when {
            s.startsWith("list:") -> RoomKey.List(s.removePrefix("list:"))
            s.startsWith("collection:") -> RoomKey.Collection(s.removePrefix("collection:"))
            else -> null
        }
    },
)

/**
 * The main screen. Folded (compact width) it is one pane: the room, or a task's detail over it.
 * Unfolded it is the list (450 dp) and the detail side by side, like the F2 inner-screen mockup.
 * [motion] is the model's motion; the screen runs its frames while it's shown (none at rest).
 */
@Composable
internal fun MainScreen(
    state: UiState,
    actions: Actions,
    dark: Boolean,
    wide: Boolean,
    server: String? = null,
    draft: String = "",
    drawerOpen: Boolean = false,
    collapsed: Boolean = false,
    motion: MotionFrame = MotionFrame.Rest,
    composerFocus: (() -> Unit)? = null,
    hand: ThumbHand = ThumbHand.RIGHT,
    onHand: (ThumbHand) -> Unit = {},
    fold: PaneFold? = null,
    switcherFilter: String = "",
    settingsOpen: Boolean = false,
    roomMenuOpen: Boolean = false,
    navMenuOpen: NavItem? = null,
    reachExpanded: Boolean = false,
    listSettingsOpen: Boolean = false,
    boardInitialStage: WorkflowStage = WorkflowStage.TODO,
    timelineGallery: TimelineGallery? = null,
    assistant: AssistantUi? = null,
    assistantModel: AssistantModel? = null,
    initialAssistantOpen: Boolean = false,
    initialAssistantOverlay: AssistantOverlay? = null,
) {
    LaunchedEffect(motion) { motion.drive() }
    CompositionLocalProvider(LocalMotion provides motion) {
        BoxWithConstraints(Modifier.fillMaxSize()) {
            val paneWidth = listPaneWidth(maxWidth, wide, fold)
            Screen(state, actions, dark, wide, server, draft, drawerOpen, collapsed, composerFocus,
                hand, onHand, fold, paneWidth, maxWidth, switcherFilter, settingsOpen, roomMenuOpen, navMenuOpen, reachExpanded,
                listSettingsOpen, boardInitialStage, timelineGallery, assistant, assistantModel, initialAssistantOpen, initialAssistantOverlay)
        }
    }
}

@Composable
private fun Screen(
    state: UiState,
    actions: Actions,
    dark: Boolean,
    wide: Boolean,
    server: String?,
    draft: String,
    drawerOpen: Boolean,
    collapsed: Boolean,
    composerFocus: (() -> Unit)?,
    hand: ThumbHand,
    onHand: (ThumbHand) -> Unit,
    fold: PaneFold?,
    paneWidth: Dp?,
    windowWidth: Dp,
    switcherFilter: String,
    settingsOpen: Boolean,
    roomMenuOpen: Boolean,
    navMenuOpen: NavItem?,
    reachExpanded: Boolean,
    listSettingsOpen: Boolean,
    boardInitialStage: WorkflowStage,
    timelineGallery: TimelineGallery?,
    assistant: AssistantUi?,
    assistantModel: AssistantModel?,
    initialAssistantOpen: Boolean,
    initialAssistantOverlay: AssistantOverlay?,
) {
    TodoTheme(dark, if (state.activity != null) "agent-activity" else state.room.hue) {
        val r = LocalDesign.current.room
        val motion = LocalMotion.current
        var switcher by rememberSaveable { mutableStateOf(drawerOpen) }
        var reveal by remember { mutableStateOf<Float?>(null) }
        var returningGesture by remember { mutableStateOf(false) }
        val reduced = reducedMotion()
        LaunchedEffect(returningGesture) {
            if (returningGesture) {
                animate(reveal ?: 0f, 0f,
                    animationSpec = tween(if (reduced) 0 else Tokens.Motion.PANEL, easing = Tokens.Motion.EASE_EXIT.cubic()),
                ) { value, _ -> reveal = value }
                reveal = null
                returningGesture = false
            }
        }
        val switcherVisible = switcher || reveal != null
        val updates = LocalUpdates.current
        var updatesOpen by rememberSaveable { mutableStateOf(false) }
        LaunchedEffect(updates?.openRequest) {
            if (updates != null && updates.openRequest > 0) {
                switcher = false
                reveal = null
                updatesOpen = true
                updates.opened()
            }
        }
        val menu = { switcher = true }
        var roomActionsOpen by rememberSaveable(state.generation) { mutableStateOf(roomMenuOpen || listSettingsOpen) }
        var roomActionsSettings by rememberSaveable(state.generation) { mutableStateOf(listSettingsOpen) }
        var navMenu by remember { mutableStateOf(navMenuOpen) }
        var bulkPicker by remember(state.generation, state.bulkIds) { mutableStateOf<BulkPicker?>(null) }
        val detail = state.detail
        val ai = assistant.takeUnless { state.signedOut }
        var aiOpen by rememberSaveable { mutableStateOf(initialAssistantOpen) }
        var aiOverlay by rememberSaveable { mutableStateOf(initialAssistantOverlay) }
        var aiOverlayAtEnd by rememberSaveable { mutableStateOf(initialAssistantOpen && initialAssistantOverlay != null) }
        val showAi = ai != null && aiOpen
        val showAiOverlay = ai != null && aiOverlay != null
        val pending = ai?.panel?.turns?.lastOrNull()?.proposal?.status == AssistantProposalStatus.PENDING
        var aiTaskLink by remember(state.generation, showAi) { mutableStateOf<String?>(null) }
        val taskActions = remember(actions, showAi) {
            if (!showAi) actions else object : Actions by actions {
                override fun select(id: String?) {
                    if (id != null) aiOpen = false
                    actions.select(id)
                }
            }
        }
        val aiScroll = rememberAssistantScroll()
        val scope = rememberCoroutineScope()
        val focus = LocalFocusManager.current
        val toggleAi = {
            focus.clearFocus()
            aiOverlay = null
            aiOpen = !aiOpen
        }
        val openAiSettings = {
            focus.clearFocus()
            switcher = false
            reveal = null
            returningGesture = false
            aiOverlayAtEnd = false
            aiOverlay = AssistantOverlay.SETTINGS
        }
        val boardInput = if (state.board == null) null else androidx.compose.runtime.key(state.generation) { rememberBoardInput(boardInitialStage) }
        val detailSheets = rememberDetailSheets(detail?.id, state.generation)
        val timelineSheets = remember(state.generation, state.view) { TimelineSheets() }
        state.unpairing?.let { q -> UnpairDialog(q.unsent, actions::cancelUnpair, { actions.confirmUnpair(q.unsent) }, q.changed, paneWidth) }
        // Room and switcher actions: rename and delete ask in a
        // dialog; a move glides the rows it swaps.
        var renaming by rememberSaveable(stateSaver = RoomSaver) { mutableStateOf<RoomKey?>(null) }
        var deleting by rememberSaveable { mutableStateOf<String?>(null) }
        var moving by remember { mutableStateOf(emptySet<RoomKey>()) }
        LaunchedEffect(moving) {
            if (moving.isNotEmpty()) {
                delay(Tokens.Motion.ROW_ENTER * 4L)
                moving = emptySet()
            }
        }
        var deleteCollection by rememberSaveable { mutableStateOf<String?>(null) }
        var newCollection by rememberSaveable { mutableStateOf(false) }
        val sheetOpen = switcherVisible || (updatesOpen && updates != null) || roomActionsOpen || bulkPicker != null ||
            detailSheets.page != null || timelineSheets.open || showAiOverlay || (showAi && !wide) ||
            renaming != null || deleting != null || deleteCollection != null || newCollection || state.unpairing != null
        if (newCollection) OverlayDialog({ newCollection = false }, paneWidth = paneWidth) { overlay ->
            RenameCard("New collection", "", { name -> overlay.choose { actions.organize(OrganizeEdit.NewCollection(name)); switcher = false } }, overlay::close)
        }
        deleteCollection?.let { id -> OverlayDialog({ deleteCollection = null }, paneWidth = paneWidth) { overlay ->
            ConfirmOrganize(ColorTarget.Collection(id), actions, overlay)
        } }
        val onRoomMenu: (MenuAction) -> Unit = { a ->
            when (a) {
                is MenuAction.DeleteCollection -> deleteCollection = a.room.id
                is MenuAction.Rename -> renaming = a.room
                is MenuAction.Move -> {
                    swappedWith(state.home, a.room, a.by)?.let { moving = setOf(a.room, it) }
                    actions.moveRoom(a.room, a.by)
                }
                is MenuAction.AskDelete -> deleting = a.list
                is MenuAction.ShowCompleted -> actions.setShowCompleted(a.on)
                else -> Unit
            }
        }
        renaming?.let { room ->
            // The name when the dialog opened: a sync renaming it meanwhile doesn't change the field.
            val name = remember(room) { state.home.groups.flatMap { listOfNotNull(it.collection) + it.lists }.firstOrNull { it.key == room }?.name }
            if (name == null) {
                SideEffect { renaming = null }
            } else {
                OverlayDialog(onDismiss = { renaming = null }, paneWidth = paneWidth) { overlay ->
                    RenameCard(renameTitle(room), name, onSave = { n -> overlay.choose { actions.renameRoom(room, n) } }, onCancel = overlay::close)
                }
            }
        }
        deleting?.let { id ->
            // The question as the owner reads it. Confirming checks it again: if the list changed meanwhile
            // (renamed, or tasks synced in or out), nothing is deleted and the new question is asked.
            var question by remember(id) { mutableStateOf(actions.deleteListQuestion(id)) }
            var changed by remember(id) { mutableStateOf(false) }
            var busy by remember(id) { mutableStateOf(false) }
            val asked = question
            if (asked == null) {
                SideEffect { deleting = null }
            } else {
                OverlayDialog(onDismiss = { deleting = null }, paneWidth = paneWidth) { overlay ->
                    DeleteListCard(
                        asked,
                        onDelete = {
                            if (overlay.open && !busy) {
                                busy = true
                                actions.deleteList(id, asked) { again ->
                                    busy = false
                                    if (again == null) {
                                        // Make room for the Undo toast.
                                        overlay.close()
                                        switcher = false
                                    } else {
                                        question = again
                                        changed = true
                                    }
                                }
                            }
                        },
                        onCancel = overlay::close,
                        changed = changed,
                        busy = busy,
                    )
                }
            }
        }
        val paneSurface = if (switcherVisible) state.copy(toast = null) else state
        val drag = remember(state.generation) { DragInput() }
        val dragModifier = taskDragInput(drag, actions, state.generation, switcherVisible, menu, { switcher = false })
        TaskDragSurface(drag, dragModifier) {
            Box(Modifier.fillMaxSize()) {
                val divider = Tokens.Elevation.HAIRLINE.dp
                val separated = fold?.takeIf { wide && it.start == paneWidth && it.end > it.start }
                val gap = separated?.let { it.end - it.start } ?: divider
                val feed = state.activity
                val fullBoard = paneWidth != null && state.board != null && detail == null && !showAi
                val fullTimeline = paneWidth != null && separated == null && state.timeline != null && detail == null && !showAi
                BackHandler(enabled = !sheetOpen) {
                    focus.clearFocus()
                    switcher = true
                }
                BackHandler(enabled = showAi && wide && !sheetOpen) { aiOpen = false }
                @Composable
                fun Chat(modifier: Modifier, handle: Modifier = Modifier, close: () -> Unit) {
                    if (ai == null) return
                    AssistantPane(ai, assistantModel, state.room.title, wide, hand.mirrors(wide), aiScroll,
                        overlay = { focus.clearFocus(); aiOverlayAtEnd = true; aiOverlay = it }, onClose = close,
                        onTask = { task ->
                            if (wide && state.view == ListView.LIST && state.room.tasks.any { it.id == task }) {
                                aiTaskLink = task // Keep chat open and bring a visible row into view.
                            } else {
                                aiOpen = false
                                actions.select(task)
                            }
                        }, modifier = modifier, handle = handle)
                }
                BackHandler(enabled = !sheetOpen && state.bulkIds.isNotEmpty()) { actions.clearBulk() }
                @Composable
                fun Tasks(modifier: Modifier) {
                    val shown = if (wide || detail == null) paneSurface else paneSurface.copy(toast = null)
                    CompositionLocalProvider(LocalAskAi provides ai?.let { AskAiControl(showAi, pending, toggleAi) }) {
                        RoomPane(
                            shown,
                            taskActions, menu, modifier, draft, collapsed,
                            onRoomMenu = { roomActionsSettings = false; roomActionsOpen = true }, onBulkPicker = { bulkPicker = it },
                            composerFocus = composerFocus, wide = wide, hand = hand, sheetOpen = sheetOpen,
                            onListSettings = { roomActionsSettings = true; roomActionsOpen = true }, fullBoard = fullBoard,
                            boardFold = separated, boardInput = boardInput, listWidth = paneWidth,
                            timelineSheets = timelineSheets, timelineGallery = timelineGallery, revealTask = aiTaskLink,
                            onOpenUpdates = if (!sheetOpen && (wide || detail == null)) {{ updatesOpen = true }} else null,
                            reachExpanded = reachExpanded, onSwipeRooms = { progress ->
                                if (progress == null && !switcher && reveal != null) returningGesture = true
                                else { returningGesture = false; reveal = progress }
                            },
                        )
                    }
                }
                Box(Modifier.fillMaxSize().background(r.pane)
                    .then(if (sheetOpen) Modifier.clearAndSetSemantics { hideFromAccessibility() } else Modifier)) {
                    if (feed != null || state.room.key == RoomKey.Trash) {
                        @Composable
                        fun HistoryRoom() {
                            CompositionLocalProvider(LocalAskAi provides ai?.let { AskAiControl(showAi, pending, toggleAi) }) {
                                if (feed != null) ActivityRoom(paneSurface, taskActions, menu, wide, hand)
                                else TrashRoom(paneSurface, taskActions, menu, wide, hand)
                            }
                        }
                        BackHandler(enabled = !sheetOpen && !showAi) {
                            actions.open(if (feed != null) state.room.key else RoomKey.Smart(app.todoornot.core.SmartList.MY_DAY))
                        }
                        if (showAi && paneWidth != null) Row(Modifier.fillMaxSize()) {
                            Box(Modifier.width(paneWidth)) { HistoryRoom() }
                            Spacer(Modifier.fillMaxHeight().width(gap).background(r.line))
                            Chat(Modifier.weight(1f).statusBarsPadding(), close = { aiOpen = false })
                        } else HistoryRoom()
                    } else if (paneWidth != null) {
                        Row(Modifier.fillMaxSize()) {
                            Tasks(if (fullBoard || fullTimeline) Modifier.fillMaxSize() else Modifier.width(paneWidth))
                            if (!fullBoard && !fullTimeline) {
                                Spacer(Modifier.fillMaxHeight().width(gap).background(r.line))
                                // Android sets status-icon contrast for the whole window, so the right
                                // status inset shares dusk's zenith while the detail keeps its own theme.
                                val insets = WindowInsets.statusBars
                                val duskTop = LocalDesign.current.dusk().top
                                val detailInset = Modifier.weight(1f).drawBehind {
                                    if (!dark && (motion.dusk(state.room.key)?.nightInk ?: state.room.finished)) {
                                        drawRect(duskTop, size = Size(size.width, insets.getTop(this).toFloat()))
                                    }
                                }
                                if (showAi) Chat(detailInset.statusBarsPadding(), close = { aiOpen = false })
                                else if (detail != null) DetailPane(paneSurface, actions, standalone = false,
                                    detailInset.statusBarsPadding(), hand = hand, sheets = detailSheets)
                                else NoDetail(detailInset.navigationBarsPadding(), selection = state.bulk?.label)
                            }
                        }
                    } else {
                        Tasks(Modifier.fillMaxSize().then(if (detail != null) Modifier.clearAndSetSemantics { hideFromAccessibility() } else Modifier))
                        if (detail != null) {
                            BackHandler(enabled = !sheetOpen) { actions.select(null) }
                            DetailPane(paneSurface, actions, standalone = true, hand = hand, sheets = detailSheets)
                        }
                    }
                }
                CompositionLocalProvider(LocalSheetWidth provides paneWidth, LocalOpenAiSettings provides openAiSettings.takeIf { ai != null }) {
                    if (switcherVisible) Box(if (navMenu != null) Modifier.clearAndSetSemantics { hideFromAccessibility() } else Modifier) {
                        BottomRoomSwitcherSheet(
                            state, server, actions, hand, wide, onHand, { switcher = false; reveal = null; returningGesture = false },
                            onMenu = { navMenu = it }, moving = moving, onNewCollection = { newCollection = true },
                            initialFilter = switcherFilter, initialSettings = settingsOpen, gestureReveal = reveal,
                        )
                    }
                    navMenu?.let { item -> RoomActionsSheet(item.name, drawerMenu(state.home, item.key), state, actions,
                        onRoomMenu, close = { navMenu = null }) }
                    if (roomActionsOpen) RoomActionsSheet(state.room.title, timelineRoomMenu(state), state, actions,
                        onRoomMenu, close = { roomActionsOpen = false },
                        initialPage = if (roomActionsSettings) (state.room.key as? RoomKey.List)?.let { MenuAction.ListSettings(it) } else null)
                    bulkPicker?.let { BulkSelectionPicker(it, state, actions) { bulkPicker = null } }
                    if (updatesOpen && updates != null) UpdatesSheet(updates) { updatesOpen = false }
                    if (timelineSheets.open) TimelineOverlays(state, timelineSheets, actions)
                }
                if (detail != null && feed == null && !showAi) CompositionLocalProvider(
                    LocalSheetWidth provides paneWidth?.let { windowWidth - it - gap }, LocalSheetAtEnd provides true,
                ) {
                    DetailOverlays(state, actions, detailSheets)
                }
                if (showAi && !wide) AssistantCover(onClose = { aiOpen = false }, blocked = showAiOverlay) { handle, close ->
                    val hidden = if (showAiOverlay) Modifier.clearAndSetSemantics { hideFromAccessibility() } else Modifier
                    Chat(hidden, handle, close)
                }
                if (ai != null && aiOverlay != null) {
                    val right = paneWidth != null && aiOverlayAtEnd
                    CompositionLocalProvider(LocalSheetWidth provides if (right) windowWidth - requireNotNull(paneWidth) - gap else paneWidth,
                        LocalSheetAtEnd provides right) {
                        when (aiOverlay) {
                            AssistantOverlay.SETTINGS, AssistantOverlay.DEVICE, AssistantOverlay.PRIVATE, AssistantOverlay.CHATGPT ->
                                AssistantSettings(ai, assistantModel, aiOverlay?.provider) { aiOverlay = null }
                            AssistantOverlay.HISTORY -> AssistantHistory(ai.panel.turns, { aiOverlay = null }) { index ->
                                scope.launch { if (index < aiScroll.layoutInfo.totalItemsCount) aiScroll.scrollToItem(index) }
                            }
                            null -> Unit
                        }
                    }
                }
            }
        }
    }
}

@Composable
private fun RoomActionsSheet(title: String, entries: List<MenuEntry>, state: UiState, actions: Actions,
    onChoose: (MenuAction) -> Unit, close: () -> Unit, initialPage: MenuAction? = null,
) {
    var page by rememberSaveable(stateSaver = OrganizeSaver) { mutableStateOf(initialPage) }
    BottomSheet(title, close) { overlay ->
        BackHandler(enabled = page != null) { page = null }
        AnimatedContent(page, modifier = Modifier.weight(1f, fill = false), transitionSpec = {
            fadeIn(tween(Tokens.Motion.CONTENT_SWAP)) togetherWith fadeOut(tween(Tokens.Motion.CONTENT_SWAP))
        }, label = "room action") { showing ->
            when (showing) {
                is MenuAction.ListSettings -> {
                    val features = state.home.featuresFor(showing.room)
                    if (features == null) SideEffect { overlay.close() }
                    else ListSettings(features, { actions.setListFeatures(showing.room.id, it) }, { page = null })
                }
                is MenuAction.MoveList -> MoveListCard(state.home, showing.room,
                    { id -> overlay.choose { actions.organize(OrganizeEdit.MoveList(showing.room.id, id)) } }, { page = null }, inSheet = true)
                is MenuAction.Colour -> {
                    val item = state.home.groups.asSequence().flatMap { listOfNotNull(it.collection) + it.lists }.firstOrNull { it.key == showing.room }
                    val target = when (val k = showing.room) {
                        is RoomKey.Collection -> ColorTarget.Collection(k.id)
                        is RoomKey.List -> ColorTarget.List(k.id)
                        else -> null
                    }
                    if (item == null || target == null) SideEffect { overlay.close() }
                    else SwatchCard(target, item.hue, { hue ->
                        overlay.choose { actions.organize(OrganizeEdit.Color(target, hue)) }
                    }, { page = null }, inSheet = true)
                }
                else -> ActionSheetContent(entries, title) { action ->
                    if (overlay.open) when (action) {
                        is MenuAction.MoveList, is MenuAction.Colour, is MenuAction.ListSettings -> page = action
                        else -> overlay.choose { onChoose(action) }
                    }
                }
            }
        }
    }
}
