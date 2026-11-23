package app.todoornot.ui

import android.content.ActivityNotFoundException
import android.content.Intent
import androidx.activity.compose.BackHandler
import androidx.compose.animation.AnimatedContent
import androidx.compose.animation.core.tween
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.togetherWith
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.isImeVisible
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.selection.selectable
import androidx.compose.foundation.selection.selectableGroup
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.snapshotFlow
import androidx.compose.ui.focus.onFocusChanged
import app.todoornot.core.filterRoomSwitcher
import app.todoornot.core.trashTitle
import app.todoornot.session.ThumbHand
import kotlinx.coroutines.flow.first
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
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
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

@Composable
internal fun BottomRoomSwitcherSheet(
    state: UiState,
    server: String?,
    actions: Actions,
    hand: ThumbHand,
    wide: Boolean,
    onHand: (ThumbHand) -> Unit,
    onDismiss: () -> Unit,
    onMenu: (NavItem) -> Unit,
    moving: Set<RoomKey>,
    onNewCollection: () -> Unit,
    initialFilter: String = "",
    initialSettings: Boolean = false,
    gestureReveal: Float? = null,
) {
    val r = LocalDesign.current.room
    var settings by rememberSaveable { mutableStateOf(initialSettings) }
    var fromLast by remember { mutableStateOf<Boolean?>(null) }
    var filter by rememberSaveable { mutableStateOf(initialFilter) }
    var namingList by rememberSaveable { mutableStateOf(false) }
    val newFocus = remember { FocusRequester() }
    val settingsFocus = remember { FocusRequester() }
    var returning by remember { mutableStateOf(false) }
    BottomSheet(if (settings) "Settings" else "Room switcher", onDismiss, tall = !settings, color = if (settings) r.raised else r.canvas,
        gestureReveal = gestureReveal) { overlay ->
        fun back() { returning = true; settings = false }
        BackHandler(enabled = settings || namingList || filter.isNotEmpty()) {
            when {
                settings -> back()
                namingList -> namingList = false
                else -> filter = ""
            }
        }
        AnimatedContent(settings, modifier = Modifier.fillMaxWidth().weight(1f, fill = false), transitionSpec = {
            fadeIn(tween(Tokens.Motion.CONTENT_SWAP)) togetherWith fadeOut(tween(Tokens.Motion.CONTENT_SWAP))
        }, label = "room switcher page") { showingSettings ->
            if (showingSettings) SettingsPage(server, state.status.state == SyncState.SYNCING, actions, hand, onHand, fromLast, ::back)
            else Column(Modifier.fillMaxWidth()) {
                val keyboard = WindowInsets.isImeVisible
                SwitcherRooms(state, filter, keyboard, actions, Modifier.weight(1f), moving,
                    onOpen = { room -> overlay.choose { actions.open(room) } },
                    onActivity = { agent -> overlay.choose { actions.openActivity(agent) } },
                    onMenu = onMenu)
                if (namingList) {
                    NewList(onCreate = { name -> overlay.choose { actions.createList(name) } },
                        onCancel = { namingList = false; newFocus.requestFocus() })
                } else RoomLookup(filter, { filter = it })
                if (state.toast != null && !keyboard) ToastStrip(state.toast, actions)
                Footer(state, actions, hand.mirrors(wide), newFocus, settingsFocus,
                    { namingList = true }, onNewCollection, { last -> fromLast = last; settings = true })
                LaunchedEffect(returning) {
                    if (returning && settingsFocus.requestFocus()) returning = false
                }
            }
        }
    }
}

