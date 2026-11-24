package app.todoornot.ui

import androidx.activity.compose.PredictiveBackHandler
import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.animation.core.snap
import androidx.compose.animation.core.tween
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.gestures.Orientation
import androidx.compose.foundation.gestures.awaitEachGesture
import androidx.compose.foundation.gestures.awaitFirstDown
import androidx.compose.foundation.gestures.draggable
import androidx.compose.foundation.gestures.rememberDraggableState
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.isImeVisible
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.LazyListState
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.runtime.staticCompositionLocalOf
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.semantics.LiveRegionMode
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.heading
import androidx.compose.ui.semantics.liveRegion
import androidx.compose.ui.semantics.paneTitle
import androidx.compose.ui.semantics.selected
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.input.KeyboardCapitalization
import androidx.compose.ui.unit.IntOffset
import androidx.compose.ui.unit.dp
import app.todoornot.core.AssistantConnection
import app.todoornot.core.AssistantPanelState
import app.todoornot.core.AssistantProposalStatus
import app.todoornot.core.AssistantTurn
import app.todoornot.design.Icon
import app.todoornot.design.LocalDesign
import app.todoornot.design.Tokens
import app.todoornot.design.cubic
import app.todoornot.design.style
import app.todoornot.model.AssistantModel
import app.todoornot.model.AssistantUi
import kotlin.math.roundToInt
import kotlinx.coroutines.flow.collect
import kotlinx.coroutines.launch

private val S = Tokens.Space
private val P = Tokens.Metrics.Phone
private val C = Tokens.Metrics.Component
private val T = Tokens.Type

internal data class AskAiControl(val open: Boolean, val pending: Boolean, val toggle: () -> Unit)
internal val LocalAskAi = staticCompositionLocalOf<AskAiControl?> { null }
internal val LocalOpenAiSettings = staticCompositionLocalOf<(() -> Unit)?> { null }

@Composable
internal fun AskAiButton() {
    val control = LocalAskAi.current ?: return
    val r = LocalDesign.current.room
    val label = if (control.pending && !control.open) "Ask AI, 1 proposal waiting" else "Ask AI"
    Box(Modifier.size(P.TOUCH.dp).clip(RoundedCornerShape(Tokens.Radius.MD.dp))
        .clickable(role = Role.Button, onClick = control.toggle)
        .semantics { contentDescription = label; selected = control.open }, contentAlignment = Alignment.Center) {
        val shape = RoundedCornerShape(Tokens.Radius.MD.dp)
        Box(Modifier.size(P.COMPACT_HEADER.dp).then(if (control.open) Modifier.background(r.wash, shape) else Modifier),
            contentAlignment = Alignment.Center) {
            Icon("sparkles", r.accentStrong)
        }
        if (control.pending && !control.open) {
            Label("1", T.COUNT, r.accentStrong, Modifier.align(Alignment.TopEnd).padding(end = S.S1.dp))
        }
    }
}

internal enum class AssistantOverlay { SETTINGS, HISTORY }

