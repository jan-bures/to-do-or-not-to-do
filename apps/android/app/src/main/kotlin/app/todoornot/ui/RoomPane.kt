package app.todoornot.ui

import androidx.compose.animation.AnimatedContent
import androidx.compose.animation.togetherWith
import androidx.compose.animation.expandVertically
import androidx.compose.animation.shrinkVertically
import androidx.compose.foundation.gestures.detectVerticalDragGestures
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.SideEffect
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.saveable.rememberSaveableStateHolder
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.platform.LocalSoftwareKeyboardController
import app.todoornot.session.ThumbHand
import kotlinx.coroutines.launch
import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.core.animate
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.asPaddingValues
import androidx.compose.foundation.layout.ime
import androidx.compose.foundation.layout.navigationBars
import androidx.compose.foundation.layout.statusBars
import androidx.compose.foundation.layout.isImeVisible
import androidx.compose.ui.input.nestedscroll.nestedScroll
import androidx.window.core.layout.WindowSizeClass
import androidx.compose.animation.core.tween
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.slideInVertically
import androidx.compose.animation.slideOutVertically
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.layout.RowScope
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.ui.layout.boundsInRoot
import androidx.compose.ui.layout.onGloballyPositioned
import androidx.compose.foundation.lazy.LazyListScope
import androidx.compose.foundation.lazy.LazyItemScope
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.itemsIndexed
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.derivedStateOf
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.focus.onFocusChanged
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.selected
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.semantics.stateDescription
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import app.todoornot.core.RoomKey
import app.todoornot.core.RoomView
import app.todoornot.core.SmartList
import app.todoornot.core.SyncState
import app.todoornot.core.SyncStatus
import app.todoornot.core.TaskRow
import app.todoornot.core.Tomorrow
import app.todoornot.design.Icon
import app.todoornot.design.LocalDesign
import app.todoornot.design.Tokens
import app.todoornot.design.cubic
import app.todoornot.design.style
import kotlinx.coroutines.delay
import app.todoornot.model.Actions
import app.todoornot.model.roomMenu
import app.todoornot.model.DuskLook
import app.todoornot.model.Ghost
import app.todoornot.model.MotionFrame
import app.todoornot.model.RowLook
import app.todoornot.model.Section
import app.todoornot.model.Shown
import app.todoornot.model.countdown
import app.todoornot.model.withGhosts
import app.todoornot.model.rowIn
import app.todoornot.model.Toast
import app.todoornot.model.UiState

private val P = Tokens.Metrics.Phone
private val S = Tokens.Space
private val M = Tokens.Motion

/** Rows that stay visible above the composer while typing (`docs/design-language.md`). */
private const val MIN_ROWS_TYPING = 4

/**
 * The sky and task list sit above all room controls. The dock swaps its input or takes over for bulk work.
 */
