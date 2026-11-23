package app.todoornot.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.LazyRow
import androidx.compose.foundation.selection.selectable
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.lazy.items
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.input.nestedscroll.nestedScroll
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.unit.dp
import app.todoornot.core.ActivityFooter
import app.todoornot.core.ActivityLoad
import app.todoornot.core.Eyebrow
import app.todoornot.core.RoomSummary
import app.todoornot.session.ThumbHand
import app.todoornot.design.LocalDesign
import app.todoornot.design.Tokens
import app.todoornot.model.Actions
import app.todoornot.model.UiState

@Composable
internal fun ActivityRoom(state: UiState, actions: Actions, drawer: () -> Unit, wide: Boolean = false, hand: ThumbHand = ThumbHand.RIGHT) {
    val feed = state.feed ?: return
    // A cursor advance leaves the list and scroll position alone. Only a different room or log replaces it.
    key(state.pagerId, feed.resetCount) {
        val load = { kind: ActivityLoad -> actions.loadActivity(state.pagerId, kind) }
        LaunchedEffect(Unit) { load(ActivityLoad.INITIAL) }
        val s = Tokens.Space
        val title = state.room.copy(title = "Activity", eyebrow = Eyebrow.Kind("History", feed.agent?.let { "Agent: $it" } ?: "All changes"),
            done = 0u, total = 0u, finished = false, summary = RoomSummary.None)
        HistoryFrame(state, actions, title, wide, hand, drawer, footer = { left ->
            val refresh: @Composable () -> Unit = {
                IconButton("refresh-cw", feed.refreshLabel, enabled = !feed.refreshing) { load(ActivityLoad.REFRESH) }
            }
            if (left) refresh()
            ActivityFilters(state, actions, Modifier.weight(1f))
            if (!left) refresh()
        }) { collapse ->
            val r = LocalDesign.current.room
            LazyColumn(Modifier.weight(1f).fillMaxWidth().nestedScroll(collapse.connection), contentPadding = PaddingValues(s.S4.dp)) {
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
                            row.undoLabel?.let { IconButton("undo-2", it) { actions.undoEvent(state.pagerId, row) } }
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
                        is ActivityFooter.Empty -> Label(footer.label, Tokens.Type.EMPTY_LINE, r.ink2, maxLines = 3)
                    }
                }
            }
            if (feed.refreshLabel.endsWith('•')) {
                Box(Modifier.fillMaxWidth().hairline(r.line, top = true).padding(horizontal = s.S3.dp, vertical = s.S1.dp)) {
                    Button(feed.refreshLabel, false, enabled = !feed.refreshing) { load(ActivityLoad.REFRESH) }
                }
            }
        }
    }
}

@Composable
private fun ActivityFilters(state: UiState, actions: Actions, modifier: Modifier) {
    val p = Tokens.Metrics.Phone
    val agents = state.home.agents
    val chosen = state.activity.orEmpty()
    val extra = chosen.takeIf { it.isNotEmpty() && agents.none { a -> a.id == it } }
    val list = androidx.compose.foundation.lazy.rememberLazyListState()
    LaunchedEffect(chosen, agents.map { it.id }) {
        if (chosen.isNotEmpty()) {
            val index = agents.indexOfFirst { it.id == chosen }.takeIf { it >= 0 }?.plus(1) ?: agents.size + 1
            if (list.layoutInfo.visibleItemsInfo.none { it.index == index }) list.scrollToItem(index)
        }
    }
    LazyRow(modifier.height(p.TOUCH.dp), state = list, verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(Tokens.Space.S1.dp)) {
        item("all") { ActivityPill("All", chosen.isEmpty(), { actions.openActivity(null) }) }
        for (agent in agents) item(agent.id) {
            ActivityPill(agent.mark.name, chosen == agent.id, { actions.openActivity(agent.id) }) { AgentBadge(agent.mark) }
        }
        extra?.let { item(it) { ActivityPill(state.feed?.agent ?: "Agent", true, { actions.openActivity(it) }) } }
    }
}

@Composable
private fun ActivityPill(name: String, on: Boolean, choose: () -> Unit, mark: (@Composable () -> Unit)? = null) {
    val r = LocalDesign.current.room
    val s = Tokens.Space
    val shape = RoundedCornerShape(Tokens.Radius.ROOM_PILL.dp)
    Box(Modifier.height(Tokens.Metrics.Phone.TOUCH.dp).widthIn(min = Tokens.Metrics.Phone.TOUCH.dp).selectable(on, role = Role.Tab, onClick = choose), contentAlignment = Alignment.Center) {
        Row(Modifier.height(Tokens.Metrics.Phone.ROOM_PILL.dp).widthIn(min = Tokens.Metrics.Phone.TOUCH.dp).clip(shape).background(if (on) r.wash else r.raised)
            .then(if (on) Modifier.selectionBar(r.accentStrong) else Modifier).padding(horizontal = s.S2.dp),
            verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(s.S1.dp, Alignment.CenterHorizontally)) {
            mark?.invoke()
            Label(name, Tokens.Type.ROOM_PILL, if (on) r.accentStrong else r.ink2,
                weight = if (on) Tokens.Type.BUTTON.weight else null)
        }
    }
}