@Composable
private fun SwitcherRooms(state: UiState, text: String, keyboard: Boolean, actions: Actions, modifier: Modifier,
    moving: Set<RoomKey>, onOpen: (RoomKey) -> Unit, onActivity: (String?) -> Unit, onMenu: (NavItem) -> Unit,
) {
    val r = LocalDesign.current.room
    val current = state.room.key.takeIf { state.activity == null }
    val filtering = text.isNotBlank()
    val matches = remember(state.home, text) { filterRoomSwitcher(state.home, text) }
    val grid = !filtering && !keyboard
    val drag = LocalTaskDrag.current
    DisposableEffect(drag) { onDispose { drag?.drawer = null } }
    val targets = remember(matches, filtering, grid, state.collapsed) { buildList {
        for (group in matches.home.groups) {
            group.collection?.let { add(it.key) }
            if (filtering || (group.collection?.key as? RoomKey.Collection)?.id !in state.collapsed) addAll(group.lists.map { it.key })
        }
        if (!grid) addAll(matches.home.smart.map { it.key })
        if (matches.activity || matches.home.agents.isNotEmpty() || matches.trash) add(null) // divider
        if (matches.activity) add(null)
        matches.home.agents.forEach { add(null) }
        if (matches.trash) add(RoomKey.Trash)
    } }
    val currentIndex = targets.indexOf(current).takeIf { current != null } ?: -1
    val initial = when {
        filtering -> 0
        currentIndex >= 0 -> (currentIndex - 3).coerceAtLeast(0)
        else -> targets.lastIndex.coerceAtLeast(0)
    }
    val list = rememberLazyListState(initialFirstVisibleItemIndex = initial)
    LaunchedEffect(current, filtering) {
        if (filtering) list.scrollToItem(0)
        else if (currentIndex >= 0) {
            val measured = snapshotFlow { list.layoutInfo }.first { it.visibleItemsInfo.isNotEmpty() }
            val above = measured.visibleItemsInfo.size * 2 / 3
            list.scrollToItem((currentIndex - above).coerceAtLeast(0))
        } else if (targets.isNotEmpty()) list.scrollToItem(targets.lastIndex)
    }
    Column(modifier) {
        LazyColumn(Modifier.weight(1f).fillMaxWidth().padding(horizontal = S.S3.dp)
            .onGloballyPositioned { coords ->
                drag?.drawer = DragInput.ScrollPane(coords.boundsInRoot()) { list.dispatchRawDelta(it) }
            }, state = list, verticalArrangement = Arrangement.Bottom) {
            matches.home.groups.forEach { g ->
                val id = (g.collection?.key as? RoomKey.Collection)?.id
                val folded = !filtering && id in state.collapsed
                g.collection?.let { room -> item(room.key.toString()) {
                    Row(Modifier.slideOnMove(room.key in moving), verticalAlignment = Alignment.CenterVertically) {
                        Box(Modifier.weight(1f)) { NavRow(room, room.key == current, false, onOpen, heading = true, onLongPress = { onMenu(room) }) }
                        if (!filtering && id != null) IconButton(if (folded) "chevron-right" else "chevron-down",
                            if (folded) "Show ${room.name}" else "Hide ${room.name}", tint = r.ink3, expanded = !folded, highlight = false) {
                            actions.toggleCollection(id)
                        }
                    }
                } }
                if (!folded) for (room in g.lists) item(room.key.toString()) {
                    Box(Modifier.slideOnMove(room.key in moving)) {
                        NavRow(room, room.key == current, id != null, onOpen, onLongPress = { onMenu(room) })
                    }
                }
            }
            if (!grid) for (room in matches.home.smart) item(room.key.toString()) {
                NavRow(room, room.key == current, false, onOpen)
            }
            if (matches.activity || matches.home.agents.isNotEmpty() || matches.trash) item("history divider") {
                Spacer(Modifier.fillMaxWidth().padding(vertical = S.S1.dp).height(Tokens.Elevation.HAIRLINE.dp).background(r.line))
            }
            if (matches.activity) item("activity") { ActivityNav(state.activity == "", onActivity) }
            for (agent in matches.home.agents) item("agent:${agent.id}") { AgentRow(agent, state.activity == agent.id) { onActivity(agent.id) } }
            if (matches.trash) item("trash") { TrashNav(current == RoomKey.Trash, onOpen) }
            if (targets.isEmpty()) item("no matches") {
                Label("No rooms match.", Tokens.Type.BODY, r.ink2, Modifier.padding(S.S3.dp))
            }
        }
        if (grid) Column(Modifier.fillMaxWidth().hairline(r.line, top = true).padding(S.S3.dp), verticalArrangement = Arrangement.spacedBy(S.S2.dp)) {
            matches.home.smart.chunked(3).forEach { rooms ->
                Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(S.S2.dp)) {
                    for (room in rooms) SmartTile(room, room.key == current, Modifier.weight(1f)) { onOpen(room.key) }
                    repeat(3 - rooms.size) { Spacer(Modifier.weight(1f)) }
                }
            }
        }
    }
}