@Composable
fun RoomPane(
    state: UiState,
    actions: Actions,
    landscape: Dp,
    onMenu: () -> Unit,
    modifier: Modifier = Modifier,
    draft: String = "",
    collapsed: Boolean = false,
    onRoomMenu: () -> Unit = {},
    onBulkPicker: (BulkPicker) -> Unit = {},
    composerFocus: (() -> Unit)? = null,
    wide: Boolean = false,
    hand: ThumbHand = ThumbHand.RIGHT,
    sheetOpen: Boolean = false,
    reachExpanded: Boolean = false,
    onSwipeRooms: (Float?) -> Unit = {},
) {
    val r = LocalDesign.current.room
    val room = state.room
    val keyboard = WindowInsets.isImeVisible
    BoxWithConstraints(modifier.fillMaxHeight().background(r.pane)) {
        val short = maxHeight < WindowSizeClass.HEIGHT_DP_MEDIUM_LOWER_BOUND.dp
        // While typing, the bar goes too if the rows left under it would be fewer than four. Worked out
        // from the window and the keyboard, not from the list, so hiding the bar can't flip it back.
        val keyboardHeight = WindowInsets.ime.asPaddingValues().calculateBottomPadding()
        val statusBar = WindowInsets.statusBars.asPaddingValues().calculateTopPadding()
        val nav = WindowInsets.navigationBars.asPaddingValues().calculateBottomPadding()
        val underBar = maxHeight - maxOf(keyboardHeight, nav) - statusBar - P.COMPACT_HEADER.dp - P.COMPOSER.dp - S.S2.dp
        val bar = !keyboard || underBar >= P.ROW.dp * MIN_ROWS_TYPING
        val force = if (collapsed || keyboard || short) 1f else 0f
        val density = LocalDensity.current
        val reachLimit = with(density) { minOf(P.HEADER_REACH_EXPAND.dp, maxHeight * P.HEADER_REACH_FRACTION).toPx() }
        val collapse = remember(state.generation) { HeaderCollapse(if (reachExpanded) reachLimit else 0f).also { it.forced = force } }
        val scope = rememberCoroutineScope()
        val reduced = reducedMotion()
        SideEffect {
            collapse.reachLimit = if (!wide && force == 0f && !sheetOpen) reachLimit else 0f
            collapse.reachLatch = with(density) { P.HEADER_REACH_LATCH.dp.toPx() }
            collapse.reducedMotion = reduced
        }
        LaunchedEffect(collapse, force) {
            animate(collapse.forced, force, animationSpec = tween(if (reduced) 0 else M.PANEL, easing = M.EASE_STANDARD.cubic())) { v, _ ->
                collapse.forced = v
            }
        }
        LaunchedEffect(collapse, sheetOpen, state.selected, force, wide, state.bulkIds) {
            if (sheetOpen || state.selected != null || force > 0f || wide || state.bulkIds.isNotEmpty()) collapse.resetReach()
        }
        val drag = LocalTaskDrag.current
        val lifted = drag?.picked != null || LocalTaskDragFrame.current != null
        val left = hand.mirrors(wide)
        val holders = rememberSaveableStateHolder()
        var addingWithFilter by remember(state.generation, state.searching) { mutableStateOf(false) }
        var focusAdd by remember { mutableStateOf(false) }
        val filterShown = state.searching && !addingWithFilter
        Column(Modifier.fillMaxSize().navigationBarsPadding().imePadding()) {
            val resetReach = { if (collapse.reach > 0f) scope.launch { collapse.resetReach() }; Unit }
            RoomHeader(room, landscape, collapse, state.status, compact = if (bar) P.COMPACT_HEADER.dp else 0.dp, onResetReach = resetReach)
            Box(Modifier.weight(1f).fillMaxWidth().hairline(r.line, top = true).nestedScroll(collapse.connection)) {
                androidx.compose.runtime.key(state.generation) { TaskList(state, actions, resetReach) }
            }
            state.failed.firstOrNull()?.let { f ->
                Banner("Not saved: ${f.what}. ${f.reason}", "Dismiss") { actions.dismissFailed(f.id) }
            }
            if (state.signedOut) Banner("This phone isn't paired any more.", "Pair again", actions::unpair)
            ToastStrip(state.toast, actions)
            if (state.bulk != null) {
                BulkStatusStrip(state.bulk.label, left, actions::clearBulk)
                if (lifted) BottomRoomBar(state, actions, left, onMenu, onRoomMenu, draggingTask = true)
                else BulkSelectionTools(state, onBulkPicker, actions::editBulk)
            } else {
                AnimatedContent(filterShown, transitionSpec = {
                    fadeIn(tween(M.CONTENT_SWAP)) togetherWith fadeOut(tween(M.CONTENT_SWAP))
                }, label = "dock input") { filtering ->
                    if (filtering) FilterField(state.filter, actions::setFilter,
                        onClose = { actions.setSearching(false) },
                        onAdd = if (state.filter.isNotBlank() && !keyboard) {{ addingWithFilter = true; focusAdd = true }} else null,
                    ) else holders.SaveableStateProvider("composer") {
                        Composer(room.addPlaceholder, draft, keyboard, actions,
                            composerFocus ?: if (focusAdd) {{ focusAdd = false }} else null, docked = true)
                    }
                }
                AnimatedVisibility(!keyboard, enter = expandVertically(tween(M.PANEL)), exit = shrinkVertically(tween(M.PANEL, easing = M.EASE_EXIT.cubic()))) {
                    BottomRoomBar(state, actions, left, onMenu, onRoomMenu, onSwipeRooms = onSwipeRooms,
                        onFilter = { if (addingWithFilter) addingWithFilter = false else actions.setSearching(!state.searching) })
                }
            }
        }
    }
}

