package app.todoornot.ui

import android.provider.Settings
import androidx.activity.compose.BackHandler
import androidx.compose.animation.AnimatedContent
import androidx.compose.animation.togetherWith
import androidx.compose.animation.EnterTransition
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.platform.LocalContext
import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.foundation.gestures.Orientation
import androidx.compose.foundation.gestures.awaitEachGesture
import androidx.compose.foundation.gestures.awaitFirstDown
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.foundation.gestures.draggable
import androidx.compose.foundation.gestures.rememberDraggableState
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.isImeVisible
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.setValue
import androidx.compose.runtime.staticCompositionLocalOf
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.unit.Dp
import androidx.compose.animation.core.MutableTransitionState
import androidx.compose.animation.core.tween
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.scaleIn
import androidx.compose.animation.scaleOut
import androidx.compose.animation.slideInVertically
import androidx.compose.animation.slideOutVertically
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ColumnScope
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.systemBarsPadding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.SideEffect
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.input.key.Key
import androidx.compose.ui.input.key.KeyEventType
import androidx.compose.ui.input.key.key
import androidx.compose.ui.input.key.onPreviewKeyEvent
import androidx.compose.ui.input.key.type
import androidx.compose.ui.graphics.TransformOrigin
import androidx.compose.ui.graphics.shadow.Shadow
import androidx.compose.ui.draw.dropShadow
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.platform.LocalView
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.paneTitle
import androidx.compose.ui.semantics.role
import androidx.compose.ui.semantics.selected
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.DpOffset
import androidx.compose.ui.unit.IntOffset
import androidx.compose.ui.unit.IntRect
import androidx.compose.ui.unit.IntSize
import androidx.compose.ui.unit.LayoutDirection
import androidx.compose.ui.unit.dp
import androidx.compose.ui.window.Dialog
import androidx.compose.ui.window.DialogProperties
import androidx.compose.ui.window.DialogWindowProvider
import androidx.compose.ui.window.SecureFlagPolicy
import androidx.compose.ui.window.Popup
import androidx.compose.ui.window.PopupPositionProvider
import androidx.compose.ui.window.PopupProperties
import app.todoornot.design.Icon
import app.todoornot.design.LocalDesign
import app.todoornot.design.Tokens
import app.todoornot.design.cubic
import app.todoornot.model.MenuAction
import app.todoornot.model.MenuEntry

private val S = Tokens.Space
private val M = Tokens.Motion
private val C = Tokens.Metrics.Component

/**
 * The one shadow (`docs/design-language.md`, Elevation): the token's two layers, for transient overlays
 * only. In dark mode it comes with a 1 dp `line-strong` border.
 */
fun Modifier.overlay(shape: RoundedCornerShape, design: app.todoornot.design.Design): Modifier {
    var m = this
    for (l in design.overlayShadow) {
        m = m.dropShadow(shape, Shadow(radius = l.blur.dp, color = l.color, spread = l.spread.dp, offset = DpOffset(0.dp, l.y.dp)))
    }
    m = m.clip(shape).background(design.room.raised)
    if (design.dark) m = m.border(Tokens.Elevation.HAIRLINE.dp, design.room.lineStrong, shape)
    return m
}

/**
 * A menu's card: the overlay surface, as wide as the desktop's menus, with [entries] drawn as items and
 * separators. [onChoose] gets the chosen item's action. The gallery draws it on its own.
 */
@Composable
fun MenuCard(entries: List<MenuEntry>, onChoose: (MenuAction) -> Unit, modifier: Modifier = Modifier, title: String? = null) {
    val d = LocalDesign.current
    val shape = RoundedCornerShape(Tokens.Radius.LG.dp)
    Column(
        modifier
            .width(Tokens.Metrics.Desktop.MENU.dp)
            .overlay(shape, d)
            .verticalScroll(rememberScrollState())
            .padding(vertical = S.S1.dp)
            .semantics { if (title != null) paneTitle = title },
    ) {
        entries.forEach { e ->
            when (e) {
                is MenuEntry.Item -> MenuItemRow(e) { onChoose(e.action) }
                MenuEntry.Separator -> Box(
                    Modifier.padding(vertical = S.S1.dp).fillMaxWidth().height(Tokens.Elevation.HAIRLINE.dp).background(d.room.line),
                )
            }
        }
    }
}

