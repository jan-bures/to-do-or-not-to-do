package app.todoornot.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicText
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.drawBehind
import androidx.compose.ui.geometry.CornerRadius
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextDecoration
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.LayoutDirection
import androidx.compose.ui.unit.dp
import app.todoornot.core.AgentMark
import app.todoornot.core.Chip
import app.todoornot.core.DueLabel
import app.todoornot.core.DueTone
import app.todoornot.core.StepCount
import app.todoornot.design.LocalDesign
import app.todoornot.design.TextToken
import app.todoornot.design.Tokens
import app.todoornot.design.style
import app.todoornot.design.text

private val C = Tokens.Metrics.Component

/**
 * Drawn details below the token scale: the step meter's bars, the eyebrow's divider, and the room
 * pill's leading inset. Everything else comes from [Tokens].
 */
internal object Micro {
    val meterBar = 3.dp
    val meterBarHeight = 10.dp
    val meterGap = 2.dp
    val meterRadius = 1.dp
    val meterBars = 5
    val eyebrowBar = 9.dp
    val pillStart = 10.dp
}

/** One line (by default) of text in a type token. */
@Composable
fun Label(
    text: String,
    token: TextToken,
    color: Color,
    modifier: Modifier = Modifier,
    maxLines: Int = 1,
    weight: Int? = null,
    decoration: TextDecoration? = null,
) {
    var style: TextStyle = token.style(color)
    if (weight != null) style = style.copy(fontWeight = FontWeight(weight))
    if (decoration != null) style = style.copy(textDecoration = decoration)
    BasicText(token.text(text), modifier, style = style, maxLines = maxLines, overflow = TextOverflow.Ellipsis, softWrap = maxLines > 1)
}

/** A 1 dp line along the bottom (or top) edge, in `line`. */
fun Modifier.hairline(color: Color, top: Boolean = false): Modifier = drawBehind {
    val w = Tokens.Elevation.HAIRLINE.dp.toPx()
    val y = if (top) w / 2 else size.height - w / 2
    drawLine(color, Offset(0f, y), Offset(size.width, y), w)
}

/** An inset leading mark with round ends; it never paints into a surface's corners. */
fun Modifier.selectionBar(color: Color): Modifier = drawBehind {
    val inset = Tokens.Space.S1.dp.toPx()
    val width = Tokens.Elevation.SELECTION_BAR.dp.toPx()
    val height = size.height - inset * 2
    if (height > 0 && size.width >= inset * 2 + width) {
        val x = if (layoutDirection == LayoutDirection.Ltr) inset else size.width - inset - width
        drawRoundRect(color, Offset(x, inset), Size(width, height), CornerRadius(width / 2))
    }
}

/** A list label on its tint, with equal padding on both sides. */
@Composable
fun ListChip(chip: Chip, modifier: Modifier = Modifier) {
    val l = LocalDesign.current.list(chip.hue)
    Row(
        modifier
            .heightIn(min = C.CHIP.dp)
            .background(l.tint, RoundedCornerShape(Tokens.Radius.CHIP.dp))
            .padding(horizontal = C.CHIP_PAD_X.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Label(chip.label, Tokens.Type.CHIP, l.ink)
    }
}

/** The overdue badge, e.g. "2d overdue" on the danger tint. */
@Composable
fun OverdueBadge(text: String, modifier: Modifier = Modifier) {
    val r = LocalDesign.current.room
    Row(
        modifier
            .heightIn(min = C.CHIP.dp)
            .background(r.dangerBg, RoundedCornerShape(Tokens.Radius.CHIP.dp))
            .padding(horizontal = C.CHIP_PAD_X.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Label(text, Tokens.Type.OVERDUE_BADGE, r.dangerInk)
    }
}

/** A tag is tinted mono text, not a pill or a status light. */
@Composable
fun TagLabel(label: String, hue: String) {
    Label(label, Tokens.Type.TAG, LocalDesign.current.list(hue).ink)
}

/** A due label: plain, soon (accent) or overdue (badge). */
@Composable
fun Due(due: DueLabel, modifier: Modifier = Modifier) {
    val r = LocalDesign.current.room
    when (due.tone) {
        DueTone.OVERDUE -> OverdueBadge(due.text, modifier)
        DueTone.SOON -> Label(due.text, Tokens.Type.DUE, r.accentStrong, modifier, weight = Tokens.Type.OVERDUE_BADGE.weight)
        DueTone.PLAIN -> Label(due.text, Tokens.Type.DUE, r.ink2, modifier)
    }
}

/** A round letter mark for an agent (C, X, π), in its own colours. */
@Composable
fun AgentBadge(agent: AgentMark, modifier: Modifier = Modifier, size: Dp = C.AGENT_MARK.dp) {
    val a = LocalDesign.current.agent(agent.hue, agent.known)
    Box(
        modifier
            .size(size)
            .background(a.bg, CircleShape)
            .border(Tokens.Elevation.HAIRLINE.dp, a.ring, CircleShape),
        contentAlignment = Alignment.Center,
    ) {
        Label(agent.letter, Tokens.Type.AGENT_LETTER, a.ink)
    }
}

/** `!`, `!!` or `!!!`: neutral, darker with each mark. */
@Composable
fun Priority(marks: Int, modifier: Modifier = Modifier, quietInk: Color? = null) {
    if (marks <= 0) return
    val r = LocalDesign.current.room
    val color = when (marks) {
        1 -> quietInk ?: r.ink3
        2 -> r.ink2
        else -> r.ink
    }
    Label("!".repeat(marks.coerceAtMost(3)), Tokens.Type.PRIORITY, color, modifier)
}

/** The step meter: up to five bars, filled in proportion, then `2/5`. */
@Composable
fun StepMeter(steps: StepCount, modifier: Modifier = Modifier, showCount: Boolean = true, quietInk: Color? = null) {
    val r = LocalDesign.current.room
    val total = steps.total.toInt()
    val done = steps.done.toInt()
    val bars = total.coerceIn(1, Micro.meterBars)
    val on = if (total == 0) 0 else (done * bars + total - 1) / total
    Row(modifier, verticalAlignment = Alignment.CenterVertically) {
        Row(horizontalArrangement = Arrangement.spacedBy(Micro.meterGap)) {
            repeat(bars) { i ->
                Box(
                    Modifier
                        .size(Micro.meterBar, Micro.meterBarHeight)
                        .background(if (i < on) r.accent else r.lineStrong, RoundedCornerShape(Micro.meterRadius)),
                )
            }
        }
        if (showCount) {
            Spacer(Modifier.width(C.META_GAP.dp))
            Label("$done/$total", Tokens.Type.COUNT, quietInk ?: r.ink3)
        }
    }
}

/** Padding that matches the design's `top end bottom start` order. */
fun pad(top: Dp = 0.dp, end: Dp = 0.dp, bottom: Dp = 0.dp, start: Dp = 0.dp) = PaddingValues(start, top, end, bottom)
