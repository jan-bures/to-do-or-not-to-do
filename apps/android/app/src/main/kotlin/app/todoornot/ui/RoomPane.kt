package app.todoornot.ui

import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.core.animate
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.asPaddingValues
import androidx.compose.foundation.layout.ime
import androidx.compose.foundation.layout.statusBars
import androidx.compose.foundation.layout.isImeVisible
import androidx.compose.ui.input.nestedscroll.nestedScroll
import androidx.window.core.layout.WindowSizeClass
import androidx.compose.animation.core.Animatable
import androidx.compose.animation.core.LinearEasing
import androidx.compose.animation.core.tween
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.slideInVertically
import androidx.compose.animation.slideOutVertically
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
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
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.lazy.LazyColumn
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
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.KeyboardCapitalization
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
import app.todoornot.model.Actions
import app.todoornot.model.DuskLook
import app.todoornot.model.MotionFrame
import app.todoornot.model.rowIn
import app.todoornot.model.Toast
import app.todoornot.model.UiState

private val P = Tokens.Metrics.Phone
private val S = Tokens.Space
private val M = Tokens.Motion

/** Rows that stay visible above the composer while typing (`docs/design-language.md`). */
private const val MIN_ROWS_TYPING = 4

/**
 * The list pane: header, room pills, the tasks, the toast strip and the composer. The header and the
 * pills give way to the tasks as the list scrolls, and stay out of the way while the keyboard is open or
 * the window is short ([collapsed] forces it, for the gallery).
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
        val composer = Tokens.Elevation.HAIRLINE.dp + S.S3.dp + P.COMPOSER.dp + S.S3.dp
        val underBar = maxHeight - keyboardHeight - statusBar - P.APP_BAR.dp - composer
        val bar = !keyboard || underBar >= P.ROW.dp * MIN_ROWS_TYPING
        val force = if (collapsed || keyboard || short) 1f else 0f
        val collapse = remember(room.key) { HeaderCollapse().also { it.forced = force } }
        LaunchedEffect(collapse, force) {
            animate(collapse.forced, force, animationSpec = tween(M.PANEL, easing = M.EASE_STANDARD.cubic())) { v, _ ->
                collapse.forced = v
            }
        }
        // The model holds the filter and whether its field is open, so the text and the rows agree.
        Column(Modifier.fillMaxSize().nestedScroll(collapse.connection)) {
            RoomHeader(room, landscape, collapse, onMenu, bar) {
                SyncChip(state.status, actions::nudge)
                IconButton("search", "Filter", tint = if (state.searching) LocalDesign.current.room.accentStrong else null) {
                    actions.setSearching(!state.searching)
                }
            }
            Box(Modifier.collapsing(collapse, HeaderCollapse.Part.Pills)) {
                RoomPills(state.home, room.key, actions::open, onMenu)
            }
            if (state.searching) FilterField(state.filter, actions::setFilter) { actions.setSearching(false) }
            state.failed.firstOrNull()?.let { f ->
                Banner("Not saved: ${f.what}. ${f.reason}", "Dismiss") { actions.dismissFailed(f.id) }
            }
            if (state.signedOut) Banner("This phone isn't paired any more.", "Pair again", actions::unpair)
            Box(Modifier.weight(1f).fillMaxWidth().hairline(r.line, top = true)) {
                TaskList(room, state.selected, state.showCompleted, actions)
            }
            ToastStrip(state.toast, actions)
            Composer(room.addPlaceholder, draft, keyboard, actions::add)
        }
    }
}

/**
 * The filter, under the room pills: the same query language as the desktop's (`tag:docs`, `due<=friday`,
 * `priority>=high`), or plain words to find in titles. The rows narrow as you type; the progress doesn't.
 */