/** Handle drag follows the finger; below the minimum height it becomes a dismissal drag. */
@Composable
internal fun AssistantCover(onClose: () -> Unit, blocked: Boolean, content: @Composable (Modifier, () -> Unit) -> Unit) {
    val d = LocalDesign.current
    var fraction by rememberSaveable { mutableFloatStateOf(P.AI_SHEET_DEFAULT_FRACTION) }
    var drag by remember { mutableFloatStateOf(0f) }
    var back by remember { mutableFloatStateOf(0f) }
    var dragging by remember { mutableStateOf(false) }
    val reduce = reducedMotion() || LocalSheetAtRest.current
    var held by remember { mutableStateOf(reduce) }
    var leaving by remember { mutableStateOf(false) }
    val duration = if (reduce) 0 else Tokens.Motion.PANEL
    val requestClose = { leaving = true }
    val entrance by animateFloatAsState(if (!held || leaving) 1f else 0f,
        tween(duration, easing = Tokens.Motion.EASE_STANDARD.cubic()), label = "AI sheet entrance")
    LaunchedEffect(Unit) { held = true }
    LaunchedEffect(leaving, entrance) { if (leaving && entrance == 1f) onClose() }
    PredictiveBackHandler(enabled = !leaving && !blocked) { progress ->
        try {
            progress.collect { back = if (reduce) 0f else it.progress }
            requestClose()
        } finally { back = 0f }
    }
    val shape = RoundedCornerShape(topStart = Tokens.Radius.XL.dp, topEnd = Tokens.Radius.XL.dp)
    Box(Modifier.fillMaxSize(), contentAlignment = Alignment.BottomCenter) {
        Box(Modifier.fillMaxSize().background(d.scrim.copy(alpha = d.scrim.alpha * (1f - maxOf(entrance, back))))
            .clickable(onClickLabel = "Dismiss AI assistant", onClick = requestClose))
        BoxWithConstraints(Modifier.fillMaxSize().imePadding(), contentAlignment = Alignment.BottomCenter) {
            val keyboard = WindowInsets.isImeVisible
            val short = maxHeight < P.AI_SHEET_SHORT_WINDOW.dp
            val base = if (keyboard) 1f else if (short) P.AI_SHEET_SHORT_FRACTION else fraction
            val min = P.AI_SHEET_MIN_FRACTION
            val max = if (short || keyboard) base else P.SHEET_MAX_FRACTION
            val density = LocalDensity.current
            val px = with(density) { maxHeight.toPx() }.coerceAtLeast(1f)
            val dismissThreshold = with(density) { P.TOUCH.dp.toPx() } / px
            val settling by animateFloatAsState(if (dragging) drag else 0f,
                if (dragging) snap() else tween(duration), label = "AI sheet drag")
            val size = (base - settling).coerceIn(min, max)
            val slip = (settling - (base - min)).coerceAtLeast(0f) * px
            val slide = maxOf(entrance, back) * size * px
            val handle = Modifier.draggable(rememberDraggableState { delta ->
                drag = (drag + delta / px).coerceAtLeast(base - max)
            }, Orientation.Vertical, enabled = !keyboard && !leaving,
                onDragStarted = { dragging = true; drag = settling }, onDragStopped = {
                    if (base - drag < min - dismissThreshold) requestClose()
                    else if (!short) fraction = (base - drag).coerceIn(min, max)
                    drag = 0f
                    dragging = false
                })
            Column(Modifier.fillMaxWidth().height(maxHeight * size)
                .offset { IntOffset(0, (slip + slide).roundToInt()) }
                .clip(shape).background(d.room.pane)
                .then(if (d.dark) Modifier.border(Tokens.Elevation.HAIRLINE.dp, d.room.lineStrong, shape) else Modifier)
                .pointerInput(Unit) { // The empty sheet is not part of its dismissible backdrop.
                    awaitEachGesture { awaitFirstDown(requireUnconsumed = false) }
                }
                .navigationBarsPadding().semantics { paneTitle = "AI assistant" }) {
                content(handle, requestClose)
            }
        }
    }
}

