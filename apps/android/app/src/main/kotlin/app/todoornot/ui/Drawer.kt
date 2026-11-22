package app.todoornot.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.RowScope
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.key
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.KeyboardCapitalization
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.semantics.selected
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.dp
import app.todoornot.core.AgentSummary
import app.todoornot.core.NavItem
import app.todoornot.core.RoomKey
import app.todoornot.core.SmartList
import app.todoornot.design.Glyph
import app.todoornot.design.Icon
import app.todoornot.design.LocalDesign
import app.todoornot.design.Tokens
import app.todoornot.design.style
import app.todoornot.model.Actions
import app.todoornot.model.MenuAction
import app.todoornot.model.drawerMenu
import app.todoornot.model.Tone
import app.todoornot.model.UiState
import app.todoornot.model.syncLine

private val S = Tokens.Space
private val P = Tokens.Metrics.Phone

/**
 * The rooms, in the order every client keeps (docs/design-language.md, *Sidebar order*): the smart lists,
 * with Activity and under it the agents seen this week right after Agent activity, then the top-level
 * lists and each collection with its lists (glyphs in each list's hue), then "+ New list" and "New
 * collection". A collection folds with its chevron. A
 * long press on a list or a collection opens its menu (rename, move, and for a list, delete), whose
 * choice goes to [onMenu]; the rows in [moving] glide to their new places. The footer says how syncing is
 * going, and its settings button holds the pairing, Sync now and Unpair.
 */
@Composable
fun RoomDrawer(
    state: UiState,
    server: String?,
    actions: Actions,
    onOpen: (RoomKey) -> Unit,
    onNewList: (String) -> Unit,
    onMenu: (MenuAction) -> Unit = {},
    moving: Set<RoomKey> = emptySet(),
    onActivity: (String?) -> Unit = {},
    onNewCollection: () -> Unit = {},
) {
    val r = LocalDesign.current.room
    val current = state.room.key.takeIf { state.activity == null }
    var menu by remember { mutableStateOf<RoomKey?>(null) }

    // A list's or a collection's row, with its long-press menu anchored to it.
    @Composable
    fun Room(item: NavItem, indent: Boolean, heading: Boolean = false) {
        Box {
            NavRow(item, item.key == current, indent, onOpen, heading, onLongPress = { menu = item.key })
            if (menu == item.key) {
                MenuPopup(drawerMenu(state.home, item.key), onMenu, onDismiss = { if (menu == item.key) menu = null }, start = true, title = item.name)
            }
        }
    }
    Column(
        Modifier
            .fillMaxHeight()
            // The desktop sidebar's width, plus room for touch-sized rows.
            .width(Tokens.Metrics.Desktop.SIDEBAR.dp + S.S10.dp)
            .background(r.canvas)
            .statusBarsPadding()
            .navigationBarsPadding(),
    ) {
        Column(Modifier.weight(1f).verticalScroll(rememberScrollState()).padding(horizontal = S.S3.dp, vertical = S.S3.dp)) {
            val smart = state.home.smart
            val activityAt = activityAt(smart)
            smart.forEachIndexed { i, item ->
                NavRow(item, item.key == current, indent = false, onOpen)
                if (i + 1 == activityAt) ActivityRows(state, onActivity)
            }
            if (activityAt == 0) ActivityRows(state, onActivity)
            state.home.groups.forEach { g ->
                val c = g.collection
                key(c?.key ?: "top") {
                    Column(Modifier.slideOnMove(c != null && c.key in moving)) {
                        Spacer(Modifier.height(S.S4.dp))
                        val id = (c?.key as? RoomKey.Collection)?.id
                        val folded = id != null && id in state.collapsed
                        c?.let {
                            // The chevron is its own 48 dp target beside the row, so folding never opens the room.
                            Row(verticalAlignment = Alignment.CenterVertically) {
                                Box(Modifier.weight(1f)) { Room(it, indent = false, heading = true) }
                                IconButton(
                                    if (folded) "chevron-right" else "chevron-down",
                                    if (folded) "Show ${it.name}" else "Hide ${it.name}",
                                    tint = r.ink3,
                                ) { id?.let(actions::toggleCollection) }
                            }
                        }
                        if (!folded) {
                            g.lists.forEach {
                                key(it.key) {
                                    Box(Modifier.slideOnMove(it.key in moving)) { Room(it, indent = c != null) }
                                }
                            }
                        }
                    }
                }
            }
            Spacer(Modifier.height(S.S2.dp))
            NewList(onNewList)
            Choice("New collection", onClick = onNewCollection)
        }
        if (state.toast != null) ToastStrip(state.toast, actions)
        Footer(state, server, actions)
    }
}

