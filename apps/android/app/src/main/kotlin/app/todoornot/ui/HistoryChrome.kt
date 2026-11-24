package app.todoornot.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ColumnScope
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.RowScope
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.runtime.Composable
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import androidx.window.core.layout.WindowSizeClass
import app.todoornot.core.RoomView
import app.todoornot.design.LocalDesign
import app.todoornot.design.TodoTheme
import app.todoornot.design.Tokens
import app.todoornot.model.Actions
import app.todoornot.model.UiState
import app.todoornot.session.ThumbHand

/** The history rooms share their sky, the reserved Undo strip and the bottom room controls. */
@Composable
internal fun HistoryFrame(state: UiState, actions: Actions, room: RoomView, wide: Boolean,
    hand: ThumbHand, rooms: () -> Unit,
    activitySummary: String? = null,
    footer: @Composable RowScope.(mirrored: Boolean) -> Unit,
    body: @Composable ColumnScope.(HeaderCollapse) -> Unit,
) {
    TodoTheme(LocalDesign.current.dark, "agent-activity") {
        val r = LocalDesign.current.room
        val p = Tokens.Metrics.Phone
        val s = Tokens.Space
        val reduced = reducedMotion()
        val left = hand.mirrors(wide)
        BoxWithConstraints(Modifier.fillMaxSize().background(r.pane)) {
            val short = maxHeight < WindowSizeClass.HEIGHT_DP_MEDIUM_LOWER_BOUND.dp
            val header = remember(state.generation, room.title, short) { HeaderCollapse().apply {
                forced = if (short) 1f else 0f
            } }
            header.reducedMotion = reduced
            Column(Modifier.fillMaxSize().navigationBarsPadding()) {
                RoomHeader(room, wide, header, state.status, activitySummary = activitySummary) {}
                body(header)
                state.failed.firstOrNull()?.let { f ->
                    Banner("Not saved: ${f.what}. ${f.reason}", "Dismiss") { actions.dismissFailed(f.id) }
                }
                if (state.signedOut) Banner("This phone isn't paired any more.", "Pair again", actions::unpair)
                ToastStrip(state.toast, actions)
                Row(Modifier.fillMaxWidth().height(p.BOTTOM_BAR.dp).background(r.canvas)
                    .hairline(r.line, top = true).padding(horizontal = s.S1.dp),
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    if (left) AskAiButton() else IconButton("menu", "Rooms", onClick = rooms)
                    footer(left)
                    if (left) IconButton("menu", "Rooms", onClick = rooms) else AskAiButton()
                }
            }
        }
    }
}