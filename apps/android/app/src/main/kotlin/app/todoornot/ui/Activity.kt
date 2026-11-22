package app.todoornot.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import app.todoornot.core.ActivityRow
import app.todoornot.design.LocalDesign
import app.todoornot.design.Tokens
import app.todoornot.model.Actions
import app.todoornot.model.UiState
import java.time.Instant
import java.time.LocalDate
import java.time.ZoneId
import java.time.format.DateTimeFormatter
import java.util.Locale

/** The cursor advances even across empty pages. Changing filter cancels the previous room's read. */
@Composable
internal fun ActivityRoom(state: UiState, agent: String?, actions: Actions, drawer: () -> Unit) = key(agent) {
    var rows by remember { mutableStateOf(emptyList<ActivityRow>()) }
    var before by remember { mutableStateOf<ULong?>(null) }
    var end by remember { mutableStateOf(false) }
    var problem by remember { mutableStateOf<String?>(null) }
    var request by remember { mutableIntStateOf(0) }
    var revision by remember { mutableStateOf(state.activityRevision) }
    var loading by remember { mutableStateOf(true) }
    LaunchedEffect(request, state.activityRevision) {
        if (revision != state.activityRevision) { before = null; revision = state.activityRevision }
        loading = true
        val from = before
        val page = actions.activityPage(agent, from)
        if (page.problem == null || page.before != from) {
            rows = ((if (from == null) emptyList() else rows) + page.rows).distinctBy { it.id }
            end = page.before == null || (from != null && page.before == from)
        }
        before = page.before
        problem = page.problem
        loading = false
    }
    ActivityContent(state, agent, rows, problem, loading, end, ZoneId.systemDefault(), drawer, actions, { request++ }, before)
}

@Composable
internal fun ActivityContent(state: UiState, agent: String?, rows: List<ActivityRow>, problem: String?, loading: Boolean, end: Boolean, zone: ZoneId, drawer: () -> Unit, actions: Actions, more: () -> Unit, cursor: ULong? = null) {
    val r = LocalDesign.current.room
    val s = Tokens.Space
    Column(Modifier.fillMaxSize().background(r.pane).systemBarsPadding()) {
        Row(Modifier.fillMaxWidth().background(r.canvas).hairline(r.line), verticalAlignment = Alignment.CenterVertically) {
            IconButton("menu", "Rooms", onClick = drawer)
            Label("Activity", Tokens.Type.DETAIL_TITLE_PHONE, r.ink)
            agent?.let { Label(it, Tokens.Type.COUNT, r.ink3, Modifier.padding(start = s.S3.dp)) }
        }
        LazyColumn(Modifier.weight(1f).fillMaxWidth(), contentPadding = PaddingValues(s.S4.dp)) {
            val today = LocalDate.parse(state.today)
            val groups = rows.groupBy { Instant.ofEpochMilli(it.atMs).atZone(zone).toLocalDate() }
            for ((day, entries) in groups) {
                item("heading-$day") { Label(when (day) { today -> "Today"; today.minusDays(1) -> "Yesterday"; else -> day.format(DateTimeFormatter.ofPattern(if (day.year == today.year) "EEE d MMM" else "EEE d MMM yyyy", Locale.ENGLISH)) }, Tokens.Type.CAPS_LABEL, r.ink3, Modifier.padding(vertical = s.S3.dp)) }
                items(entries, key = { it.id }) { row ->
                    val outcome = state.eventStates[row.id]
                    Row(Modifier.fillMaxWidth().hairline(r.line).padding(vertical = s.S2.dp), verticalAlignment = Alignment.CenterVertically) {
                        Column(Modifier.weight(1f)) {
                            Label(row.summary, Tokens.Type.NAV, r.ink, maxLines = Int.MAX_VALUE)
                            Label("${row.actor} · ${row.relative}", Tokens.Type.META_PHONE, r.ink3)
                            if (outcome != null || row.undone) Label(outcome ?: "Undone", Tokens.Type.META_PHONE, r.ink2, maxLines = Int.MAX_VALUE)
                        }
                        if (row.canUndo && !row.undone && outcome == null) IconButton("undo-2", "Undo") { actions.undoEvent(row) }
                    }
                }
            }
            item {
                when {
                    loading -> Label("Loading activity…", Tokens.Type.META_PHONE, r.ink3)
                    problem != null -> Column { Label(problem, Tokens.Type.NOTES, r.ink2); Button("Try again", false, onClick = more) }
                    // A page can be empty after visibility filtering and still have a next cursor.
                    !end -> LaunchedEffect(cursor) { more() }
                    rows.isEmpty() -> Label(if (agent == null) "No activity yet." else "No activity from this agent.", Tokens.Type.EYEBROW_DAY, r.ink3)
                }
            }
        }
        ToastStrip(state.toast, actions)
    }
}
