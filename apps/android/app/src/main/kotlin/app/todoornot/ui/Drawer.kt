package app.todoornot.ui

import android.content.ActivityNotFoundException
import android.content.Intent
import android.net.Uri
import android.widget.Toast
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.RowScope
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.imePadding
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
import androidx.compose.ui.input.key.Key
import androidx.compose.ui.input.key.KeyEventType
import androidx.compose.ui.input.key.key as keyboardKey
import androidx.compose.ui.input.key.onPreviewKeyEvent
import androidx.compose.ui.input.key.type
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.clearAndSetSemantics
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.role
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.KeyboardCapitalization
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.semantics.selected
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.dp
import app.todoornot.Cleartext
import androidx.compose.ui.layout.boundsInRoot
import androidx.compose.ui.layout.onGloballyPositioned
import app.todoornot.core.AgentSummary
import app.todoornot.core.NavItem
import app.todoornot.core.RoomKey
import app.todoornot.core.SmartList
import app.todoornot.core.SyncState
import app.todoornot.core.SyncTone
import app.todoornot.core.adminUrl
import app.todoornot.design.Glyph
import app.todoornot.design.Icon
import app.todoornot.design.LocalDesign
import app.todoornot.design.Tokens
import app.todoornot.design.style
import app.todoornot.model.Actions
import app.todoornot.model.MenuAction
import app.todoornot.model.drawerMenu
import app.todoornot.model.UiState
import app.todoornot.model.syncLine

private val S = Tokens.Space
private val P = Tokens.Metrics.Phone