@Composable
internal fun AssistantPane(
    ui: AssistantUi,
    model: AssistantModel?,
    room: String,
    inner: Boolean,
    left: Boolean,
    scroll: LazyListState,
    overlay: (AssistantOverlay) -> Unit,
    onClose: () -> Unit,
    onTask: (String) -> Unit,
    modifier: Modifier = Modifier,
    handle: Modifier = Modifier,
) {
    val r = LocalDesign.current.room
    val state = ui.panel
    val pending = state.turns.lastOrNull()?.proposal?.status == AssistantProposalStatus.PENDING
    val keyboard = WindowInsets.isImeVisible
    val title = state.turns.firstOrNull()?.prompt?.lineSequence()?.firstOrNull() ?: "New chat"
    val settings = { overlay(AssistantOverlay.SETTINGS) }
    val history = { overlay(AssistantOverlay.HISTORY) }
    val newChat = { model?.newChat(); Unit }
    val draft: (String) -> Unit = { model?.draft(it) }
    val turns = state.turns.asReversed()
    val scope = rememberCoroutineScope()
    LaunchedEffect(turns.firstOrNull()?.id) { if (turns.isNotEmpty()) scroll.scrollToItem(0) }
    Column(modifier.fillMaxSize().background(r.pane)
        .then(if (inner) Modifier.imePadding().navigationBarsPadding() else Modifier)
        .semantics { if (inner) paneTitle = "AI assistant" }) {
        if (inner) {
            Row(Modifier.fillMaxWidth().height(P.DETAIL_BREADCRUMB.dp).hairline(r.line).padding(horizontal = S.S5.dp),
                verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(S.S2.dp)) {
                AssistantMark()
                Label(title, T.TASK_TITLE, r.ink, Modifier.weight(1f).semantics { heading() })
                DestinationText(state)
            }
        } else {
            Column(handle.fillMaxWidth().hairline(r.line)) {
                Box(Modifier.fillMaxWidth().padding(vertical = S.S2.dp), contentAlignment = Alignment.Center) {
                    Box(Modifier.size(P.SHEET_HANDLE_WIDTH.dp, S.S1.dp)
                        .background(r.lineStrong, RoundedCornerShape(Tokens.Radius.SM.dp)))
                }
                Row(Modifier.fillMaxWidth().padding(start = S.S2.dp, end = S.S1.dp), verticalAlignment = Alignment.CenterVertically) {
                    Row(Modifier.weight(1f).heightIn(min = P.TOUCH.dp).clip(RoundedCornerShape(Tokens.Radius.MD.dp))
                        .clickable(role = Role.Button, onClickLabel = "Conversation history", onClick = history)
                        .padding(horizontal = S.S2.dp), verticalAlignment = Alignment.CenterVertically,
                        horizontalArrangement = Arrangement.spacedBy(S.S2.dp)) {
                        AssistantMark()
                        Label(title, T.ROOM_TITLE_BAR, r.ink, Modifier.weight(1f, fill = false))
                        Icon("chevron-down", r.ink3, size = C.ICON_SMALL.dp)
                    }
                    IconButton("plus", "New chat", enabled = turns.isNotEmpty() && !ui.busy, onClick = newChat)
                    IconButton("settings", "AI settings", onClick = settings)
                    IconButton("x", "Close assistant", onClick = onClose)
                }
            }
        }
        LazyColumn(Modifier.weight(1f).fillMaxWidth(), state = scroll, reverseLayout = true,
            contentPadding = PaddingValues(horizontal = if (inner) S.S5.dp else S.S4.dp, vertical = S.S4.dp),
            verticalArrangement = Arrangement.spacedBy(S.S4.dp, Alignment.Bottom)) {
            if (turns.isEmpty()) item("empty") { AssistantEmpty(state.server != null, settings, draft) }
            items(turns, key = { it.id }) { turn ->
                ChatTurn(turn, active = state.streaming && turn.id == turns.firstOrNull()?.id,
                    left = left, enabled = !ui.busy && !state.streaming, model = model, onTask = onTask, onSettings = settings,
                    onRetry = if (turn.id == turns.firstOrNull()?.id && !state.streaming && !ui.busy && ui.draft.isBlank()) {{ model?.retry(turn.id) }} else null)
            }
        }
        ui.problem?.let { AssistantProblem(it) }
        if (pending) {
            Row(Modifier.fillMaxWidth().heightIn(min = P.ROOM_PILL.dp).background(r.wash)
                .hairline(r.line, top = true).hairline(r.line).padding(horizontal = S.S4.dp)
                .semantics { liveRegion = LiveRegionMode.Polite },
                horizontalArrangement = Arrangement.spacedBy(S.S2.dp), verticalAlignment = Alignment.CenterVertically) {
                Icon("circle-alert", r.accentStrong, size = C.ICON_SMALL.dp)
                Label("1 proposal waiting for review", T.META_PHONE, r.ink, Modifier.weight(1f), maxLines = 2)
                if (scroll.firstVisibleItemIndex > 0 || scroll.firstVisibleItemScrollOffset > 0) {
                    AiButton("Jump to proposal", { scope.launch { scroll.scrollToItem(0) } }, quiet = true)
                }
            }
        }
        if (!inner && !keyboard) {
            Row(Modifier.fillMaxWidth().padding(horizontal = S.S2.dp), verticalAlignment = Alignment.CenterVertically) {
                ModelControl(state, Modifier.weight(1f), settings)
                Row(Modifier.heightIn(min = P.TOUCH.dp).clip(RoundedCornerShape(Tokens.Radius.MD.dp))
                    .clickable(role = Role.Button, onClickLabel = "List access settings", onClick = settings)
                    .padding(horizontal = S.S2.dp), verticalAlignment = Alignment.CenterVertically,
                    horizontalArrangement = Arrangement.spacedBy(S.S1.dp)) {
                    Icon("eye-off", r.ink3, size = C.ICON_SMALL.dp)
                    Label(if (state.hidden == 1u) "1 list hidden" else "${state.hidden} lists hidden", T.META_PHONE, r.ink2)
                }
            }
        }
        ChatComposer(ui.draft, draft, state.server != null, state.streaming, ui.busy || state.connection == AssistantConnection.TESTING,
            if (pending) "Approve above, or reply…" else if (inner) "Ask about $room…" else "Ask anything…",
            onSend = { model?.send() }, onStop = { model?.stop() })
        if (inner && !keyboard) {
            Row(Modifier.fillMaxWidth().height(P.BOTTOM_BAR.dp).background(r.canvas).hairline(r.line, top = true)
                .padding(horizontal = S.S1.dp), verticalAlignment = Alignment.CenterVertically) {
                IconButton("clock", "Conversation history", onClick = history)
                ModelControl(state, Modifier.weight(1f), settings)
                IconButton("plus", "New chat", enabled = turns.isNotEmpty() && !ui.busy, onClick = newChat)
                IconButton("settings", "AI settings", onClick = settings)
                IconButton("x", "Close assistant", onClick = onClose)
            }
        }
    }
}