/** "+ New list": tap to name it, Done adds it and opens it. */
@Composable
private fun NewList(onCreate: (String) -> Unit) {
    val r = LocalDesign.current.room
    var naming by rememberSaveable { mutableStateOf(false) }
    var name by rememberSaveable { mutableStateOf("") }
    val focus = remember { FocusRequester() }
    TouchTarget({ naming = true }, Modifier.fillMaxWidth()) { press ->
        Row(
            Modifier
                .fillMaxWidth()
                .height(P.NAV_ROW.dp)
                .clip(RoundedCornerShape(Tokens.Radius.MD.dp))
                .then(press)
                .padding(start = S.S2.dp, end = S.S3.dp),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(S.S3.dp),
        ) {
            Icon("plus", r.ink3, size = Tokens.Metrics.Component.GLYPH.dp)
            if (!naming) {
                Label("New list", Tokens.Type.NAV_SUB, r.ink3, Modifier.weight(1f))
            } else {
                LaunchedEffect(Unit) { runCatching { focus.requestFocus() } }
                fun done() {
                    val n = name.trim()
                    if (n.isNotEmpty()) onCreate(n)
                    name = ""
                    naming = false
                }
                BasicTextField(
                    value = name,
                    onValueChange = { name = it.replace("\n", "") },
                    modifier = Modifier.weight(1f).focusRequester(focus).semantics { contentDescription = "New list name" },
                    textStyle = Tokens.Type.NAV.style(r.ink),
                    singleLine = true,
                    cursorBrush = SolidColor(r.accent),
                    keyboardOptions = KeyboardOptions(capitalization = KeyboardCapitalization.Sentences, imeAction = ImeAction.Done),
                    keyboardActions = KeyboardActions(onDone = { done() }),
                    decorationBox = { inner ->
                        Box(contentAlignment = Alignment.CenterStart) {
                            if (name.isEmpty()) Label("List name", Tokens.Type.NAV, r.ink3)
                            inner()
                        }
                    },
                )
            }
        }
    }
}