@Composable
private fun SmartTile(item: NavItem, active: Boolean, modifier: Modifier, open: () -> Unit) {
    val d = LocalDesign.current
    val r = d.room
    val shape = RoundedCornerShape(Tokens.Radius.MD.dp)
    val (probe, drop) = item.key.taskDrop()?.let { taskDragNav(it) } ?: (Modifier to false)
    val label = when ((item.key as? RoomKey.Smart)?.list) {
        SmartList.COMPLETED -> "Done"
        SmartList.AGENT_ACTIVITY -> "Agents"
        else -> item.name
    }
    Row(modifier.then(probe).heightIn(min = P.SMART_TILE.dp).clip(shape)
        .background(if (drop) r.selectionWash else if (active) r.wash else r.raised)
        .border(Tokens.Elevation.HAIRLINE.dp, if (active || drop) r.accentStrong else r.line, shape)
        .then(if (active) Modifier.selectionBar(r.accentStrong) else Modifier)
        .selectable(active, role = Role.Tab, onClick = open)
        .semantics { contentDescription = item.name }
        .padding(horizontal = S.S2.dp), verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(S.S1.dp)) {
        item.icon?.let { Icon(it, d.list(item.hue).glyph, size = Tokens.Metrics.Component.GLYPH.dp) }
        Label(label, Tokens.Type.NAV_SUB, if (active) r.ink else r.ink2, Modifier.weight(1f), weight = if (active) Tokens.Type.BUTTON.weight else null)
        if (item.open > 0u && label != "Done") Label("${item.open}", Tokens.Type.COUNT, r.ink3)
    }
}

@Composable
private fun RoomLookup(text: String, onChange: (String) -> Unit) {
    val r = LocalDesign.current.room
    var focused by remember { mutableStateOf(false) }
    val keyboard = androidx.compose.ui.platform.LocalSoftwareKeyboardController.current
    val shape = RoundedCornerShape(Tokens.Radius.MD.dp)
    Row(Modifier.fillMaxWidth().padding(start = S.S3.dp, end = S.S3.dp, bottom = S.S2.dp)
        .heightIn(min = P.TOUCH.dp).border(Tokens.Elevation.HAIRLINE.dp, if (focused) r.accentStrong else r.control, shape)
        .background(r.raised, shape).padding(start = S.S3.dp), verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(S.S2.dp)) {
        Icon("search", r.ink3, size = Tokens.Metrics.Component.GLYPH.dp)
        BasicTextField(text, { onChange(it.replace("\n", "")) },
            Modifier.weight(1f).onFocusChanged { focused = it.isFocused }.semantics { contentDescription = "Find a room" },
            singleLine = true, textStyle = Tokens.Type.NAV_SUB.style(r.ink), cursorBrush = SolidColor(r.accent),
            keyboardOptions = KeyboardOptions(imeAction = ImeAction.Search, autoCorrectEnabled = false),
            keyboardActions = KeyboardActions(onSearch = { keyboard?.hide() }),
            decorationBox = { inner -> Box(Modifier.heightIn(min = P.TOUCH.dp), contentAlignment = Alignment.CenterStart) {
                if (text.isEmpty()) Label("Find a room…", Tokens.Type.NAV_SUB, r.ink3)
                inner()
            } })
        if (text.isNotEmpty()) IconButton("x", "Clear room search") { onChange("") }
    }
}

