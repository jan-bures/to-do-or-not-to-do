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
import app.todoornot.core.activityAgentName
import app.todoornot.design.LocalDesign
import app.todoornot.design.Tokens
import app.todoornot.model.Actions
import app.todoornot.model.UiState
import app.todoornot.model.activitySource
import java.time.Instant
import java.time.LocalDate
import java.time.ZoneOffset
import java.time.format.DateTimeFormatter
import java.util.Locale

private val shortDay = DateTimeFormatter.ofPattern("EEE d MMM", Locale.ENGLISH)
private val fullDay = DateTimeFormatter.ofPattern("d MMM yyyy", Locale.ENGLISH)

internal fun activityDay(atMs: Long, offsetSecs: Int): LocalDate =
    Instant.ofEpochMilli(atMs).atOffset(ZoneOffset.ofTotalSeconds(offsetSecs)).toLocalDate()

internal fun dayHeading(day: LocalDate, today: LocalDate): String = when (day) {
    today -> "Today"
    today.minusDays(1) -> "Yesterday"
    else -> day.format(if (day.year == today.year) shortDay else fullDay)
}

/** The cursor advances even across empty pages. Changing filter cancels the previous room's read. */
@Composable
internal fun ActivityRoom(state: UiState, actions: Actions, drawer: () -> Unit) = key(state.activitySource) {
    var rows by remember { mutableStateOf(emptyList<ActivityRow>()) }
    var before by remember { mutableStateOf<ULong?>(null) }
    var end by remember { mutableStateOf(false) }
    var problem by remember { mutableStateOf<String?>(null) }
    var request by remember { mutableIntStateOf(0) }
    var revision by remember { mutableStateOf(state.activityRevision) }
    var latest by remember { mutableStateOf(true) }
    var skip by remember { mutableStateOf(false) }
    var retired by remember { mutableStateOf(emptyMap<String, String>()) }
    LaunchedEffect(rows, state.activityRevision) { retired = actions.retiredEvents(rows.map { it.id }) }
    var loading by remember { mutableStateOf(true) }
    LaunchedEffect(request) {
        val reading = state.activityRevision
        loading = true
        val from = before
        val page = actions.activityPage(state.activitySource, from) ?: return@LaunchedEffect
        if (page.problem == null) {
            rows = ((if (latest) emptyList() else rows) + page.rows).distinctBy { it.id }
            if (latest) revision = reading
            latest = false
            end = page.before == null || (from != null && page.before == from)
        }
        before = page.before
        problem = page.problem
        skip = page.skip
        loading = false
    }
    ActivityContent(state, state.activitySource.agent, rows, problem, loading, end, drawer, actions,
        more = { request++ }, cursor = before, skip = skip, retired = retired,
        updates = state.activityRevision != revision, refresh = { latest = true; before = null; request++ })
}

@Composable
internal fun ActivityContent(state: UiState, agent: String?, rows: List<ActivityRow>, problem: String?, loading: Boolean, end: Boolean, drawer: () -> Unit, actions: Actions, more: () -> Unit, cursor: ULong? = null, skip: Boolean = false, retired: Map<String, String> = emptyMap(), updates: Boolean = false, refresh: () -> Unit = {}) {
    val r = LocalDesign.current.room
    val s = Tokens.Space
    Column(Modifier.fillMaxSize().background(r.pane).systemBarsPadding()) {
        Row(Modifier.fillMaxWidth().background(r.canvas).hairline(r.line), verticalAlignment = Alignment.CenterVertically) {
            IconButton("menu", "Rooms", onClick = drawer)
            Label("Activity", Tokens.Type.DETAIL_TITLE_PHONE, r.ink)
            agent?.let { Label(activityAgentName(it), Tokens.Type.COUNT, r.ink3, Modifier.padding(start = s.S3.dp)) }
        }
        Button(if (updates) "Refresh activity •" else "Refresh activity", false, enabled = !loading, onClick = refresh)
        LazyColumn(Modifier.weight(1f).fillMaxWidth(), contentPadding = PaddingValues(s.S4.dp)) {
            val today = LocalDate.parse(state.today)
            val groups = rows.groupBy { activityDay(it.atMs, state.utcOffsetSecs) }
            for ((day, entries) in groups) {
                item("heading-$day") {
                    Label(dayHeading(day, today), Tokens.Type.CAPS_LABEL, r.ink3, Modifier.padding(vertical = s.S3.dp))
                }
                items(entries, key = { it.id }) { row ->
                    val pending = row.group in state.pendingGroups || state.eventStates[row.id] == "Undoing…"
                    val resolved = retired[row.id]
                    val outcome = resolved ?: if (pending) "Undoing…" else state.eventStates[row.id]
                    Row(Modifier.fillMaxWidth().hairline(r.line).padding(vertical = s.S2.dp), verticalAlignment = Alignment.CenterVertically) {
                        Column(Modifier.weight(1f)) {
                            Label(row.summary, Tokens.Type.NAV, r.ink, maxLines = Int.MAX_VALUE)
                            Label("${row.actor} · ${row.relative}", Tokens.Type.META_PHONE, r.ink3)
                            if (outcome != null || row.undone) Label(outcome ?: "Undone", Tokens.Type.META_PHONE, r.ink2, maxLines = Int.MAX_VALUE)
                        }
                        if (row.canUndo && !row.undone && resolved == null && !pending) IconButton("undo-2", "Undo") { actions.undoEvent(row) }
                    }
                }
            }
            item {
                when {
                    loading -> Label("Loading activity…", Tokens.Type.META_PHONE, r.ink3)
                    problem != null -> Column { Label(problem, Tokens.Type.NOTES, r.ink2); Button(if (skip) "Skip this change" else "Try again", false, onClick = more) }
                    // A page can be empty after visibility filtering and still have a next cursor.
                    !end -> LaunchedEffect(cursor) { more() }
                    rows.isEmpty() -> Label(if (agent == null) "No activity yet." else "No activity from this agent.", Tokens.Type.EYEBROW_DAY, r.ink3)
                }
            }
        }
        ToastStrip(state.toast, actions)
    }
}