@Composable
private fun BottomRoomBar(
    state: UiState,
    actions: Actions,
    left: Boolean,
    onRooms: () -> Unit,
    onMenu: () -> Unit,
    draggingTask: Boolean = false,
    onFilter: () -> Unit = {},
    onSwipeRooms: (Float?) -> Unit = {},
) {
    val r = LocalDesign.current.room
    val drag = LocalTaskDrag.current
    val hasMenu = roomMenu(state.home, state.room.key, state.showCompleted).isNotEmpty()
    val dot = when {
        state.status.problem != null -> r.danger
        state.status.state == SyncState.OFFLINE -> r.ink4
        state.status.pending > 0u -> r.accent
        else -> null
    }
    val roomsAction by rememberUpdatedState(onRooms)
    val swipeAction by rememberUpdatedState(onSwipeRooms)
    Row(Modifier.fillMaxWidth().height(P.BOTTOM_BAR.dp).background(r.canvas).hairline(r.line, top = true)
        .padding(horizontal = S.S1.dp).then(if (!draggingTask) Modifier.pointerInput(Unit) {
            var up = 0f
            var revealed = false
            detectVerticalDragGestures(onDragStart = { up = 0f; revealed = false }, onDragCancel = { swipeAction(null) }, onDragEnd = {
                if (up >= P.BULK_STATUS_BAR.dp.toPx()) roomsAction()
                swipeAction(null)
            }) { change, dy ->
                change.consume()
                up = (up - dy).coerceAtLeast(0f)
                if (up > 0f) revealed = true
                if (revealed) swipeAction(up)
            }
        } else Modifier), verticalAlignment = Alignment.CenterVertically) {
        val rooms: @Composable () -> Unit = {
            DisposableEffect(drag) { onDispose { drag?.roomsButton = null } }
            Box(Modifier.onGloballyPositioned { drag?.roomsButton = it.boundsInRoot() }) {
                IconButton("menu", "Rooms", onClick = onRooms)
                dot?.let { Box(Modifier.align(Alignment.TopEnd).padding(top = S.S2.dp, end = S.S2.dp)
                    .size(Tokens.Metrics.Component.CHIP_DOT.dp).background(it, CircleShape)) }
            }
        }
        val search: @Composable () -> Unit = { if (!draggingTask) IconButton("search", "Filter", expanded = state.searching, onClick = onFilter) }
        val overflow: @Composable () -> Unit = { if (!draggingTask && hasMenu) IconButton("ellipsis", "Room menu", onClick = onMenu) }
        if (left) { overflow(); search() } else rooms()
        RoomPillsStrip(state.home, state.room.key, Modifier.weight(1f).padding(horizontal = S.S1.dp), actions::open)
        if (left) rooms() else { search(); overflow() }
    }
}

/**
 * The filter, above the bottom utility bar: the same query language as the desktop's (`tag:docs`, `due<=friday`,
 * `priority>=high`), or plain words to find in titles. The rows narrow as you type; the progress doesn't.
 */
