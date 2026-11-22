package app.todoornot.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import app.todoornot.core.ActivityFooter
import app.todoornot.core.ActivityLoad
import app.todoornot.design.LocalDesign
import app.todoornot.design.Tokens
import app.todoornot.model.Actions
import app.todoornot.model.UiState

@Composable
internal fun ActivityRoom(state: UiState, actions: Actions, drawer: () -> Unit) {
    val feed = state.feed ?: return
    // A cursor advance leaves the list and scroll position alone. Only a different room or log replaces it.
    key(state.generation, feed.epoch) {
        val load = { kind: ActivityLoad -> actions.loadActivity(state.generation, kind) }
        LaunchedEffect(Unit) { load(ActivityLoad.INITIAL) }
        val r = LocalDesign.current.room
        val s = Tokens.Space
        Column(Modifier.fillMaxSize().background(r.pane).systemBarsPadding()) {
            Row(Modifier.fillMaxWidth().background(r.canvas).hairline(r.line), verticalAlignment = Alignment.CenterVertically) {
                IconButton("menu", "Rooms", onClick = drawer)
                Label("Activity", Tokens.Type.DETAIL_TITLE_PHONE, r.ink)
                feed.agent?.let { Label(it, Tokens.Type.COUNT, r.ink3, Modifier.padding(start = s.S3.dp)) }
            }
            Button(feed.refreshLabel, false, enabled = !feed.refreshing) { load(ActivityLoad.REFRESH) }
            LazyColumn(Modifier.weight(1f).fillMaxWidth(), contentPadding = PaddingValues(s.S4.dp)) {
                for (day in feed.days) {
                    item("heading-${day.heading}") {
                        Label(day.heading, Tokens.Type.CAPS_LABEL, r.ink3, Modifier.padding(vertical = s.S3.dp))
                    }
                    items(day.rows, key = { it.id }) { row ->
                        Row(Modifier.fillMaxWidth().hairline(r.line).padding(vertical = s.S2.dp), verticalAlignment = Alignment.CenterVertically) {
                            Column(Modifier.weight(1f)) {
                                Label(row.summary, Tokens.Type.NAV, r.ink, maxLines = Int.MAX_VALUE)
                                Label("${row.actor} · ${row.relative}", Tokens.Type.META_PHONE, r.ink3)
                                row.note?.let { Label(it, Tokens.Type.META_PHONE, r.ink2, maxLines = Int.MAX_VALUE) }
                            }
                            row.undoLabel?.let { IconButton("undo-2", it) { actions.undoEvent(state.generation, row) } }
                        }
                    }
                }
                item {
                    when (val footer = feed.footer) {
                        ActivityFooter.None -> Unit
                        is ActivityFooter.Loading -> Label(footer.label, Tokens.Type.META_PHONE, r.ink3)
                        is ActivityFooter.Older -> LaunchedEffect(footer.cursor) { load(ActivityLoad.OLDER) }
                        is ActivityFooter.Error -> Column {
                            Label(footer.message, Tokens.Type.NOTES, r.ink2)
                            Button(footer.action, false) { load(ActivityLoad.RETRY) }
                        }
                        is ActivityFooter.Empty -> Label(footer.label, Tokens.Type.EYEBROW_DAY, r.ink3)
                    }
                }
            }
            ToastStrip(state.toast, actions)
        }
    }
}