@Composable
private fun DestinationText(state: AssistantPanelState, modifier: Modifier = Modifier, model: Boolean = false) {
    val r = LocalDesign.current.room
    val server = state.server
    val label = when (state.connection) {
        AssistantConnection.UNREACHABLE -> "Server unreachable"
        AssistantConnection.TESTING -> "Testing connection…"
        else -> state.destination ?: "Choose a model"
    }
    val color = when (state.connection) {
        AssistantConnection.UNREACHABLE -> r.danger
        AssistantConnection.TESTING -> r.accent
        AssistantConnection.CONNECTED -> r.success
        AssistantConnection.UNTESTED -> null
    }
    Row(modifier, verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(S.S1.dp)) {
        color?.let { Box(Modifier.size(C.STATUS_LIGHT.dp).background(it, CircleShape)) }
        Label(if (model && server != null) "$label · ${server.model}" else label, T.META_PHONE, r.ink2)
    }
}

@Composable
private fun ModelControl(state: AssistantPanelState, modifier: Modifier, onClick: () -> Unit) {
    val r = LocalDesign.current.room
    val shape = RoundedCornerShape(Tokens.Radius.MD.dp)
    Box(modifier.heightIn(min = P.TOUCH.dp).clip(shape).clickable(role = Role.Button,
        onClickLabel = "Model and privacy", onClick = onClick), contentAlignment = Alignment.CenterStart) {
        Row(Modifier.fillMaxWidth().heightIn(min = P.COMPACT_HEADER.dp).background(r.raised, shape)
            .border(Tokens.Elevation.HAIRLINE.dp, r.line, shape).padding(horizontal = S.S2.dp),
            horizontalArrangement = Arrangement.spacedBy(S.S1.dp), verticalAlignment = Alignment.CenterVertically) {
            DestinationText(state, Modifier.weight(1f), model = true)
            Icon("chevron-down", r.ink3, size = C.ICON_SMALL.dp)
        }
    }
}

@Composable
internal fun AssistantMark() {
    val mark = LocalDesign.current.agent("assistant", true)
    Box(Modifier.size(C.AGENT_MARK.dp).background(mark.bg, CircleShape)
        .border(Tokens.Elevation.HAIRLINE.dp, mark.ring, CircleShape), contentAlignment = Alignment.Center) {
        Label("A", T.AGENT_LETTER, mark.ink)
    }
}

