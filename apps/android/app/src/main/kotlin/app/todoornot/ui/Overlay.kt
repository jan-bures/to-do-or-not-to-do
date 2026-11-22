package app.todoornot.ui

import androidx.compose.animation.AnimatedVisibility
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
private fun MenuItemRow(item: MenuEntry.Item, onClick: () -> Unit) {
    val r = LocalDesign.current.room
    val ink = when {
        !item.enabled -> r.ink4
        item.danger -> r.dangerInk
        item.checked -> r.accentStrong
        else -> r.ink
    }
    Row(
        Modifier
            .fillMaxWidth()
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
        Icon(item.icon, if (!item.enabled) r.ink4 else if (item.danger) r.danger else r.ink3, size = C.ICON_SMALL.dp)
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
private fun rememberOverlay(onGone: () -> Unit): OverlayState {
    val state = remember { OverlayState(MutableTransitionState(false).apply { targetState = true }) }
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
private class MenuPosition(val start: Boolean, val margin: Int, val inset: Int) : PopupPositionProvider {
    override fun calculatePosition(anchorBounds: IntRect, windowSize: IntSize, layoutDirection: LayoutDirection, popupContentSize: IntSize): IntOffset {
        val w = popupContentSize.width
        val h = popupContentSize.height
        val x = if (start) anchorBounds.left - inset else anchorBounds.right - w + inset
        val below = anchorBounds.bottom - inset
        val y = if (below + h <= windowSize.height - margin) below else anchorBounds.top - h + inset
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
) {
    val overlay = rememberOverlay(onDismiss)
    val density = LocalDensity.current
    val shift = with(density) { M.OVERLAY_SHIFT.dp.roundToPx() }
    val position = remember(start, density) {
        with(density) { MenuPosition(start, S.S2.dp.roundToPx(), S.S2.dp.roundToPx()) }
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
            // Room for the shadow around the card.
            MenuCard(entries, { action -> overlay.choose { onChoose(action) } }, Modifier.padding(S.S2.dp), title)
        }
    }
}

/**
 * The scrim and the centred card of a modal dialog, at rest. The dialog animates both; the gallery draws
 * this as it is.
 */
@Composable
fun DialogBackdrop(modifier: Modifier = Modifier, content: @Composable () -> Unit) {
    val d = LocalDesign.current
    Box(modifier.fillMaxSize().background(d.scrim).systemBarsPadding().padding(S.S4.dp), contentAlignment = Alignment.Center) {
        content()
    }
}

/** A dialog's card: the opaque pane, as wide as the inner screen's list at most. */
@Composable
fun DialogCard(title: String, modifier: Modifier = Modifier, content: @Composable ColumnScope.() -> Unit) {
    val r = LocalDesign.current.room
    Column(
        modifier
            .widthIn(max = Tokens.Metrics.Phone.INNER_LIST.dp)
            .background(r.pane, RoundedCornerShape(Tokens.Radius.LG.dp))
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
fun OverlayDialog(onDismiss: () -> Unit, content: @Composable (OverlayState) -> Unit) {
    val overlay = rememberOverlay(onDismiss)
    val density = LocalDensity.current
    val rise = with(density) { M.DIALOG_RISE.dp.roundToPx() }
    val drop = with(density) { M.DIALOG_DROP.dp.roundToPx() }
    Dialog(
        onDismissRequest = overlay::close,
        properties = DialogProperties(usePlatformDefaultWidth = false, decorFitsSystemWindows = false),
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
            Box(Modifier.fillMaxSize().systemBarsPadding().padding(S.S4.dp), contentAlignment = Alignment.Center) {
                AnimatedVisibility(
                    overlay.shown,
                    enter = fadeIn(tween(M.OVERLAY_IN, easing = M.EASE_ENTER.cubic())) +
                        scaleIn(tween(M.OVERLAY_IN, easing = M.EASE_ENTER.cubic()), initialScale = M.OVERLAY_FROM_SCALE) +
                        slideInVertically(tween(M.OVERLAY_IN, easing = M.EASE_ENTER.cubic())) { rise },
                    exit = fadeOut(tween(M.OVERLAY_OUT, easing = M.EASE_EXIT.cubic())) +
                        slideOutVertically(tween(M.OVERLAY_OUT, easing = M.EASE_EXIT.cubic())) { drop },
                ) {
                    content(overlay)
                }
            }
        }
    }
}