/** The inline list name editor. Done adds a non-empty name and opens the list. */
@Composable
private fun NewList(onCreate: (String) -> Unit, onCancel: () -> Unit) {
    val r = LocalDesign.current.room
    var name by rememberSaveable { mutableStateOf("") }
    val focus = remember { FocusRequester() }
    val keyboard = androidx.compose.ui.platform.LocalSoftwareKeyboardController.current
    LaunchedEffect(Unit) { if (focus.requestFocus()) keyboard?.show() }
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
                Box(Modifier.heightIn(min = P.TOUCH.dp), contentAlignment = Alignment.CenterStart) {
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
    var pending by remember { mutableStateOf<(() -> Unit)?>(null) }
    val list = remember { FocusRequester() }
    val collection = remember { FocusRequester() }
    fun dismiss() {
        open = false
        val choose = pending
        pending = null
        if (choose == null) trigger.requestFocus() else choose()
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
        if (open) AnchoredPopup(::dismiss, start = true, above = true) { overlay ->
            Column(Modifier.padding(S.S2.dp).width(Tokens.Metrics.Component.NEW_MENU.dp)
                .overlay(RoundedCornerShape(Tokens.Radius.LG.dp), LocalDesign.current)) {
            LaunchedEffect(Unit) { list.requestFocus() }
            @Composable
            fun Item(label: String, icon: String, focus: FocusRequester, other: FocusRequester, choose: () -> Unit) {
                val select = {
                    if (overlay.open) {
                        pending = choose
                        overlay.close()
                    }
                }
                DropdownMenuItem(
                    text = { Label(label, Tokens.Type.NAV_SUB, r.ink) },
                    leadingIcon = { Icon(icon, r.ink2) },
                    modifier = Modifier.focusRequester(focus).onPreviewKeyEvent { ev ->
                        val key = ev.keyboardKey
                        if (key !in listOf(Key.DirectionUp, Key.DirectionDown, Key.Escape, Key.Enter, Key.Spacebar)) {
                            false
                        } else {
                            if (ev.type == KeyEventType.KeyUp) when (key) {
                                Key.Escape -> overlay.close()
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
}

/** The single pinned row: New, the sync line, and the settings menu. */
@Composable
private fun Footer(
    state: UiState,
    actions: Actions,
    left: Boolean,
    newFocus: FocusRequester,
    settingsFocus: FocusRequester,
    onNewList: () -> Unit,
    onNewCollection: () -> Unit,
    onSettings: (Boolean?) -> Unit,
) {
    val r = LocalDesign.current.room
    val line = syncLine(state.status, state.syncedAt)
    Row(
        Modifier.fillMaxWidth().heightIn(min = P.BOTTOM_BAR.dp).hairline(r.line, top = true).padding(horizontal = S.S2.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(S.S2.dp),
    ) {
        val color = when (line.tone) {
            SyncTone.OK -> r.success
            SyncTone.BUSY -> r.accent
            SyncTone.PROBLEM -> r.danger
            SyncTone.IDLE -> r.ink4
        }
        if (left) SettingsButton(settingsFocus, onSettings) else NewMenu(newFocus, onNewList, onNewCollection)
        Row(
            Modifier.weight(1f).heightIn(min = P.TOUCH.dp).clip(RoundedCornerShape(Tokens.Radius.MD.dp))
                .clickable(role = Role.Button, onClickLabel = "Sync now", enabled = state.status.state != SyncState.SYNCING, onClick = actions::nudge)
                .semantics { contentDescription = line.full },
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(S.S2.dp, Alignment.CenterHorizontally),
        ) {
            Box(Modifier.size(Tokens.Metrics.Component.CHIP_DOT.dp).background(color, CircleShape))
            Label(line.short, Tokens.Type.COUNT, r.ink3)
        }
        if (left) NewMenu(newFocus, onNewList, onNewCollection) else SettingsButton(settingsFocus, onSettings)
    }
}

@Composable
internal fun SettingsButton(trigger: FocusRequester, open: (Boolean?) -> Unit) {
    IconButton("settings", "Settings", modifier = Modifier.focusRequester(trigger).onPreviewKeyEvent { ev ->
        val key = ev.keyboardKey
        if (key !in listOf(Key.Enter, Key.Spacebar, Key.DirectionUp, Key.DirectionDown)) false else {
            if (ev.type == KeyEventType.KeyUp) open(key == Key.DirectionUp)
            true
        }
    }) { open(null) }
}

@Composable
internal fun SettingsPage(server: String?, syncing: Boolean, actions: Actions, hand: ThumbHand, onHand: (ThumbHand) -> Unit,
    fromLast: Boolean?, onBack: () -> Unit,
) {
    val r = LocalDesign.current.room
    val context = LocalContext.current
    val admin = remember(server) { server?.let { adminUrl(it, Cleartext.allowLoopbackHttp) } }
    // Keep the existing secure device pairing dialog in this phase.
    var pairing by rememberSaveable { mutableStateOf<DevicePairingPage?>(null) }
    val owner = LocalPairingOwner.current
    if (owner != null) pairing?.let { page ->
        DevicePairingDialog(owner, page, onPage = { pairing = it }, onClose = { pairing = null; onBack() })
    }
    Column(Modifier.fillMaxWidth().verticalScroll(rememberScrollState()).padding(bottom = S.S2.dp)) {
        Row(verticalAlignment = Alignment.CenterVertically) {
            IconButton("chevron-left", "Back to rooms", onClick = onBack)
            Label("Settings", Tokens.Type.ROOM_TITLE_BAR, r.ink)
        }
        SettingsMenuContent(
            server, syncing, fromLast, onDismiss = onBack,
            onAdmin = admin?.let { url ->
                {
                    onBack()
                    val intent = Intent(Intent.ACTION_VIEW, Uri.parse(url)).addCategory(Intent.CATEGORY_BROWSABLE)
                    try {
                        context.startActivity(intent)
                    } catch (_: ActivityNotFoundException) {
                        Toast.makeText(context, "No browser can open the admin pages.", Toast.LENGTH_SHORT).show()
                    } catch (_: SecurityException) {
                        Toast.makeText(context, "This phone won't open the admin pages.", Toast.LENGTH_SHORT).show()
                    }
                }
            },
            onSync = { if (!syncing) { actions.nudge(); onBack() } },
            onDevices = owner?.let { { pairing = DevicePairingPage.SETTINGS } },
            onUnpair = { actions.unpair() },
            hand = hand, onHand = onHand,
        )
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
    hand: ThumbHand? = null, onHand: (ThumbHand) -> Unit = {},
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
        if (item.danger) Spacer(Modifier.fillMaxWidth().height(Tokens.Elevation.HAIRLINE.dp).background(r.line))
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
        if (hand != null && item.icon == "refresh-cw") HandSelector(hand, onHand)
    }
}

@Composable
private fun HandSelector(hand: ThumbHand, onHand: (ThumbHand) -> Unit) {
    val r = LocalDesign.current.room
    val shape = RoundedCornerShape(Tokens.Radius.MD.dp)
    Column(Modifier.fillMaxWidth().padding(horizontal = S.S4.dp, vertical = S.S2.dp), verticalArrangement = Arrangement.spacedBy(S.S2.dp)) {
        Label("Thumb-zone hand", Tokens.Type.NAV_SUB, r.ink)
        Row(Modifier.selectableGroup(), horizontalArrangement = Arrangement.spacedBy(S.S2.dp)) {
            for (choice in ThumbHand.entries) {
                val on = choice == hand
                Row(Modifier.weight(1f).heightIn(min = P.TOUCH.dp).clip(shape)
                    .background(if (on) r.wash else r.raised)
                    .border(Tokens.Elevation.HAIRLINE.dp, if (on) r.accentStrong else r.control, shape)
                    .selectable(on, role = Role.RadioButton) { onHand(choice) }.padding(horizontal = S.S2.dp),
                    verticalAlignment = Alignment.CenterVertically,
                    horizontalArrangement = Arrangement.spacedBy(S.S2.dp, Alignment.CenterHorizontally)) {
                    if (on) Icon("check", r.accentStrong, size = Tokens.Metrics.Component.ICON_SMALL.dp)
                    Label(choice.label, Tokens.Type.NAV_SUB, if (on) r.accentStrong else r.ink)
                }
            }
        }
    }
}

@Composable
private fun ActivityNav(active: Boolean, onActivity: (String?) -> Unit) {
    val d = LocalDesign.current
    DrawerRow(on = active, indent = false, onClick = { onActivity(null) }) {
        Icon("clock", d.list("agent-activity").glyph)
        Label("Activity", Tokens.Type.NAV, d.room.ink, Modifier.weight(1f), weight = if (active) Tokens.Type.BUTTON.weight else null)
    }
}

@Composable
private fun TrashNav(active: Boolean, onOpen: (RoomKey) -> Unit) {
    val d = LocalDesign.current
    val title = remember { trashTitle() }
    DrawerRow(on = active, indent = false, onClick = { onOpen(RoomKey.Trash) }) {
        Icon("trash-2", d.list("agent-activity").glyph)
        Label(title, Tokens.Type.NAV, d.room.ink, Modifier.weight(1f), weight = if (active) Tokens.Type.BUTTON.weight else null)
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
