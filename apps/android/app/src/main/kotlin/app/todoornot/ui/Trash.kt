package app.todoornot.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.runtime.Composable
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.input.nestedscroll.nestedScroll
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.dp
import app.todoornot.core.TrashKind
import app.todoornot.design.Glyph
import app.todoornot.design.Icon
import app.todoornot.design.LocalDesign
import app.todoornot.design.Tokens
import app.todoornot.model.Actions
import app.todoornot.model.UiState

@Composable
internal fun TrashRoom(state: UiState, actions: Actions, drawer: () -> Unit, wide: Boolean) {
    val trash = state.room.trash ?: return
    val r = LocalDesign.current.room
    val space = Tokens.Space
    val phone = Tokens.Metrics.Phone
    val collapse = remember(state.generation) { HeaderCollapse() }
    val pad = (if (wide) Tokens.Metrics.Desktop.LIST_PAD_X else phone.ROW_PAD_LEFT).dp
    Column(Modifier.fillMaxSize().background(r.pane).navigationBarsPadding().nestedScroll(collapse.connection)) {
        val landscape = if (wide) Tokens.Landscape.HEIGHT_INNER else Tokens.Landscape.HEIGHT_COVER
        RoomHeader(state.room, landscape.dp, collapse, drawer) { SyncChip(state.status, actions::nudge) }
        Label(trash.description, Tokens.Type.BODY, r.ink2,
            Modifier.fillMaxWidth().hairline(r.line).padding(horizontal = pad, vertical = space.S3.dp),
            maxLines = Int.MAX_VALUE)
        state.failed.firstOrNull()?.let { f ->
            Banner("Not saved: ${f.what}. ${f.reason}", "Dismiss") { actions.dismissFailed(f.id) }
        }
        if (state.signedOut) Banner("This phone isn't paired any more.", "Pair again", actions::unpair)
        LazyColumn(Modifier.weight(1f).fillMaxWidth()) {
            if (trash.rows.isEmpty()) item {
                Label(state.room.emptyLine, Tokens.Type.EMPTY_LINE, r.ink2,
                    Modifier.padding(horizontal = pad, vertical = space.S6.dp))
            }
            items(trash.rows, key = { "${it.kind}:${it.id}" }) { row ->
                Row(
                    Modifier.fillMaxWidth().heightIn(min = phone.ROW.dp).hairline(r.line)
                        .padding(horizontal = pad, vertical = space.S1.dp),
                    horizontalArrangement = Arrangement.spacedBy(space.S2.dp),
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    Box(Modifier.width(Tokens.Metrics.Component.ICON.dp), contentAlignment = Alignment.Center) {
                        when (row.kind) {
                            TrashKind.TASK -> Icon("circle-check", r.ink3)
                            TrashKind.LIST -> Icon("list", r.ink3)
                            TrashKind.COLLECTION -> Glyph("layers", r.ink3)
                        }
                    }
                    Column(Modifier.weight(1f)) {
                        Label(row.title, Tokens.Type.TASK_TITLE_PHONE, r.ink)
                        Label(row.metadata, Tokens.Type.META_PHONE, r.ink3)
                        row.blocked?.let { Label(it, Tokens.Type.META_PHONE, r.ink3, maxLines = 2) }
                    }
                    if (row.blocked == null) {
                        val shape = RoundedCornerShape(Tokens.Radius.BUTTON.dp)
                        Box(
                            Modifier.heightIn(min = phone.TOUCH.dp).clip(shape)
                                .border(Tokens.Elevation.HAIRLINE.dp, r.control, shape)
                                .semantics { contentDescription = "Restore ${row.title}" }
                                .clickable(enabled = !state.signedOut, role = Role.Button) { actions.restoreTrash(row) }
                                .padding(horizontal = space.S4.dp),
                            contentAlignment = Alignment.Center,
                        ) { Label("Restore", Tokens.Type.BUTTON, r.ink2) }
                    }
                }
            }
        }
        ToastStrip(state.toast, actions)
    }
}