@Composable
private fun FilterField(text: String, onChange: (String) -> Unit, onClose: () -> Unit) {
    val r = LocalDesign.current.room
    val focus = remember { FocusRequester() }
    LaunchedEffect(Unit) { if (text.isEmpty()) runCatching { focus.requestFocus() } }
    val shape = RoundedCornerShape(Tokens.Radius.MD.dp)
    Box(Modifier.fillMaxWidth().background(r.pane).hairline(r.line, top = true).padding(horizontal = S.S4.dp, vertical = S.S2.dp)) {
        Row(
            Modifier
                .fillMaxWidth()
                .heightIn(min = P.TOUCH.dp)
                .clip(shape)
                .background(r.raised)
                .border(Tokens.Elevation.HAIRLINE.dp, if (text.isEmpty()) r.control else r.accent, shape)
                .padding(start = S.S3.dp),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(S.S2.dp),
        ) {
            Icon("search", if (text.isEmpty()) r.ink3 else r.accentStrong, size = Tokens.Metrics.Component.ICON_SMALL.dp)
            BasicTextField(
                value = text,
                onValueChange = { onChange(it.replace("\n", "")) },
                modifier = Modifier.weight(1f).focusRequester(focus).semantics { contentDescription = "Filter" },
                textStyle = Tokens.Type.SUMMARY.style(r.ink),
                singleLine = true,
                cursorBrush = SolidColor(r.accent),
                keyboardOptions = KeyboardOptions(imeAction = ImeAction.Search, autoCorrectEnabled = false),
                keyboardActions = KeyboardActions(onSearch = { }),
                decorationBox = { inner ->
                    Box(contentAlignment = Alignment.CenterStart) {
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
fun IconButton(icon: String, label: String, tint: androidx.compose.ui.graphics.Color? = null, onClick: () -> Unit) {
    val r = LocalDesign.current.room
    Box(
        Modifier.size(P.TOUCH.dp).clip(CircleShape).clickable(onClick = onClick).semantics { contentDescription = label },
        contentAlignment = Alignment.Center,
    ) {
        Icon(icon, tint ?: r.ink2)
    }
}

/** Shown only when there's something to say: offline, syncing changes, or a problem. */
@Composable
private fun SyncChip(status: SyncStatus, onClick: () -> Unit) {
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
private fun Banner(text: String, action: String, onAction: () -> Unit) {
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
 * has begun to appear.
 */
@Composable
private fun TaskList(room: RoomView, selected: String?, showDone: Boolean, actions: Actions) {
    val r = LocalDesign.current.room
    val motion = LocalMotion.current
    val dusk: () -> DuskLook = remember(room.key, room.finished, motion) {
        { motion.dusk(room.key) ?: DuskLook.rest(room.finished) }
    }
    LazyColumn(Modifier.fillMaxSize()) {
        // Only when nothing at all is listed: a filter that matches only done tasks shows them under
        // Completed, without a "nothing matches" line above them.
        if (showsEmptyLine(room)) {
            item("empty") { Empty(room.emptyLine) }
        }
        items(room.tasks, key = { it.id }) { t -> MovingRow(t, inCompleted = false, selected, actions, motion) }
        room.tomorrow?.let { tomorrow(it, actions, dusk) }
        // Finished with nothing due tomorrow: no hand-over, but planning the next day is still offered.
        if (room.tomorrow == null) room.planWeekday?.let { weekday -> planOnly(weekday, actions, dusk) }
        // The finished day keeps today's Completed section under the hand-over, as on the desktop.
        if (room.completed.isNotEmpty()) {
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
                items(room.completed, key = { "done-" + it.id }) { t -> MovingRow(t, inCompleted = true, selected, actions, motion) }
            }
        }
    }
}

/**
 * A task row with its choreography: only a row that moves recomposes as the frames go by. A row at
 * rest that the list just gained (a new task, another room) fades in; one arriving from its
 * choreography enters as that says.
 */
@Composable
private fun LazyItemScope.MovingRow(t: TaskRow, inCompleted: Boolean, selected: String?, actions: Actions, motion: MotionFrame) {
    val look by remember(t.id, inCompleted, motion) { derivedStateOf { motion.rowIn(t.id, inCompleted) } }
    val checked by remember(t.id, motion) { derivedStateOf { motion.checked(t.id) } }
    val l = look
    TaskRowView(
        t,
        selected = t.id == selected,
        checked = checked ?: t.done,
        onCheck = { actions.setDone(t.id, it) },
        onOpen = { actions.select(t.id) },
        modifier = Modifier.animateItem(
            fadeInSpec = if (l == null) tween(M.ROW_ENTER, easing = M.EASE_ENTER.cubic()) else null,
            placementSpec = null,
            fadeOutSpec = null,
        ),
        look = l,
    )
}

@Composable
private fun Empty(line: String) {
    val r = LocalDesign.current.room
    Column(Modifier.fillMaxWidth().padding(horizontal = S.S5.dp, vertical = S.S8.dp)) {
        Label(line, Tokens.Type.EMPTY_LINE, r.ink3, maxLines = 3)
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
private fun LazyListScope.tomorrow(t: Tomorrow, actions: Actions, dusk: () -> DuskLook) {
    var n = 0
    item("tomorrow-first") {
        val d = LocalDesign.current
        val r = d.room
        val on = shown(0, dusk)
        Column(Modifier.handover(0, dusk).fillMaxWidth().padding(start = S.S5.dp, end = S.S5.dp, top = S.S5.dp)) {
            Label("When the sun comes up", Tokens.Type.CAPS_LABEL, r.ink3)
            Spacer(Modifier.height(S.S3.dp))
            Row(
                Modifier.fillMaxWidth().clickable(enabled = on) { actions.select(t.first.id) },
                verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.spacedBy(S.S4.dp),
            ) {
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
        itemsIndexed(t.also, key = { _, it -> "also-" + it.id }) { i, it -> SmallRow(it, actions, first + i, dusk) }
    }
    if (t.week.isNotEmpty()) {
        val caps = ++n
        item("tomorrow-week") { Caps("Rest of the week", Modifier.handover(caps, dusk)) }
        val first = n + 1
        n += t.week.size
        itemsIndexed(t.week, key = { _, it -> "week-" + it.id }) { i, it -> SmallRow(it, actions, first + i, dusk) }
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

/** A compact row for the hand-over (item [i] of it): check, title, chip, and the date on the right. */
@Composable
private fun SmallRow(t: TaskRow, actions: Actions, i: Int, dusk: () -> DuskLook) {
    val r = LocalDesign.current.room
    val on = shown(i, dusk)
    Row(
        Modifier
            .handover(i, dusk)
            .fillMaxWidth()
            .height(P.TOUCH.dp)
            .hairline(r.line, top = true)
            .clickable(enabled = on) { actions.select(t.id) }
            .padding(start = S.S3.dp, end = S.S5.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(S.S2.dp),
    ) {
        Check(t.done, { actions.setDone(t.id, it) }, t.title, enabled = on)
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
private fun ToastStrip(toast: Toast?, actions: Actions) {
    val r = LocalDesign.current.room
    AnimatedVisibility(
        visible = toast != null,
        enter = fadeIn(tween(M.TOAST_IN)) + slideInVertically(tween(M.TOAST_IN)) { it / 2 },
        exit = fadeOut(tween(M.TOAST_OUT)) + slideOutVertically(tween(M.TOAST_OUT)) { it / 2 },
    ) {
        val t = remember(toast?.serial) { toast } ?: return@AnimatedVisibility
        val left = remember(t.serial) { Animatable(1f) }
        LaunchedEffect(t.serial) {
            left.animateTo(0f, tween(M.UNDO_WINDOW, easing = LinearEasing))
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
                    if (t.reopen != null) {
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
                        .fillMaxWidth(0.6f * left.value)
                        .height(Tokens.Elevation.HAIRLINE.dp * 2)
                        .background(r.toastInk2.copy(alpha = 0.55f), RoundedCornerShape(Tokens.Elevation.HAIRLINE.dp)),
                )
            }
        }
    }
}

/** The single 48 dp composer at the bottom, docked above the keyboard. Enter adds the task to this room. */
@Composable
private fun Composer(placeholder: String, draft: String, keyboard: Boolean, onAdd: (String) -> Unit) {
    val r = LocalDesign.current.room
    var text by rememberSaveable(draft) { mutableStateOf(draft) }
    val shape = RoundedCornerShape(Tokens.Radius.COMPOSER.dp)
    fun submit() {
        val t = text.trim()
        if (t.isNotEmpty()) onAdd(t)
        text = ""
    }
    Box(
        Modifier
            .fillMaxWidth()
            .background(r.pane)
            .hairline(r.line, top = true)
            .navigationBarsPadding()
            .imePadding()
            .padding(start = S.S4.dp, end = S.S4.dp, top = S.S3.dp, bottom = if (keyboard) S.S3.dp else S.S4.dp),
    ) {
        Row(
            Modifier
                .fillMaxWidth()
                .height(P.COMPOSER.dp)
                .clip(shape)
                .background(r.raised)
                .border(Tokens.Elevation.HAIRLINE.dp, r.control, shape)
                .padding(start = S.S3.dp, end = S.S4.dp),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(S.S3.dp),
        ) {
            Icon("plus", r.accentStrong)
            BasicTextField(
                value = text,
                onValueChange = { text = it },
                modifier = Modifier.weight(1f).semantics { contentDescription = placeholder },
                textStyle = Tokens.Type.COMPOSER.style(r.ink),
                singleLine = true,
                cursorBrush = SolidColor(r.accent),
                keyboardOptions = KeyboardOptions(capitalization = KeyboardCapitalization.Sentences, imeAction = ImeAction.Done),
                keyboardActions = KeyboardActions(onDone = { submit() }),
                decorationBox = { inner ->
                    Box(contentAlignment = Alignment.CenterStart) {
                        if (text.isEmpty()) Label(placeholder, Tokens.Type.COMPOSER, r.ink3)
                        inner()
                    }
                },
            )
        }
    }
}

/**
 * The empty line ("Nothing here yet.", "Nothing matches the filter.") only when nothing at all is listed:
 * no open tasks, no completed ones, and no hand-over or planning for tomorrow.
 */
fun showsEmptyLine(room: RoomView): Boolean =
    room.tasks.isEmpty() && room.completed.isEmpty() && room.tomorrow == null && room.planWeekday == null