@Composable
private fun AssistantEmpty(configured: Boolean, settings: () -> Unit, draft: (String) -> Unit) {
    val r = LocalDesign.current.room
    val shape = RoundedCornerShape(Tokens.Radius.LG.dp)
    Column(Modifier.fillMaxWidth(), verticalArrangement = Arrangement.spacedBy(S.S2.dp)) {
        Box(Modifier.size(P.COMPACT_HEADER.dp).background(r.wash, shape).border(Tokens.Elevation.HAIRLINE.dp, r.line, shape),
            contentAlignment = Alignment.Center) { Icon("sparkles", r.accentStrong) }
        Label(if (configured) "What can I help plan?" else "Connect a model", T.DETAIL_TITLE, r.ink, maxLines = 2)
        if (configured) {
            Label("Ask about your tasks, or describe changes to make. Every change waits for your approval.", T.BODY, r.ink2, maxLines = Int.MAX_VALUE)
            AiButton("What’s due this week?", { draft("What’s due this week?") }, icon = "calendar")
            AiButton("Summarize open tasks", { draft("Summarize my open tasks.") }, icon = "list")
            AiButton("Plan overdue tasks", { draft("Help me plan my overdue tasks.") }, icon = "sun")
            AiButton("Turn a note into tasks…", { draft("Turn this note into tasks: ") }, icon = "plus")
        } else {
            Label("Use an OpenAI-compatible model on your own private server. List access is off until you enable it.",
                T.BODY, r.ink2, maxLines = Int.MAX_VALUE)
            AiButton("AI settings", settings, primary = true, icon = "settings")
        }
    }
}

@Composable
private fun ChatTurn(turn: AssistantTurn, active: Boolean, left: Boolean, enabled: Boolean, model: AssistantModel?,
    onTask: (String) -> Unit, onSettings: () -> Unit, onRetry: (() -> Unit)?,
) {
    val r = LocalDesign.current.room
    val shape = RoundedCornerShape(Tokens.Radius.LG.dp)
    var expanded by rememberSaveable(turn.id) { mutableStateOf(false) }
    Column(Modifier.fillMaxWidth(), verticalArrangement = Arrangement.spacedBy(S.S2.dp)) {
        Label(turn.prompt, T.NOTES, r.ink,
            Modifier.align(Alignment.End).fillMaxWidth(0.88f).background(r.wash, shape)
                .border(Tokens.Elevation.HAIRLINE.dp, r.line, shape).padding(S.S3.dp), maxLines = Int.MAX_VALUE)
        if (turn.staleSnapshot) Label("Reading the last synced copy", T.META_PHONE, r.ink3)
        val summary = turn.stepSummary
        if (summary != null) AiButton(summary, { expanded = !expanded }, icon = if (expanded) "chevron-down" else "chevron-right", quiet = true)
        if (expanded || active) (if (active) turn.steps.takeLast(3) else turn.steps).forEach { step ->
            val icon = when {
                step.endsWith("…") -> "text" to r.accentStrong
                step.endsWith(" ms") -> "check" to r.success
                else -> "circle-alert" to r.ink3
            }
            Row(horizontalArrangement = Arrangement.spacedBy(S.S2.dp), verticalAlignment = Alignment.CenterVertically) {
                Icon(icon.first, icon.second, size = C.ICON_SMALL.dp)
                Label(step, T.META_PHONE, r.ink3, Modifier.weight(1f), maxLines = 2)
            }
        }
        if (turn.prose.isNotBlank()) Label(turn.prose, T.NOTES, r.ink, maxLines = Int.MAX_VALUE)
        turn.links.forEach { link -> AiButton(link.title, { onTask(link.id) }, icon = "arrow-up-right", modifier = Modifier.fillMaxWidth()) }
        turn.problem?.let {
            if (it == "the reply was stopped") Label("Reply stopped", T.META_PHONE, r.ink3)
            else AssistantProblem(it, onSettings, onRetry)
        }
        turn.proposal?.let { AssistantProposalCard(it, left, enabled, model, onTask) }
    }
}