@Composable
private fun FilterField(text: String, onChange: (String) -> Unit, onClose: () -> Unit, onAdd: (() -> Unit)?) {
    val r = LocalDesign.current.room
    val focus = remember { FocusRequester() }
    var focused by remember { mutableStateOf(false) }
    val keyboard = LocalSoftwareKeyboardController.current
    LaunchedEffect(Unit) { if (text.isEmpty() && focus.requestFocus()) keyboard?.show() }
    val shape = RoundedCornerShape(Tokens.Radius.MD.dp)
    Row(Modifier.fillMaxWidth().background(r.pane).hairline(r.line, top = true).padding(horizontal = S.S3.dp, vertical = S.S1.dp),
        verticalAlignment = Alignment.CenterVertically) {
        onAdd?.let { IconButton("plus", "Add a task without clearing the filter", onClick = it) }
        Row(
            Modifier
                .weight(1f)
                .heightIn(min = P.TOUCH.dp)
                .clip(shape)
                .background(r.wash)
                .border(Tokens.Elevation.HAIRLINE.dp, if (focused) r.accentStrong else r.control, shape)
                .padding(start = S.S3.dp),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(S.S2.dp),
        ) {
            Icon("search", if (text.isEmpty()) r.ink3 else r.accentStrong, size = Tokens.Metrics.Component.ICON_SMALL.dp)
            BasicTextField(
                value = text,
                onValueChange = { onChange(it.replace("\n", "")) },
                modifier = Modifier.weight(1f).focusRequester(focus).onFocusChanged { focused = it.isFocused }
                    .semantics { contentDescription = "Filter" },
                textStyle = Tokens.Type.SUMMARY.style(r.ink),
                singleLine = true,
                cursorBrush = SolidColor(r.accent),
                keyboardOptions = KeyboardOptions(imeAction = ImeAction.Search, autoCorrectEnabled = false),
                keyboardActions = KeyboardActions(onSearch = { keyboard?.hide() }),
                decorationBox = { inner ->
                    Box(Modifier.heightIn(min = P.TOUCH.dp), contentAlignment = Alignment.CenterStart) {
                        if (text.isEmpty()) Label("Filter: words, or tag:docs", Tokens.Type.SUMMARY, r.ink3)
                        inner()
                    }
                },
            )
            IconButton("x", "Clear the filter", onClick = onClose)
        }
    }
}

/** 48 dp touch target around an 18 dp icon. */
@Composable
fun IconButton(icon: String, label: String, tint: androidx.compose.ui.graphics.Color? = null, enabled: Boolean = true,
    expanded: Boolean? = null, modifier: Modifier = Modifier, highlight: Boolean = expanded == true, onClick: () -> Unit) {
    val r = LocalDesign.current.room
    Box(
        modifier.size(P.TOUCH.dp).clip(CircleShape)
            .then(if (highlight) Modifier.background(r.wash) else Modifier)
            .clickable(enabled = enabled, onClick = onClick)
            .semantics {
                contentDescription = label
                if (expanded != null) stateDescription = if (expanded) "Expanded" else "Collapsed"
            },
        contentAlignment = Alignment.Center,
    ) {
        Icon(icon, if (enabled) tint ?: if (highlight) r.accentStrong else r.ink2 else r.ink4)
    }
}

