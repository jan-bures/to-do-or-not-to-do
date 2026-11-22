package app.todoornot.ui

import androidx.activity.compose.BackHandler
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.layout.width
import androidx.compose.material3.DrawerValue
import androidx.compose.material3.ModalNavigationDrawer
import androidx.compose.material3.rememberDrawerState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.SideEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.saveable.Saver
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import app.todoornot.core.RoomKey
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
import kotlinx.coroutines.launch

/** Keeps a list's or a collection's room across a recreation (a rename dialog's target). */
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
) {
    LaunchedEffect(motion) { motion.drive() }
    CompositionLocalProvider(LocalMotion provides motion) { Screen(state, actions, dark, wide, server, draft, drawerOpen, collapsed) }
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
) {
    TodoTheme(dark, state.room.hue) {
        val r = LocalDesign.current.room
        val drawer = rememberDrawerState(if (drawerOpen) DrawerValue.Open else DrawerValue.Closed)
        val scope = rememberCoroutineScope()
        fun open(key: RoomKey) {
            actions.open(key)
            scope.launch { drawer.close() }
        }
        val menu = { scope.launch { drawer.open() }; Unit }
        state.unpairing?.let { q -> UnpairDialog(q.unsent, actions::cancelUnpair, { actions.confirmUnpair(q.unsent) }, q.changed) }
        // The list and collection menus (the drawer's long press, the room's ⋯): rename and delete ask in a
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
        val onRoomMenu: (MenuAction) -> Unit = { a ->
            when (a) {
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
                OverlayDialog(onDismiss = { renaming = null }) { overlay ->
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
                OverlayDialog(onDismiss = { deleting = null }) { overlay ->
                    DeleteListCard(
                        asked,
                        onDelete = {
                            if (overlay.open && !busy) {
                                busy = true
                                actions.deleteList(id, asked) { again ->
                                    busy = false
                                    if (again == null) {
                                        // Done: the toast with Undo is on the room, so the drawer makes way.
                                        overlay.close()
                                        scope.launch { drawer.close() }
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
        ModalNavigationDrawer(
            drawerState = drawer,
            scrimColor = r.toast.copy(alpha = 0.32f),
            drawerContent = {
                RoomDrawer(
                    state,
                    server,
                    actions,
                    ::open,
                    onNewList = { name ->
                        actions.createList(name)
                        scope.launch { drawer.close() }
                    },
                    onMenu = onRoomMenu,
                    moving = moving,
                )
            },
        ) {
            val detail = state.detail
            if (wide) {
                Row(Modifier.fillMaxSize().background(r.pane)) {
                    RoomPane(
                        state, actions, Tokens.Landscape.HEIGHT_INNER.dp, menu, Modifier.width(Tokens.Metrics.Phone.INNER_LIST.dp), draft, collapsed,
                        onRoomMenu = onRoomMenu,
                    )
                    Box(Modifier.fillMaxHeight().width(Tokens.Elevation.HAIRLINE.dp).background(r.line))
                    if (detail != null) {
                        DetailPane(detail, actions, standalone = false, state.generation, Modifier.weight(1f).statusBarsPadding(), today = state.today)
                    } else {
                        NoDetail(Modifier.weight(1f))
                    }
                }
            } else {
                Box(Modifier.fillMaxSize().background(r.pane)) {
                    RoomPane(state, actions, Tokens.Landscape.HEIGHT_COVER.dp, menu, Modifier.fillMaxSize(), draft, collapsed, onRoomMenu = onRoomMenu)
                    if (detail != null) {
                        BackHandler { actions.select(null) }
                        DetailPane(detail, actions, standalone = true, state.generation, today = state.today, toast = state.toast)
                    }
                }
            }
        }
    }
}