/**
 * The rooms, in the order every client keeps (docs/design-language.md, *Sidebar order*): the smart lists,
 * with Activity and today's agents right after Agent activity, then each collection with its lists,
 * then loose lists. A collection folds with its chevron. A long press on a list or a collection opens
 * its menu, whose choice goes to [onMenu]; the rows in [moving] glide to their new places. The pinned
 * footer holds New, the sync status, and settings (admin pages, Sync now, pairing and Unpair).
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
    var namingList by rememberSaveable { mutableStateOf(false) }
    val newFocus = remember { FocusRequester() }

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
            // Shrink the scrolling rooms, keeping the pinned name editor above the keyboard.
            .navigationBarsPadding()
            .imePadding(),
    ) {
        val drag = LocalTaskDrag.current
        val scroll = rememberScrollState()
        Column(Modifier.weight(1f).onGloballyPositioned { coords ->
            drag?.drawer = DragInput.ScrollPane(coords.boundsInRoot()) { scroll.dispatchRawDelta(it) }
        }.verticalScroll(scroll).padding(horizontal = S.S3.dp, vertical = S.S3.dp)) {
            val smart = state.home.smart
            val activityAt = activityAt(smart)
            smart.forEachIndexed { i, item ->
                NavRow(item, item.key == current, indent = false, onOpen)
                if (i + 1 == activityAt) HistoryRows(state, current, onActivity, onOpen)
            }
            if (activityAt == 0) HistoryRows(state, current, onActivity, onOpen)
            // Home supplies the display order: collection groups, then the loose lists.
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
        }
        if (namingList) {
            NewList(
                onCreate = { name -> namingList = false; onNewList(name) },
                onCancel = { namingList = false; newFocus.requestFocus() },
            )
        }
        if (state.toast != null) ToastStrip(state.toast, actions)
        Footer(state, server, actions, newFocus, { namingList = true }, onNewCollection)
    }
}

/** The inline list name editor. Done adds a non-empty name and opens the list. */
@Composable
private fun NewList(onCreate: (String) -> Unit, onCancel: () -> Unit) {
    val r = LocalDesign.current.room
    var name by rememberSaveable { mutableStateOf("") }
    val focus = remember { FocusRequester() }
    LaunchedEffect(Unit) { focus.requestFocus() }
    Row(
        Modifier.fillMaxWidth().padding(start = S.S4.dp, end = S.S2.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(S.S3.dp),
    ) {
        Icon("plus", r.ink3, size = Tokens.Metrics.Component.GLYPH.dp)
        BasicTextField(
            value = name,
            onValueChange = { name = it.replace("\n", "") },
            modifier = Modifier.weight(1f).focusRequester(focus).semantics { contentDescription = "New list name" }
                .onPreviewKeyEvent { ev ->
                    if (ev.keyboardKey != Key.Escape) false else {
                        if (ev.type == KeyEventType.KeyUp) onCancel()
                        true
                    }
                },
            textStyle = Tokens.Type.NAV.style(r.ink),
            singleLine = true,
            cursorBrush = SolidColor(r.accent),
            keyboardOptions = KeyboardOptions(capitalization = KeyboardCapitalization.Sentences, imeAction = ImeAction.Done),
            keyboardActions = KeyboardActions(onDone = {
                val n = name.trim()
                if (n.isNotEmpty()) onCreate(n) else onCancel()
            }),
            decorationBox = { inner ->
                Box(contentAlignment = Alignment.CenterStart) {
                    if (name.isEmpty()) Label("List name", Tokens.Type.NAV, r.ink3)
                    inner()
                }
            },
        )
        IconButton("x", "Cancel new list", onClick = onCancel)
    }
}

@Composable
private fun NewMenu(trigger: FocusRequester, onNewList: () -> Unit, onNewCollection: () -> Unit) {
    val r = LocalDesign.current.room
    var open by remember { mutableStateOf(false) }
    val list = remember { FocusRequester() }
    val collection = remember { FocusRequester() }
    fun dismiss() {
        open = false
        trigger.requestFocus()
    }
    Box {
        TouchTarget(
            onClick = { open = true },
            modifier = Modifier.focusRequester(trigger).semantics { role = Role.Button }
                .onPreviewKeyEvent { ev ->
                    if (ev.keyboardKey !in listOf(Key.Enter, Key.Spacebar)) false else {
                        if (ev.type == KeyEventType.KeyUp) open = true
                        true
                    }
                },
        ) { press ->
            Row(
                Modifier.height(P.NAV_ROW.dp).clip(RoundedCornerShape(Tokens.Radius.MD.dp))
                    .then(press).padding(horizontal = S.S2.dp),
                verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.spacedBy(S.S2.dp),
            ) {
                Icon("plus", r.ink3, size = Tokens.Metrics.Component.ICON_SMALL.dp)
                Label("New", Tokens.Type.NAV_SUB, r.ink3)
            }
        }
        DropdownMenu(open, ::dismiss, containerColor = r.raised) {
            if (open) LaunchedEffect(Unit) { list.requestFocus() }
            @Composable
            fun Item(label: String, icon: String, focus: FocusRequester, other: FocusRequester, choose: () -> Unit) {
                val select = { open = false; choose() }
                DropdownMenuItem(
                    text = { Label(label, Tokens.Type.NAV_SUB, r.ink) },
                    leadingIcon = { Icon(icon, r.ink2) },
                    modifier = Modifier.focusRequester(focus).onPreviewKeyEvent { ev ->
                        val key = ev.keyboardKey
                        if (key !in listOf(Key.DirectionUp, Key.DirectionDown, Key.Escape, Key.Enter, Key.Spacebar)) {
                            false
                        } else {
                            if (ev.type == KeyEventType.KeyUp) when (key) {
                                Key.Escape -> dismiss()
                                Key.Enter, Key.Spacebar -> select()
                                else -> other.requestFocus()
                            }
                            true
                        }
                    },
                    onClick = select,
                )
            }
            Item("List", "plus", list, collection, onNewList)
            Item("Collection", "folder-plus", collection, list, onNewCollection)
        }
    }
}

/** The single pinned row: New, the sync line, and the settings menu. */
@Composable
private fun Footer(
    state: UiState,
    server: String?,
    actions: Actions,
    newFocus: FocusRequester,
    onNewList: () -> Unit,
    onNewCollection: () -> Unit,
) {
    val r = LocalDesign.current.room
    val line = syncLine(state.status, state.syncedAt)
    Row(
        Modifier.fillMaxWidth().hairline(r.line, top = true).padding(horizontal = S.S2.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(S.S2.dp),
    ) {
        val color = when (line.tone) {
            SyncTone.OK -> r.success
            SyncTone.BUSY -> r.accent
            SyncTone.PROBLEM -> r.danger
            SyncTone.IDLE -> r.ink4
        }
        NewMenu(newFocus, onNewList, onNewCollection)
        Row(
            Modifier.weight(1f).clearAndSetSemantics { contentDescription = line.full },
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(S.S2.dp, Alignment.CenterHorizontally),
        ) {
            Box(Modifier.size(Tokens.Metrics.Component.CHIP_DOT.dp).background(color, CircleShape))
            Label(line.short, Tokens.Type.COUNT, r.ink3)
        }
        DrawerSettings(server, state.status.state == SyncState.SYNCING, actions)
    }
}

@Composable
internal fun DrawerSettings(server: String?, syncing: Boolean, actions: Actions) {
    val r = LocalDesign.current.room
    val context = LocalContext.current
    val admin = remember(server) { server?.let { adminUrl(it, Cleartext.allowLoopbackHttp) } }
    val trigger = remember { FocusRequester() }
    var menu by remember { mutableStateOf(false) }
    var fromLast by remember { mutableStateOf(false) }
    // Only the dialog page is saved. Credentials and scan state stay in the session model.
    var pairing by rememberSaveable { mutableStateOf<DevicePairingPage?>(null) }
    val owner = LocalPairingOwner.current
    fun dismiss(restoreFocus: Boolean = true) {
        menu = false
        if (restoreFocus) trigger.requestFocus()
    }
    if (owner != null) pairing?.let { page ->
        DevicePairingDialog(owner, page, onPage = { pairing = it }, onClose = {
            pairing = null
            trigger.requestFocus()
        })
    }
    Box {
        IconButton("settings", "Settings", expanded = menu, modifier = Modifier.focusRequester(trigger)
            .onPreviewKeyEvent { ev ->
                val key = ev.keyboardKey
                if (key !in listOf(Key.Enter, Key.Spacebar, Key.DirectionUp, Key.DirectionDown)) false else {
                    if (ev.type == KeyEventType.KeyUp) {
                        fromLast = key == Key.DirectionUp
                        menu = true
                    }
                    true
                }
            }) { fromLast = false; menu = true }
        DropdownMenu(menu, { dismiss() }, containerColor = r.raised) {
            SettingsMenuContent(
                server, syncing, fromLast.takeIf { menu },
                onDismiss = { dismiss() },
                onAdmin = admin?.let { url ->
                    {
                        if (menu) {
                            dismiss()
                            val intent = Intent(Intent.ACTION_VIEW, Uri.parse(url)).addCategory(Intent.CATEGORY_BROWSABLE)
                            try {
                                context.startActivity(intent)
                            } catch (_: ActivityNotFoundException) {
                                Toast.makeText(context, "No browser can open the admin pages.", Toast.LENGTH_SHORT).show()
                            } catch (_: SecurityException) {
                                Toast.makeText(context, "This phone won't open the admin pages.", Toast.LENGTH_SHORT).show()
                            }
                        }
                    }
                },
                onSync = {
                    if (menu && !syncing) {
                        dismiss()
                        actions.nudge()
                    }
                },
                onDevices = owner?.let { { dismiss(restoreFocus = false); pairing = DevicePairingPage.SETTINGS } },
                onUnpair = { dismiss(); actions.unpair() },
            )
        }
    }
}

private data class SettingsItem(
    val label: String, val icon: String, val enabled: Boolean = true, val danger: Boolean = false, val choose: () -> Unit,
)

/** Shared by the popup and the in-place Paparazzi gallery (which can't render popup windows). */
@Composable
internal fun SettingsMenuContent(
    server: String?, syncing: Boolean, fromLast: Boolean?, onDismiss: () -> Unit,
    onAdmin: (() -> Unit)?, onSync: () -> Unit, onDevices: (() -> Unit)?, onUnpair: () -> Unit,
) {
    val r = LocalDesign.current.room
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
    val items = listOfNotNull(
        onAdmin?.let { SettingsItem("Open admin pages", "external-link", choose = it) },
        SettingsItem(if (syncing) "Syncing…" else "Sync now", "refresh-cw", server != null && !syncing, choose = onSync),
        onDevices?.let { SettingsItem("Devices…", "link", choose = it) },
        SettingsItem("Unpair this phone", "log-out", danger = true, choose = onUnpair),
    )
    val focus = remember(items.size) { List(items.size) { FocusRequester() } }
    val available = items.indices.filter { items[it].enabled }
    if (fromLast != null) LaunchedEffect(fromLast, items.size) {
        focus[if (fromLast) available.last() else available.first()].requestFocus()
    }
    items.forEachIndexed { index, item ->
        val ink = when { !item.enabled -> r.ink4; item.danger -> r.danger; else -> r.ink }
        val glyph = if (item.enabled && !item.danger) r.ink2 else ink
        DropdownMenuItem(
            text = { Label(item.label, Tokens.Type.BODY, ink) },
            leadingIcon = { Icon(item.icon, glyph) },
            enabled = item.enabled,
            modifier = Modifier.focusRequester(focus[index]).onPreviewKeyEvent { ev ->
                val key = ev.keyboardKey
                if (key !in listOf(Key.DirectionUp, Key.DirectionDown, Key.Escape, Key.Enter, Key.Spacebar)) false else {
                    if (ev.type == KeyEventType.KeyUp) when (key) {
                        Key.DirectionUp -> focus[available.lastOrNull { it < index } ?: available.last()].requestFocus()
                        Key.DirectionDown -> focus[available.firstOrNull { it > index } ?: available.first()].requestFocus()
                        Key.Escape -> onDismiss()
                        else -> if (item.enabled) item.choose() else Unit
                    }
                    true
                }
            },
            onClick = item.choose,
        )
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
 * Activity, a row like the smart lists', and under it, indented, today's active agents with their
 * letter marks and statuses ("2 added, 1 edited today"). Tap one to open Activity filtered to it.
 */
@Composable
private fun HistoryRows(state: UiState, current: RoomKey?, onActivity: (String?) -> Unit, onOpen: (RoomKey) -> Unit) {
    val d = LocalDesign.current
    DrawerRow(on = state.activity == "", indent = false, onClick = { onActivity(null) }) {
        Icon("clock", d.list("agent-activity").glyph)
        Label("Activity", Tokens.Type.NAV, d.room.ink, Modifier.weight(1f),
            weight = if (state.activity == "") Tokens.Type.BUTTON.weight else null)
    }
    state.home.agents.forEach { a -> AgentRow(a, state.activity == a.id) { onActivity(a.id) } }
    val trash = current == RoomKey.Trash
    val title = remember { app.todoornot.core.trashTitle() }
    DrawerRow(on = trash, indent = false, onClick = { onOpen(RoomKey.Trash) }) {
        Icon("trash-2", d.list("agent-activity").glyph)
        Label(title, Tokens.Type.NAV, d.room.ink, Modifier.weight(1f),
            weight = if (trash) Tokens.Type.BUTTON.weight else null)
    }
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
        Label(agent.mark.name, Tokens.Type.NAV_SUB, r.ink, Modifier.weight(1f),
            weight = if (on) Tokens.Type.BUTTON.weight else null)
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
    drop: Boolean = false,
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
                .background(if (drop) r.selectionWash else if (on) r.wash else r.canvas)
                .then(if (drop) Modifier.border(Tokens.Elevation.HAIRLINE.dp, r.accentStrong, RoundedCornerShape(Tokens.Radius.MD.dp)) else Modifier)
                .then(if (on) Modifier.selectionBar(r.accentStrong) else Modifier)
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
    val (probe, drop) = item.key.taskDrop()?.let { taskDragNav(it) } ?: (Modifier to false)
    DrawerRow(
        on = on,
        indent = indent,
        modifier = probe,
        drop = drop,
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
        Label(item.name, token, if (on) r.ink else r.ink2, Modifier.weight(1f),
            weight = if (on) Tokens.Type.BUTTON.weight else null)
        if (item.open > 0u) Label("${item.open}", Tokens.Type.COUNT, r.ink3)
    }
}