/** One item: icon, label, an optional hint (`!!!`) and a tick for the current choice. 48 dp to touch. */
@Composable
internal fun MenuItemRow(item: MenuEntry.Item, modifier: Modifier = Modifier, onClick: () -> Unit) {
    val r = LocalDesign.current.room
    val ink = when {
        !item.enabled -> r.ink4
        item.danger -> r.dangerInk
        item.checked -> r.accentStrong
        else -> r.ink
    }
    Row(
        modifier.fillMaxWidth()
            .height(Tokens.Metrics.Phone.TOUCH.dp)
            .then(if (item.checked) Modifier.background(r.wash) else Modifier)
            .clickable(enabled = item.enabled, onClick = onClick)
            .semantics {
                role = Role.Button
                if (item.checked) selected = true
            }
            .padding(horizontal = S.S3.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(S.S3.dp),
    ) {
        val iconInk = when {
            !item.enabled -> r.ink4
            item.danger -> r.danger
            else -> r.taskToggleFill(item.icon) ?: r.ink3
        }
        Icon(item.icon, iconInk, size = C.ICON_SMALL.dp)
        Label(item.label, Tokens.Type.NAV_SUB, ink, Modifier.weight(1f),
            weight = if (item.checked) Tokens.Type.BUTTON.weight else null)
        item.hint?.let { Label(it, Tokens.Type.PRIORITY, r.ink3) }
        Box(Modifier.size(C.ICON_SMALL.dp)) {
            if (item.checked) Icon("check", r.accentStrong, size = C.ICON_SMALL.dp)
        }
    }
}

/**
 * Whether an overlay is up, and how it leaves: [close] plays the exit (`OVERLAY_OUT`), then calls
 * `onGone`. Nothing waits for it: the chosen action has already run. Choosing is one-shot: once the
 * overlay is leaving, [choose] does nothing, so a second tap during the exit (a double tap on Delete step)
 * never runs the action again.
 */
class OverlayState internal constructor(internal val shown: MutableTransitionState<Boolean>) {
    /** Up, and not leaving. */
    val open: Boolean get() = shown.targetState

    fun close() {
        shown.targetState = false
    }

    /** Runs [action] and closes, once: nothing while the overlay is already leaving. */
    fun choose(action: () -> Unit) {
        if (!open) return
        close()
        action()
    }
}

@Composable
private fun rememberOverlay(onGone: () -> Unit, atRest: Boolean = false): OverlayState {
    val state = remember { OverlayState(MutableTransitionState(atRest).apply { targetState = true }) }
    val s = state.shown
    LaunchedEffect(s.currentState, s.targetState, s.isIdle) {
        if (!s.targetState && !s.currentState && s.isIdle) onGone()
    }
    return state
}

/**
 * Places a menu under its anchor (over it, when there is no room below), lined up with the anchor's end,
 * or its start for [start], and kept [margin] px inside the window. The card's own padding (room for the
 * shadow) is [inset] px.
 */
private class MenuPosition(val start: Boolean, val margin: Int, val inset: Int, val above: Int? = null) : PopupPositionProvider {
    override fun calculatePosition(anchorBounds: IntRect, windowSize: IntSize, layoutDirection: LayoutDirection, popupContentSize: IntSize): IntOffset {
        val w = popupContentSize.width
        val h = popupContentSize.height
        val x = if (start) anchorBounds.left - inset else anchorBounds.right - w + inset
        val below = anchorBounds.bottom - inset
        val y = if (above != null) anchorBounds.top - h + inset - above
            else if (below + h <= windowSize.height - margin) below else anchorBounds.top - h + inset
        return IntOffset(
            x.coerceIn(margin - inset, maxOf(margin - inset, windowSize.width - w - margin + inset)),
            y.coerceIn(margin - inset, maxOf(margin - inset, windowSize.height - h - margin + inset)),
        )
    }
}

/**
 * A menu anchored to its parent (put it in the same `Box` as its button or row): it opens under the anchor,
 * fading in and growing from 0.97 over `OVERLAY_IN` (`ENTER`) while it moves its last 4 dp away from the
 * anchor, and fades out over `OVERLAY_OUT` (`EXIT`). Choosing an item runs it at once, then closes the
 * menu. [onDismiss] is called once it has gone. It lines up with the anchor's end, or its [start].
 */
@Composable
fun MenuPopup(
    entries: List<MenuEntry>,
    onChoose: (MenuAction) -> Unit,
    onDismiss: () -> Unit,
    start: Boolean = false,
    title: String? = null,
    above: Boolean = false,
) = AnchoredPopup(onDismiss, start, above) { overlay ->
    MenuCard(entries, { action -> overlay.choose { onChoose(action) } }, Modifier.padding(S.S2.dp), title)
}

/** Shared anchor, placement and motion for menus and the calendar popover. */
@Composable
fun AnchoredPopup(onDismiss: () -> Unit, start: Boolean = false, above: Boolean = false, content: @Composable (OverlayState) -> Unit) {
    val overlay = rememberOverlay(onDismiss)
    val density = LocalDensity.current
    val shift = with(density) { M.OVERLAY_SHIFT.dp.roundToPx() }
    val position = remember(start, density, above) {
        with(density) { MenuPosition(start, S.S2.dp.roundToPx(), S.S2.dp.roundToPx(), if (above) S.S1.dp.roundToPx() else null) }
    }
    val origin = if (start) 0f else 1f
    Popup(
        popupPositionProvider = position,
        onDismissRequest = overlay::close,
        properties = PopupProperties(focusable = true),
    ) {
        AnimatedVisibility(
            overlay.shown,
            enter = fadeIn(tween(M.OVERLAY_IN, easing = M.EASE_ENTER.cubic())) +
                scaleIn(tween(M.OVERLAY_IN, easing = M.EASE_ENTER.cubic()), initialScale = M.OVERLAY_FROM_SCALE, transformOrigin = TransformOrigin(origin, 0f)) +
                slideInVertically(tween(M.OVERLAY_IN, easing = M.EASE_ENTER.cubic())) { -shift },
            exit = fadeOut(tween(M.OVERLAY_OUT, easing = M.EASE_EXIT.cubic())) +
                scaleOut(tween(M.OVERLAY_OUT, easing = M.EASE_EXIT.cubic()), targetScale = M.OVERLAY_TO_SCALE, transformOrigin = TransformOrigin(origin, 0f)),
        ) {
            content(overlay)
        }
    }
}

private val Phone = Tokens.Metrics.Phone

/** The left pane's live width (including a separating fold's actual position, when present). */
internal val LocalSheetWidth = staticCompositionLocalOf<Dp?> { null }
internal val LocalSheetAtEnd = staticCompositionLocalOf { false }
/** Freeze the initial sheet motion at rest for visual fixtures. */
internal val LocalSheetAtRest = staticCompositionLocalOf { false }

@Composable
internal fun reducedMotion(): Boolean {
    val context = LocalContext.current
    return Settings.Global.getFloat(context.contentResolver, Settings.Global.ANIMATOR_DURATION_SCALE, 1f) == 0f
}

/** Full-screen layer in the activity, so a task drag can stay captured while the switcher opens. */
@Composable
internal fun BottomSheet(
    title: String,
    onDismiss: () -> Unit,
    tall: Boolean = false,
    color: Color = LocalDesign.current.room.raised,
    gestureReveal: Float? = null,
    content: @Composable ColumnScope.(OverlayState) -> Unit,
) {
    val overlay = rememberOverlay(onDismiss, atRest = LocalSheetAtRest.current)
    val d = LocalDesign.current
    val width = LocalSheetWidth.current
    val fromGesture = remember { gestureReveal != null }
    val reduced = reducedMotion()
    val keyboard = WindowInsets.isImeVisible
    val duration = if (reduced) 0 else M.PANEL
    val anchor = if (LocalSheetAtEnd.current) Alignment.BottomEnd else Alignment.BottomStart
    BackHandler(onBack = overlay::close)
    Box(Modifier.fillMaxSize(), contentAlignment = anchor) {
        AnimatedVisibility(overlay.shown, enter = fadeIn(tween(duration)), exit = fadeOut(tween(duration))) {
            Box(Modifier.fillMaxSize().graphicsLayer { alpha = if (gestureReveal == null) 1f else (gestureReveal / Phone.HEADER_REACH_EXPAND.dp.toPx()).coerceIn(0f, 1f) }
                    .background(d.scrim).clickable(
                remember { MutableInteractionSource() }, indication = null, onClickLabel = "Dismiss $title", onClick = overlay::close,
            ))
        }
        BoxWithConstraints(Modifier.fillMaxSize().statusBarsPadding().imePadding(), contentAlignment = anchor) {
            val viewport = maxHeight
            // Fit the pinned actions and search results in the reduced space above the IME.
            val base = if (keyboard) viewport else viewport * Phone.SHEET_DEFAULT_FRACTION
            val max = if (keyboard) viewport else viewport * Phone.SHEET_MAX_FRACTION
            var expanded by remember { androidx.compose.runtime.mutableStateOf(false) }
            var drag by remember(viewport) { mutableFloatStateOf(0f) }
            var dragging by remember { androidx.compose.runtime.mutableStateOf(false) }
            val density = LocalDensity.current
            val roomToGrow = with(density) { (max - base).toPx() }
            val rest = if (!overlay.open && drag < 0f) drag else if (expanded) roomToGrow else 0f
            val position by animateFloatAsState(if (dragging) drag else rest,
                if (dragging) androidx.compose.animation.core.snap() else tween(duration, easing = M.EASE_STANDARD.cubic()),
                label = "sheet settle")
            val basePx = with(density) { (if (tall) base else max).toPx() }
            val revealOffset by animateFloatAsState(if (gestureReveal == null) 0f else (basePx - gestureReveal).coerceAtLeast(0f),
                if (gestureReveal != null) androidx.compose.animation.core.snap() else tween(duration, easing = M.EASE_ENTER.cubic()),
                label = "switcher reveal")
            val shape = RoundedCornerShape(topStart = Tokens.Radius.XL.dp, topEnd = Tokens.Radius.XL.dp)
            AnimatedVisibility(
                overlay.shown,
                modifier = if (width == null) Modifier.fillMaxWidth() else Modifier.width(width),
                enter = if (fromGesture) EnterTransition.None else slideInVertically(tween(duration, easing = M.EASE_ENTER.cubic())) { it },
                exit = slideOutVertically(tween(duration, easing = M.EASE_EXIT.cubic())) { it },
            ) {
                val delta = with(density) { position.toDp() }
                Column(Modifier.fillMaxWidth()
                    .offset { IntOffset(0, ((-position).coerceAtLeast(0f) + revealOffset).toInt()) }
                    .then(if (tall) Modifier.height((base + delta.coerceAtLeast(0.dp)).coerceAtMost(max)) else Modifier.heightIn(max = max))
                    .clip(shape).background(color)
                    .then(if (d.dark) Modifier.border(Tokens.Elevation.HAIRLINE.dp, d.room.lineStrong, shape) else Modifier)
                    .pointerInput(Unit) { // Keep taps on blank sheet space from reaching the scrim.
                        awaitEachGesture { awaitFirstDown(requireUnconsumed = false) }
                    }
                    .semantics { paneTitle = title }
                    .onPreviewKeyEvent {
                        if (it.key != Key.Escape) false else {
                            if (it.type == KeyEventType.KeyUp) overlay.close()
                            true
                        }
                    }
                    .navigationBarsPadding(),
                ) {
                    Box(Modifier.fillMaxWidth().draggable(
                        rememberDraggableState { dy -> drag = (drag - dy).coerceAtMost(if (tall) roomToGrow else 0f) },
                        orientation = Orientation.Vertical,
                        onDragStarted = { drag = position; dragging = true },
                        onDragStopped = {
                            if (drag < -with(density) { Phone.BULK_STATUS_BAR.dp.toPx() }) overlay.close()
                            else if (tall) expanded = drag >= roomToGrow / 2
                            dragging = false
                        },
                    ).padding(vertical = S.S2.dp), contentAlignment = Alignment.Center) {
                        Box(Modifier.size(Phone.SHEET_HANDLE_WIDTH.dp, S.S1.dp)
                            .background(d.room.lineStrong, RoundedCornerShape(Tokens.Radius.SM.dp)))
                    }
                    content(overlay)
                }
            }
        }
    }
}

@Composable
internal fun <T> SheetPages(page: T, content: @Composable (T) -> Unit) {
    AnimatedContent(page, modifier = Modifier.fillMaxWidth(), transitionSpec = {
        fadeIn(tween(M.CONTENT_SWAP)) togetherWith fadeOut(tween(M.CONTENT_SWAP))
    }, label = "sheet page") { content(it) }
}

@Composable
internal fun ActionSheetContent(entries: List<MenuEntry>, title: String, onChoose: (MenuAction) -> Unit) {
    val r = LocalDesign.current.room
    val first = entries.indexOfFirst { it is MenuEntry.Item && it.enabled }
    val target = remember { FocusRequester() }
    if (first >= 0 && !LocalSheetAtRest.current) LaunchedEffect(title) { target.requestFocus() }
    Column(Modifier.fillMaxWidth().verticalScroll(rememberScrollState()).padding(bottom = S.S2.dp)) {
        Label(title, Tokens.Type.CAPS_LABEL, r.ink3, Modifier.padding(horizontal = S.S3.dp, vertical = S.S2.dp))
        entries.forEachIndexed { index, entry ->
            when (entry) {
                is MenuEntry.Item -> MenuItemRow(entry, if (index == first) Modifier.focusRequester(target) else Modifier) { onChoose(entry.action) }
                MenuEntry.Separator -> Box(Modifier.fillMaxWidth().height(Tokens.Elevation.HAIRLINE.dp).background(r.line))
            }
        }
    }
}

/**
 * The scrim and the bottom-aligned card of a modal dialog, at rest. The dialog animates both; the gallery draws
 * this as it is.
 */
@Composable
fun DialogBackdrop(modifier: Modifier = Modifier, content: @Composable () -> Unit) {
    val d = LocalDesign.current
    Box(modifier.fillMaxSize().background(d.scrim).systemBarsPadding().imePadding().padding(Tokens.Metrics.Phone.DIALOG_INSET.dp), contentAlignment = Alignment.BottomCenter) {
        content()
    }
}

@Composable
internal fun PanelCard(title: String, inSheet: Boolean, modifier: Modifier = Modifier, content: @Composable ColumnScope.() -> Unit) {
    if (inSheet) Column(modifier.fillMaxWidth().padding(S.S4.dp).semantics { paneTitle = title },
        verticalArrangement = Arrangement.spacedBy(S.S3.dp), content = content)
    else DialogCard(title, modifier, content)
}

/** A dialog's card: the opaque pane, as wide as the inner screen's list at most. */
@Composable
fun DialogCard(title: String, modifier: Modifier = Modifier, content: @Composable ColumnScope.() -> Unit) {
    val r = LocalDesign.current.room
    Column(
        modifier
            .widthIn(max = Tokens.Metrics.Phone.INNER_LIST.dp)
            .overlay(RoundedCornerShape(Tokens.Radius.XL.dp), LocalDesign.current)
            .background(r.pane)
            .padding(S.S5.dp)
            .semantics { paneTitle = title },
        verticalArrangement = Arrangement.spacedBy(S.S4.dp),
        content = content,
    )
}

/**
 * A modal dialog (`docs/design-language.md`, Motion, 12): the scrim (the token's, in the room's hue, never
 * blurred) fades in over `OVERLAY_IN` while the card fades in, grows from 0.97 and rises 6 dp; leaving,
 * both fade over `OVERLAY_OUT` as the card drops 4 dp. Back or a tap on the scrim cancels.
 * [content] gets the dialog's [OverlayState]: `close` plays the exit and then calls [onDismiss], and
 * `choose` runs a button's action only once.
 */
@Composable
fun OverlayDialog(
    onDismiss: () -> Unit,
    securePolicy: SecureFlagPolicy = SecureFlagPolicy.Inherit,
    paneWidth: Dp? = LocalSheetWidth.current,
    atEnd: Boolean = LocalSheetAtEnd.current,
    content: @Composable (OverlayState) -> Unit,
) {
    val overlay = rememberOverlay(onDismiss)
    val density = LocalDensity.current
    val rise = with(density) { M.DIALOG_RISE.dp.roundToPx() }
    val drop = with(density) { M.DIALOG_DROP.dp.roundToPx() }
    Dialog(
        onDismissRequest = overlay::close,
        properties = DialogProperties(usePlatformDefaultWidth = false, decorFitsSystemWindows = false, securePolicy = securePolicy),
    ) {
        // The scrim is ours (the token's colour, fading with the card), not the window's dim.
        val window = (LocalView.current.parent as? DialogWindowProvider)?.window
        SideEffect { window?.setDimAmount(0f) }
        val d = LocalDesign.current
        Box(Modifier.fillMaxSize()) {
            AnimatedVisibility(
                overlay.shown,
                enter = fadeIn(tween(M.OVERLAY_IN, easing = M.EASE_ENTER.cubic())),
                exit = fadeOut(tween(M.OVERLAY_OUT, easing = M.EASE_EXIT.cubic())),
            ) {
                Box(
                    Modifier
                        .fillMaxSize()
                        .background(d.scrim)
                        .clickable(remember { MutableInteractionSource() }, indication = null, onClick = overlay::close),
                )
            }
            Box(Modifier.fillMaxSize().systemBarsPadding().imePadding(), contentAlignment = if (atEnd) Alignment.BottomEnd else Alignment.BottomStart) {
                Box((paneWidth?.let { Modifier.width(it) } ?: Modifier.fillMaxWidth()).padding(Phone.DIALOG_INSET.dp),
                    contentAlignment = Alignment.BottomCenter,
                ) {
                    AnimatedVisibility(
                        overlay.shown,
                        enter = fadeIn(tween(M.OVERLAY_IN, easing = M.EASE_ENTER.cubic())) +
                            scaleIn(tween(M.OVERLAY_IN, easing = M.EASE_ENTER.cubic()), initialScale = M.OVERLAY_FROM_SCALE) +
                            slideInVertically(tween(M.OVERLAY_IN, easing = M.EASE_ENTER.cubic())) { rise },
                        exit = fadeOut(tween(M.OVERLAY_OUT, easing = M.EASE_EXIT.cubic())) +
                            slideOutVertically(tween(M.OVERLAY_OUT, easing = M.EASE_EXIT.cubic())) { drop },
                    ) { content(overlay) }
                }
            }
        }
    }
}