/** Shown only when there's something to say: offline, syncing changes, or a problem. */
@Composable
internal fun SyncChip(status: SyncStatus, onClick: () -> Unit) {
    val r = LocalDesign.current.room
    val (icon, text) = when {
        status.state == SyncState.OFFLINE || status.problem != null ->
            "cloud-off" to if (status.pending > 0u) "${status.pending}" else "Offline"
        status.state == SyncState.SYNCING && status.pending > 0u -> "refresh-cw" to "${status.pending}"
        else -> return
    }
    val shape = RoundedCornerShape(Tokens.Radius.MD.dp)
    Row(
        Modifier
            .padding(end = S.S1.dp)
            .height(Tokens.Metrics.Component.BUTTON_SMALL.dp)
            .clip(shape)
            .background(r.raised)
            .border(Tokens.Elevation.HAIRLINE.dp, r.lineStrong, shape)
            .clickable(onClick = onClick)
            .semantics { contentDescription = "Sync: ${status.state.name.lowercase()}, ${status.pending} waiting" }
            .padding(horizontal = S.S2.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(S.S2.dp),
    ) {
        Icon(icon, r.ink3, size = Tokens.Metrics.Component.ICON_SMALL.dp)
        Label(text, Tokens.Type.SUMMARY_FIGURES, r.ink2)
    }
}

@Composable
internal fun Banner(text: String, action: String, onAction: () -> Unit) {
    val r = LocalDesign.current.room
    Row(
        Modifier.fillMaxWidth().background(r.dangerBg).hairline(r.line, top = true).padding(start = S.S5.dp, end = S.S2.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Label(text, Tokens.Type.META_PHONE, r.dangerInk, Modifier.weight(1f), maxLines = 2)
        Box(
            Modifier.height(P.TOUCH.dp).clickable(onClick = onAction).padding(horizontal = S.S3.dp),
            contentAlignment = Alignment.Center,
        ) {
            Label(action, Tokens.Type.BUTTON, r.dangerInk)
        }
    }
}

/**
 * The tasks. Checking a box plays the choreography in place (`docs/design-language.md`, Motion): the
 * row fills, holds and collapses while the rows below close up, and only then leaves for Completed. As
 * dusk falls on a finished day, tomorrow's hand-over enters item by item, and takes taps only once it
 * has begun to appear. A row the room no longer lists while its choreography plays is painted where it
 * last was, inert, until that's over (a ghost: [withGhosts]). Every tap on a row carries the room
 * [generation] the rows were read in.
 */
@Composable
private fun TaskList(state: UiState, actions: Actions, onTouchTask: () -> Unit) {
    val room = state.room
    val showDone = state.showCompleted
    val r = LocalDesign.current.room
    val motion = LocalMotion.current
    val dusk: () -> DuskLook = remember(room.key, room.finished, motion) {
        { motion.dusk(room.key) ?: DuskLook.rest(room.finished) }
    }
    val ghosts by remember(motion) { derivedStateOf { motion.ghosts() } }
    var swapping by remember(state.reorder) { mutableStateOf(state.reorder?.rows.orEmpty()) }
    LaunchedEffect(state.reorder) {
        if (swapping.isNotEmpty()) {
            delay(M.ROW_ENTER.toLong())
            swapping = emptySet()
        }
    }
    val rows = withGhosts(room.tasks, ghosts, Section.ROWS)
    val list = rememberLazyListState()
    val drag = LocalTaskDrag.current
    LazyColumn(Modifier.fillMaxSize().onGloballyPositioned { coords ->
        drag?.list = DragInput.ScrollPane(coords.boundsInRoot()) { list.dispatchRawDelta(it) }
    }, state = list) {
        // Only when nothing at all is listed: a filter that matches only done tasks shows them under
        // Completed, without a "nothing matches" line above them. Nor while a ghost plays out.
        if (showsEmptyLine(room, ghosts)) {
            item("empty") { Empty(room.emptyLine) }
        }
        // The rows; the Completed and Agent activity smart lists show their done tasks here too.
        items(rows, key = { it.row.id }) { s -> MovingRow(s, Section.ROWS, state, actions, motion, onTouchTask, swap = s.row.id in swapping) }
        room.tomorrow?.let { tomorrow(it, state, actions, dusk) }
        // Finished with nothing due tomorrow: no hand-over, but planning the next day is still offered.
        if (room.tomorrow == null) room.planWeekday?.let { weekday -> planOnly(weekday, actions, dusk) }
        // The finished day keeps today's Completed section under the hand-over, as on the desktop.
        // A ghost there keeps the open section while it plays out (it isn't counted).
        if (room.completed.isNotEmpty() || (showDone && ghosts.any { it.section == Section.COMPLETED })) {
            item("completed-header") {
                Row(
                    Modifier
                        .fillMaxWidth()
                        .height(P.TOUCH.dp)
                        .clickable { actions.setShowCompleted(!showDone) }
                        .padding(horizontal = P.ROW_PAD_LEFT.dp),
                    verticalAlignment = Alignment.CenterVertically,
                    horizontalArrangement = Arrangement.spacedBy(S.S2.dp),
                ) {
                    Icon(if (showDone) "chevron-down" else "chevron-right", r.ink3, size = Tokens.Metrics.Component.ICON_SMALL.dp)
                    Label("Completed", Tokens.Type.CAPS_LABEL, r.ink3)
                    Label("${room.completed.size}", Tokens.Type.COUNT, r.ink3)
                }
            }
            if (showDone) {
                items(withGhosts(room.completed, ghosts, Section.COMPLETED), key = { "done-" + it.row.id }) { s ->
                    MovingRow(s, Section.COMPLETED, state, actions, motion, onTouchTask)
                }
            }
        }
    }
}

/**
 * A task row in [section] of the list, with its choreography: only a row that moves recomposes as the
 * frames go by. A row at rest that the list just gained (a new task, another room) fades in; one arriving
 * from its choreography enters as that says. A ghost is paint only: it takes no taps.
 */
@Composable
private fun LazyItemScope.MovingRow(
    s: Shown,
    section: Section,
    state: UiState,
    actions: Actions,
    motion: MotionFrame,
    onTouchTask: () -> Unit,
    swap: Boolean = false,
) {
    val t = s.row
    val look by remember(t.id, section, motion) { derivedStateOf { motion.rowIn(t.id, section) } }
    val checked by remember(t.id, motion) { derivedStateOf { motion.checked(t.id) } }
    // A ghost whose choreography is over is gone, whatever the list has yet to hear.
    val l = look ?: if (s.ghost) RowLook.hidden(t.done) else null
    val done = checked ?: t.done
    val selecting = state.bulk != null
    val manual = state.room.key is RoomKey.List
    val dragHandle: (@Composable () -> Unit)? = if (t.id in state.bulkIds && !s.ghost && l?.gone != true) {
        { DragGrip(t, manual) }
    } else null
    val dragRow = taskDragRow(t.id, manual && section == Section.ROWS && !done && !s.ghost)
    val toggle = { if (!s.ghost && l?.gone != true) { onTouchTask(); actions.toggleBulk(t.id, state.generation) } }
    TaskRowView(t, selected = !s.ghost && (t.id == state.selected || t.id in state.bulkIds), checked = done,
        onCheck = { if (!s.ghost) { onTouchTask(); actions.setDone(t.id, it, state.generation) } },
        onImportant = { if (!s.ghost) { onTouchTask(); actions.setImportant(t.id, it) } },
        onOpen = { if (selecting) toggle() else if (!s.ghost) { onTouchTask(); actions.select(t.id) } },
        onActions = toggle, look = l, inert = s.ghost, selecting = selecting, dragHandle = dragHandle,
        modifier = dragRow.animateItem(
            fadeInSpec = if (l == null) tween(M.ROW_ENTER, easing = M.EASE_ENTER.cubic()) else null,
            placementSpec = if (swap) tween(M.ROW_ENTER, easing = M.EASE_STANDARD.cubic()) else null,
            fadeOutSpec = null,
        ),
    )
}

@Composable
private fun Empty(line: String) {
    val r = LocalDesign.current.room
    Column(Modifier.fillMaxWidth().padding(horizontal = S.S5.dp, vertical = S.S8.dp)) {
        Label(line, Tokens.Type.EMPTY_LINE, r.ink2, maxLines = 3)
    }
}

/** Whether item [i] of the hand-over has begun to appear: only then does it take taps. */
@Composable
private fun shown(i: Int, dusk: () -> DuskLook): Boolean {
    val on by remember(i, dusk) { derivedStateOf { dusk().handoverShown(i) } }
    return on
}

/**
 * The finished day hands over to tomorrow: the first thing, the rest of tomorrow, then the week. Each
 * item enters in turn as dusk falls ([handover]).
 */
private fun LazyListScope.tomorrow(t: Tomorrow, state: UiState, actions: Actions, dusk: () -> DuskLook) {
    var n = 0
    item("tomorrow-first") {
        val d = LocalDesign.current
        val r = d.room
        val on = shown(0, dusk)
        Column(Modifier.handover(0, dusk).fillMaxWidth().padding(start = S.S5.dp, end = S.S5.dp, top = S.S5.dp)) {
            Label("When the sun comes up", Tokens.Type.CAPS_LABEL, r.ink3)
            Spacer(Modifier.height(S.S3.dp))
            HandoverRow(t.first, state, actions, enabled = on, gap = S.S4.dp) {
                Box(
                    Modifier.size(Tokens.Metrics.Component.BUTTON.dp + S.S3.dp).background(r.tint, CircleShape),
                    contentAlignment = Alignment.Center,
                ) {
                    Icon("sunrise", r.accentStrong)
                }
                Column(Modifier.weight(1f)) {
                    Label(t.first.title, Tokens.Type.NEXT_TITLE, r.ink, maxLines = 2)
                    Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(S.S2.dp)) {
                        Label(t.firstWhen, Tokens.Type.DUE, r.accentStrong, weight = Tokens.Type.OVERDUE_BADGE.weight)
                        t.first.chip?.let { ListChip(it) }
                    }
                }
            }
            Spacer(Modifier.height(S.S4.dp))
            // Planning the next day happens in Planned, where tomorrow's tasks are (as on the desktop).
            Button("Plan ${t.weekday}", primary = true, enabled = on) { actions.open(RoomKey.Smart(SmartList.PLANNED)) }
        }
    }
    if (t.also.isNotEmpty()) {
        val caps = ++n
        item("tomorrow-also") { Caps("Also on ${t.weekday}", Modifier.handover(caps, dusk)) }
        val first = n + 1
        n += t.also.size
        itemsIndexed(t.also, key = { _, it -> "also-" + it.id }) { i, it -> SmallRow(it, state, actions, first + i, dusk) }
    }
    if (t.week.isNotEmpty()) {
        val caps = ++n
        item("tomorrow-week") { Caps("Rest of the week", Modifier.handover(caps, dusk)) }
        val first = n + 1
        n += t.week.size
        itemsIndexed(t.week, key = { _, it -> "week-" + it.id }) { i, it -> SmallRow(it, state, actions, first + i, dusk) }
    }
    if (t.more > 0u) {
        val last = ++n
        item("tomorrow-more") {
            val r = LocalDesign.current.room
            Label(
                "${t.more} more",
                Tokens.Type.COUNT,
                r.ink3,
                Modifier.handover(last, dusk).padding(start = S.S5.dp + S.S8.dp, top = S.S3.dp, bottom = S.S6.dp),
            )
        }
    }
}

private fun LazyListScope.planOnly(weekday: String, actions: Actions, dusk: () -> DuskLook) {
    item("plan-next") {
        val r = LocalDesign.current.room
        val on = shown(0, dusk)
        Column(Modifier.handover(0, dusk).fillMaxWidth().padding(start = S.S5.dp, end = S.S5.dp, top = S.S5.dp)) {
            Label("When the sun comes up", Tokens.Type.CAPS_LABEL, r.ink3)
            Spacer(Modifier.height(S.S2.dp))
            Label("Nothing is due on $weekday yet.", Tokens.Type.NOTES, r.ink2)
            Spacer(Modifier.height(S.S4.dp))
            Button("Plan $weekday", primary = true, enabled = on) { actions.open(RoomKey.Smart(SmartList.PLANNED)) }
        }
    }
}

@Composable
private fun Caps(text: String, modifier: Modifier = Modifier) {
    val r = LocalDesign.current.room
    Label(text, Tokens.Type.CAPS_LABEL, r.ink3, modifier.padding(start = S.S5.dp, top = S.S6.dp, bottom = S.S2.dp))
}

@Composable
private fun HandoverRow(t: TaskRow, state: UiState, actions: Actions, enabled: Boolean, gap: Dp,
    modifier: Modifier = Modifier, inset: Modifier = Modifier, content: @Composable RowScope.() -> Unit,
) {
    val r = LocalDesign.current.room
    val isSelected = t.id in state.bulkIds
    val toggle = { actions.toggleBulk(t.id, state.generation) }
    Row(modifier.fillMaxWidth().then(if (isSelected) Modifier.background(r.selectionWash).selectionBar(r.accentStrong) else Modifier)
        .combinedClickable(enabled = enabled, onLongClickLabel = "Select task", onLongClick = toggle,
            onClick = { if (state.bulk != null) toggle() else actions.select(t.id) })
        .semantics { selected = isSelected }.then(inset), verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(gap), content = content)
}

/** A compact row for the hand-over (item [i] of it): check, title, chip, and the date on the right. */
@Composable
private fun SmallRow(t: TaskRow, state: UiState, actions: Actions, i: Int, dusk: () -> DuskLook) {
    val r = LocalDesign.current.room
    val on = shown(i, dusk)
    HandoverRow(t, state, actions, enabled = on, gap = S.S2.dp,
        modifier = Modifier.handover(i, dusk).height(P.TOUCH.dp).hairline(r.line, top = true),
        inset = Modifier.padding(start = S.S3.dp, end = S.S5.dp),
    ) {
        Check(t.done, { actions.setDone(t.id, it, state.generation) }, t.title, enabled = on && state.bulk == null,
            onSelectionWash = t.id in state.bulkIds)
        Row(Modifier.weight(1f), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(S.S2.dp)) {
            Label(t.title, Tokens.Type.TASK_TITLE, r.ink, Modifier.weight(1f, fill = false))
            t.chip?.let { ListChip(it) }
        }
        t.due?.let { Label(it.text, Tokens.Type.DUE, r.ink2) }
    }
}

/**
 * The 56 dp strip above the composer that holds the toast, so it never covers the last row. The thin bar
 * at the bottom counts down the undo window.
 */
@Composable
internal fun ToastStrip(toast: Toast?, actions: Actions) {
    val r = LocalDesign.current.room
    AnimatedVisibility(
        visible = toast != null,
        enter = fadeIn(tween(M.TOAST_IN)) + slideInVertically(tween(M.TOAST_IN)) { it / 2 },
        exit = fadeOut(tween(M.TOAST_OUT)) + slideOutVertically(tween(M.TOAST_OUT)) { it / 2 },
    ) {
        val t = remember(toast?.serial) { toast } ?: return@AnimatedVisibility
        // A functional timer: its full length whatever the animation scale (even 0), the bar following it.
        val window = M.UNDO_WINDOW.toLong()
        var left by remember(t.serial) { mutableFloatStateOf((actions.toastRemaining(t) ?: window).toFloat() / window) }
        LaunchedEffect(t.serial) {
            if (actions.toastRemaining(t) == null) countdown(window) { left = it } else {
                while (left > 0f) androidx.compose.runtime.withFrameMillis {
                    left = (actions.toastRemaining(t) ?: 0L).toFloat() / window
                }
            }
            actions.dismissToast(t)
        }
        Box(
            Modifier.fillMaxWidth().height(P.TOAST_STRIP.dp).background(r.pane).hairline(r.line, top = true).padding(horizontal = S.S3.dp),
            contentAlignment = Alignment.Center,
        ) {
            val shape = RoundedCornerShape(Tokens.Radius.TOAST.dp)
            Box(Modifier.fillMaxWidth().height(P.TOAST.dp).clip(shape).background(r.toast)) {
                Row(
                    Modifier.fillMaxSize().padding(start = S.S3.dp, end = S.S1.dp),
                    verticalAlignment = Alignment.CenterVertically,
                    horizontalArrangement = Arrangement.spacedBy(S.S2.dp),
                ) {
                    Label(t.text, Tokens.Type.TOAST, r.toastInk, Modifier.weight(1f))
                    if (t.undoable) {
                        Row(
                            Modifier
                                .height(P.TOAST.dp)
                                .clickable { actions.undo(t) }
                                .padding(horizontal = S.S2.dp),
                            verticalAlignment = Alignment.CenterVertically,
                            horizontalArrangement = Arrangement.spacedBy(S.S2.dp),
                        ) {
                            Icon("undo-2", r.toastAccent, size = Tokens.Metrics.Component.ICON_SMALL.dp)
                            Label("Undo", Tokens.Type.TOAST_ACTION, r.toastAccent)
                        }
                    }
                    Box(
                        Modifier.size(P.TOAST.dp).clickable { actions.dismissToast(t) }.semantics { contentDescription = "Dismiss" },
                        contentAlignment = Alignment.Center,
                    ) {
                        Icon("x", r.toastInk2, size = Tokens.Metrics.Component.ICON_SMALL.dp)
                    }
                }
                Box(
                    Modifier
                        .align(Alignment.BottomStart)
                        .padding(start = S.S3.dp, bottom = Tokens.Elevation.SELECTION_BAR.dp)
                        .fillMaxWidth(0.6f * left)
                        .height(Tokens.Elevation.HAIRLINE.dp * 2)
                        .background(r.toastInk2.copy(alpha = 0.55f), RoundedCornerShape(Tokens.Elevation.HAIRLINE.dp)),
                )
            }
        }
    }
}

/**
 * The empty line ("Nothing here yet.", "Nothing matches the filter.") only when nothing at all is listed:
 * no open tasks, no completed ones, and no hand-over or planning for tomorrow. While [ghosts] play out
 * (the last match just completed, say) it waits until they've gone, so it never pushes them down.
 */
fun showsEmptyLine(room: RoomView, ghosts: List<Ghost> = emptyList()): Boolean =
    room.tasks.isEmpty() && room.completed.isEmpty() && room.tomorrow == null && room.planWeekday == null &&
        ghosts.isEmpty()
