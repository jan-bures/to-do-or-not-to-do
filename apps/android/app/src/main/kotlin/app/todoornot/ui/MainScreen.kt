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
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import app.todoornot.core.RoomKey
import app.todoornot.design.LocalDesign
import app.todoornot.design.TodoTheme
import app.todoornot.design.Tokens
import app.todoornot.model.Actions
import app.todoornot.model.MotionFrame
import app.todoornot.model.UiState
import kotlinx.coroutines.launch

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
        ModalNavigationDrawer(
            drawerState = drawer,
            scrimColor = r.toast.copy(alpha = 0.32f),
            drawerContent = {
                RoomDrawer(state, server, actions, ::open) { name ->
                    actions.createList(name)
                    scope.launch { drawer.close() }
                }
            },
        ) {
            val detail = state.detail
            if (wide) {
                Row(Modifier.fillMaxSize().background(r.pane)) {
                    RoomPane(state, actions, Tokens.Landscape.HEIGHT_INNER.dp, menu, Modifier.width(Tokens.Metrics.Phone.INNER_LIST.dp), draft, collapsed)
                    Box(Modifier.fillMaxHeight().width(Tokens.Elevation.HAIRLINE.dp).background(r.line))
                    if (detail != null) {
                        DetailPane(detail, actions, standalone = false, state.generation, Modifier.weight(1f).statusBarsPadding())
                    } else {
                        NoDetail(Modifier.weight(1f))
                    }
                }
            } else {
                Box(Modifier.fillMaxSize().background(r.pane)) {
                    RoomPane(state, actions, Tokens.Landscape.HEIGHT_COVER.dp, menu, Modifier.fillMaxSize(), draft, collapsed)
                    if (detail != null) {
                        BackHandler { actions.select(null) }
                        DetailPane(detail, actions, standalone = true, state.generation)
                    }
                }
            }
        }
    }
}