@Composable
private fun ChatComposer(draft: String, onDraft: (String) -> Unit, configured: Boolean, streaming: Boolean, busy: Boolean,
    placeholder: String, onSend: () -> Unit, onStop: () -> Unit,
) {
    val r = LocalDesign.current.room
    val shape = RoundedCornerShape(Tokens.Radius.COMPOSER.dp)
    val canSend = configured && draft.isNotBlank() && !busy
    val actionable = if (streaming) !busy else canSend
    Row(Modifier.fillMaxWidth().hairline(r.line, top = true).padding(horizontal = S.S3.dp, vertical = S.S1.dp)
        .heightIn(min = P.COMPOSER.dp, max = P.AI_COMPOSER_MAX.dp).clip(shape).background(r.raised)
        .border(Tokens.Elevation.HAIRLINE.dp, r.control, shape), verticalAlignment = Alignment.Bottom) {
        BasicTextField(draft, onDraft, Modifier.weight(1f).heightIn(min = P.COMPOSER.dp)
            .semantics { contentDescription = "Assistant message" }, enabled = configured,
            textStyle = T.COMPOSER.style(r.ink), cursorBrush = SolidColor(r.accentStrong),
            keyboardOptions = KeyboardOptions(capitalization = KeyboardCapitalization.Sentences),
            decorationBox = { field ->
                Box(Modifier.heightIn(min = P.COMPOSER.dp).padding(horizontal = S.S3.dp, vertical = S.S2.dp),
                    contentAlignment = Alignment.CenterStart) {
                    if (draft.isEmpty()) Label(if (configured) placeholder else "Choose a model to start", T.COMPOSER, r.ink3)
                    field()
                }
            })
        Box(Modifier.size(P.TOUCH.dp).clickable(enabled = actionable, role = Role.Button,
            onClick = if (streaming) onStop else onSend)
            .semantics { contentDescription = if (streaming) "Stop reply" else "Send message" }, contentAlignment = Alignment.Center) {
            val button = RoundedCornerShape(Tokens.Radius.MD.dp)
            val primary = canSend && !streaming
            Box(Modifier.size(P.DETAIL_BREADCRUMB.dp).background(if (primary) r.accentFill else r.canvas, button)
                .border(Tokens.Elevation.HAIRLINE.dp, if (primary) r.accentFill else r.lineStrong, button),
                contentAlignment = Alignment.Center) {
                Icon(if (streaming) "square" else "arrow-up", if (primary) r.onAccent else if (streaming) r.ink else r.ink3)
            }
        }
    }
}

@Composable
internal fun AiButton(text: String, onClick: () -> Unit, modifier: Modifier = Modifier, icon: String? = null,
    enabled: Boolean = true, primary: Boolean = false, quiet: Boolean = false, maxLines: Int = 2,
) {
    val r = LocalDesign.current.room
    val shape = RoundedCornerShape(Tokens.Radius.MD.dp)
    val fill = if (enabled && primary) r.accentFill else if (quiet) r.pane else r.raised
    val ink = if (!enabled) r.ink3 else if (primary) r.onAccent else r.ink2
    Row(modifier.heightIn(min = P.TOUCH.dp).clip(shape).background(fill)
        .then(if (!quiet) Modifier.border(Tokens.Elevation.HAIRLINE.dp, if (primary && enabled) fill else r.lineStrong, shape) else Modifier)
        .clickable(enabled = enabled, role = Role.Button, onClick = onClick).padding(horizontal = S.S3.dp, vertical = S.S1.dp),
        horizontalArrangement = Arrangement.spacedBy(S.S2.dp, Alignment.CenterHorizontally), verticalAlignment = Alignment.CenterVertically) {
        icon?.let { Icon(it, if (primary || !enabled) ink else r.accentStrong, size = C.ICON_SMALL.dp) }
        Label(text, T.BUTTON, ink, maxLines = maxLines)
    }
}

@Composable
internal fun AssistantProblem(message: String, settings: (() -> Unit)? = null, retry: (() -> Unit)? = null) {
    val r = LocalDesign.current.room
    val shape = RoundedCornerShape(Tokens.Radius.LG.dp)
    Column(Modifier.fillMaxWidth().clip(shape).background(r.raised).border(Tokens.Elevation.HAIRLINE.dp, r.lineStrong, shape)
        .selectionBar(r.danger).padding(start = S.S4.dp, end = S.S3.dp, top = S.S2.dp, bottom = S.S2.dp)
        .semantics { liveRegion = LiveRegionMode.Polite }) {
        Label(message.replaceFirstChar { it.uppercaseChar() }, T.BODY, r.dangerInk, maxLines = Int.MAX_VALUE)
        if (settings != null || retry != null) Row(horizontalArrangement = Arrangement.spacedBy(S.S2.dp)) {
            retry?.let { AiButton("Retry", it, icon = "refresh-cw", quiet = true) }
            settings?.let { AiButton("AI settings", it, quiet = true) }
        }
    }
}

@Composable
internal fun rememberAssistantScroll() = rememberLazyListState()