/** The sync line with its dot ("Synced 10:35", "Offline, 3 waiting"), and the settings menu. */
@Composable
private fun Footer(state: UiState, server: String?, actions: Actions) {
    val r = LocalDesign.current.room
    val (line, tone) = syncLine(state.status, state.syncedAt)
    Row(
        Modifier.fillMaxWidth().hairline(r.line, top = true).padding(start = S.S5.dp, end = S.S2.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(S.S2.dp),
    ) {
        val color = when (tone) {
            Tone.OK -> r.success
            Tone.BUSY -> r.accent
            Tone.PROBLEM -> r.danger
            Tone.IDLE -> r.ink4
        }
        Box(Modifier.size(Tokens.Metrics.Component.CHIP_DOT.dp).background(color, CircleShape))
        Label(line, Tokens.Type.COUNT, r.ink3, Modifier.weight(1f).semantics { contentDescription = "Sync: $line" })
        var menu by remember { mutableStateOf(false) }
        Box {
            IconButton("settings", "Settings") { menu = true }
            DropdownMenu(menu, { menu = false }, containerColor = r.raised) {
                DropdownMenuItem(
                    text = {
                        Column {
                            Label("Paired with", Tokens.Type.CAPS_LABEL, r.ink3)
                            Label(server?.removePrefix("https://") ?: "No server", Tokens.Type.DUE, r.ink)
                        }
                    },
                    onClick = {},
                    enabled = false,
                )
                DropdownMenuItem(
                    text = { Label("Sync now", Tokens.Type.BODY, r.ink) },
                    leadingIcon = { Icon("refresh-cw", r.ink2) },
                    onClick = { menu = false; actions.nudge() },
                )
                DropdownMenuItem(
                    text = { Label("Unpair this phone", Tokens.Type.BODY, r.danger) },
                    leadingIcon = { Icon("log-out", r.danger) },
                    onClick = { menu = false; actions.unpair() },
                )
            }
        }
    }
}

/**
 * Where Activity goes among the smart lists: right after Agent activity, or last if it isn't there (or
 * first, at 0, with no smart lists at all).
 */
internal fun activityAt(smart: List<NavItem>): Int {
    val agentActivity = smart.indexOfFirst { (it.key as? RoomKey.Smart)?.list == SmartList.AGENT_ACTIVITY }
    return if (agentActivity >= 0) agentActivity + 1 else smart.size
}

/**
 * Activity, a row like the smart lists', and under it, indented, the agents seen in the last week, each
 * with its letter mark and what it did today ("2 added, 1 edited today", "idle"), as the desktop's
 * sidebar shows them. Tap one to open Activity filtered to it.
 */
@Composable
private fun ActivityRows(state: UiState, onActivity: (String?) -> Unit) {
    val d = LocalDesign.current
    DrawerRow(on = state.activity == "", indent = false, onClick = { onActivity(null) }) {
        Icon("clock", d.list("agent-activity").glyph)
        Label("Activity", Tokens.Type.NAV, d.room.ink, Modifier.weight(1f))
    }
    state.home.agents.forEach { a -> AgentRow(a, state.activity == a.id) { onActivity(a.id) } }
}

/** An agent's row under Activity: its letter mark, its name and what it did today. */
@Composable
internal fun AgentRow(agent: AgentSummary, on: Boolean, onClick: () -> Unit) {
    val r = LocalDesign.current.room
    DrawerRow(
        on = on,
        indent = true,
        onClick = onClick,
        modifier = Modifier.semantics(mergeDescendants = true) { contentDescription = "Agent: ${agent.mark.name}, ${agent.today}" },
    ) {
        AgentBadge(agent.mark)
        Label(agent.mark.name, Tokens.Type.NAV_SUB, r.ink, Modifier.weight(1f))
        Label(agent.today, Tokens.Type.COUNT, r.ink3)
    }
}

/**
 * A drawer row: a compact highlight, raised while [on], inside a full touch target. Indented rows sit
 * under a collection or under Activity.
 */
@Composable
private fun DrawerRow(
    on: Boolean,
    indent: Boolean,
    onClick: () -> Unit,
    modifier: Modifier = Modifier,
    onLongPress: (() -> Unit)? = null,
    longClickLabel: String? = null,
    content: @Composable RowScope.() -> Unit,
) {
    val r = LocalDesign.current.room
    TouchTarget(
        onClick,
        modifier.fillMaxWidth().semantics { selected = on },
        onLongClick = onLongPress,
        longClickLabel = longClickLabel,
    ) { press ->
        Row(
            Modifier
                .fillMaxWidth()
                .height(P.NAV_ROW.dp)
                .clip(RoundedCornerShape(Tokens.Radius.MD.dp))
                .background(if (on) r.raised else r.canvas)
                .then(press)
                .padding(start = if (indent) S.S8.dp else S.S2.dp, end = S.S3.dp),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(S.S3.dp),
            content = content,
        )
    }
}

@Composable
internal fun NavRow(
    item: NavItem,
    on: Boolean,
    indent: Boolean,
    onOpen: (RoomKey) -> Unit,
    heading: Boolean = false,
    onLongPress: (() -> Unit)? = null,
) {
    val d = LocalDesign.current
    val r = d.room
    val l = d.list(item.hue)
    DrawerRow(
        on = on,
        indent = indent,
        onClick = { onOpen(item.key) },
        onLongPress = onLongPress,
        longClickLabel = if (onLongPress != null) "Options for ${item.name}" else null,
    ) {
        when {
            item.glyph != null -> Glyph(item.glyph!!, l.glyph)
            item.icon != null -> Icon(item.icon!!, l.glyph)
            else -> Box(Modifier.size(Tokens.Metrics.Component.GLYPH.dp))
        }
        val token = when {
            heading -> Tokens.Type.NAV_COLLECTION
            indent -> Tokens.Type.NAV_SUB
            else -> Tokens.Type.NAV
        }
        Label(item.name, token, r.ink, Modifier.weight(1f))
        if (item.open > 0u) Label("${item.open}", Tokens.Type.COUNT, r.ink3)
    }
}
