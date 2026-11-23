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
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.layout.width
import androidx.compose.ui.semantics.hideFromAccessibility
import androidx.compose.ui.semantics.clearAndSetSemantics
import androidx.compose.ui.unit.Dp
import app.todoornot.core.NavItem
import app.todoornot.model.MenuEntry
import app.todoornot.model.drawerMenu
import app.todoornot.model.roomMenu
import app.todoornot.session.ThumbHand
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.SideEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.Saver
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
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
        is MenuAction.Colour -> when (val room = action.room) {
            is RoomKey.List -> "list:${room.id}"
            is RoomKey.Collection -> "collection:${room.id}"
            else -> ""
        }
        else -> ""
    } },
    restore = { saved -> val id = saved.substringAfter(':'); when (saved.substringBefore(':')) {
        "move" -> MenuAction.MoveList(RoomKey.List(id))
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
fun MainScreen(
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
) {
    LaunchedEffect(motion) { motion.drive() }
    CompositionLocalProvider(LocalMotion provides motion) {
        BoxWithConstraints(Modifier.fillMaxSize()) {
            val paneWidth = listPaneWidth(maxWidth, wide, fold)
            Screen(state, actions, dark, wide, server, draft, drawerOpen, collapsed, composerFocus,
                hand, onHand, fold, paneWidth, switcherFilter, settingsOpen, roomMenuOpen, navMenuOpen, reachExpanded)
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
    switcherFilter: String,
    settingsOpen: Boolean,
    roomMenuOpen: Boolean,
    navMenuOpen: NavItem?,
    reachExpanded: Boolean,
) {
    TodoTheme(dark, state.room.hue) {
        val r = LocalDesign.current.room
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
        val menu = { switcher = true }
        var roomActionsOpen by remember(state.generation) { mutableStateOf(roomMenuOpen) }
        var navMenu by remember { mutableStateOf(navMenuOpen) }
        var bulkPicker by remember(state.generation, state.bulkIds) { mutableStateOf<BulkPicker?>(null) }
        val sheetOpen = switcherVisible || roomActionsOpen || bulkPicker != null
        state.unpairing?.let { q -> UnpairDialog(q.unsent, actions::cancelUnpair, { actions.confirmUnpair(q.unsent) }, q.changed) }
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
                val detail = state.detail
                val feed = state.activity
                BackHandler(enabled = !sheetOpen && state.bulkIds.isNotEmpty()) { actions.clearBulk() }
                @Composable
                fun Tasks(modifier: Modifier) {
                    RoomPane(
                        if (wide || detail == null) paneSurface else paneSurface.copy(toast = null),
                        actions, (if (wide) Tokens.Landscape.HEIGHT_INNER else Tokens.Landscape.HEIGHT_COVER).dp,
                        menu, modifier, draft, collapsed, onRoomMenu = { roomActionsOpen = true }, onBulkPicker = { bulkPicker = it },
                        composerFocus = composerFocus, wide = wide, hand = hand, sheetOpen = sheetOpen,
                        reachExpanded = reachExpanded, onSwipeRooms = { progress ->
                            if (progress == null && !switcher && reveal != null) returningGesture = true
                            else { returningGesture = false; reveal = progress }
                        },
                    )
                }
                Box(Modifier.fillMaxSize().background(r.pane)
                    .then(if (sheetOpen) Modifier.clearAndSetSemantics { hideFromAccessibility() } else Modifier)) {
                    if (feed != null) {
                        BackHandler(enabled = !sheetOpen) { actions.open(state.room.key) }
                        ActivityRoom(paneSurface, actions, menu)
                    } else if (state.room.key == RoomKey.Trash) {
                        BackHandler(enabled = !sheetOpen) { actions.open(RoomKey.Smart(app.todoornot.core.SmartList.MY_DAY)) }
                        TrashRoom(paneSurface, actions, menu, wide)
                    } else if (paneWidth != null) {
                        Row(Modifier.fillMaxSize()) {
                            Tasks(Modifier.width(paneWidth))
                            Spacer(Modifier.fillMaxHeight().width(separated?.let { it.end - it.start } ?: divider).background(r.line))
                            if (detail != null) DetailPane(paneSurface, actions, standalone = false, Modifier.weight(1f).statusBarsPadding())
                            else NoDetail(Modifier.weight(1f).navigationBarsPadding(), selection = state.bulk?.label)
                        }
                    } else {
                        Tasks(Modifier.fillMaxSize())
                        if (detail != null) {
                            BackHandler(enabled = !sheetOpen) { actions.select(null) }
                            DetailPane(paneSurface, actions, standalone = true)
                        }
                    }
                }
                CompositionLocalProvider(LocalSheetWidth provides paneWidth) {
                    if (switcherVisible) Box(if (navMenu != null) Modifier.clearAndSetSemantics { hideFromAccessibility() } else Modifier) {
                        BottomRoomSwitcherSheet(
                            state, server, actions, hand, wide, onHand, { switcher = false; reveal = null; returningGesture = false },
                            onMenu = { navMenu = it }, moving = moving, onNewCollection = { newCollection = true },
                            initialFilter = switcherFilter, initialSettings = settingsOpen, gestureReveal = reveal,
                        )
                    }
                    navMenu?.let { item -> RoomActionsSheet(item.name, drawerMenu(state.home, item.key), state, actions,
                        onRoomMenu, close = { navMenu = null }) }
                    if (roomActionsOpen) RoomActionsSheet(state.room.title, roomMenu(state.home, state.room.key, state.showCompleted), state, actions,
                        onRoomMenu, close = { roomActionsOpen = false })
                    bulkPicker?.let { BulkSelectionPicker(it, state, actions) { bulkPicker = null } }
                }
            }
        }
    }
}

@Composable
private fun RoomActionsSheet(title: String, entries: List<MenuEntry>, state: UiState, actions: Actions,
    onChoose: (MenuAction) -> Unit, close: () -> Unit,
) {
    var page by rememberSaveable(stateSaver = OrganizeSaver) { mutableStateOf<MenuAction?>(null) }
    BottomSheet(title, close) { overlay ->
        BackHandler(enabled = page != null) { page = null }
        AnimatedContent(page, modifier = Modifier.weight(1f, fill = false), transitionSpec = {
            fadeIn(tween(Tokens.Motion.CONTENT_SWAP)) togetherWith fadeOut(tween(Tokens.Motion.CONTENT_SWAP))
        }, label = "room action") { showing ->
            when (showing) {
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
                        is MenuAction.MoveList, is MenuAction.Colour -> page = action
                        else -> overlay.choose { onChoose(action) }
                    }
                }
            }
        }
    }
}
