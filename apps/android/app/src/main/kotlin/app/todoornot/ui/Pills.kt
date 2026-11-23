package app.todoornot.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.layout.SubcomposeLayout
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.role
import androidx.compose.ui.semantics.selected
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.Constraints
import androidx.compose.ui.unit.dp
import app.todoornot.core.Home
import app.todoornot.core.NavItem
import app.todoornot.core.RoomKey
import app.todoornot.design.Glyph
import app.todoornot.design.Icon
import app.todoornot.design.LocalDesign
import app.todoornot.design.Tokens

private val P = Tokens.Metrics.Phone
private val S = Tokens.Space

/**
 * The rooms next to this one: the smart lists from a smart list, the collections from a collection, and
 * the collection's lists from a list.
 */
fun siblings(home: Home, key: RoomKey): List<NavItem> = when (key) {
    is RoomKey.Smart -> home.smart
    RoomKey.Trash -> emptyList()
    is RoomKey.Collection -> home.groups.mapNotNull { it.collection }
    is RoomKey.List -> home.groups.firstOrNull { g -> g.lists.any { it.key == key } }?.lists ?: emptyList()
}

/**
 * The room pills under the header: whole pills only, then a `+3` button for the rest (it opens the
 * drawer). The current room's pill is always among the visible ones.
 */
@Composable
fun RoomPills(home: Home, current: RoomKey, onOpen: (RoomKey) -> Unit, onMore: () -> Unit) {
    val r = LocalDesign.current.room
    val items = siblings(home, current)
    val gap = S.S2.dp
    SubcomposeLayout(
        Modifier
            .fillMaxWidth()
            .height(P.ROOM_ROW.dp)
            .hairline(r.line, top = true)
            .padding(horizontal = S.S5.dp),
    ) { constraints ->
        val loose = Constraints(maxHeight = constraints.maxHeight)
        val pills = items.map { item ->
            subcompose(item.key) { RoomPill(item, item.key == current) { onOpen(item.key) } }.first().measure(loose)
        }
        val gapPx = gap.roundToPx()
        // The overflow button at its widest, to decide how many pills fit (measured, never placed).
        val moreMax = if (items.size > 1) subcompose("more-probe") { MorePill(items.size, {}) }.first().measure(loose).width else 0
        val width = constraints.maxWidth
        fun fits(order: List<Int>, more: Int): Boolean {
            val used = order.sumOf { pills[it].width } + gapPx * (order.size - 1).coerceAtLeast(0)
            return used + (if (more > 0) gapPx + moreMax else 0) <= width
        }
        // Natural order; if the current pill doesn't fit, it takes the last visible slot.
        var shown = items.indices.toList()
        while (shown.isNotEmpty() && !fits(shown, items.size - shown.size)) shown = shown.dropLast(1)
        val cur = items.indexOfFirst { it.key == current }
        if (cur >= 0 && cur !in shown && shown.isNotEmpty()) shown = shown.dropLast(1) + cur
        val hidden = items.size - shown.size
        val more = if (hidden > 0) subcompose("more") { MorePill(hidden, onMore) }.first().measure(loose) else null
        val h = constraints.maxHeight
        layout(width, h) {
            var x = 0
            for (i in shown) {
                val p = pills[i]
                p.place(x, (h - p.height) / 2)
                x += p.width + gapPx
            }
            more?.place(x, (h - more.height) / 2)
        }
    }
}

@Composable
fun RoomPill(item: NavItem, on: Boolean, onClick: () -> Unit) {
    val l = LocalDesign.current.list(item.hue)
    val ink = if (on) l.onStrong else l.ink
    val mark = if (on) l.onStrong else l.glyph
    TouchTarget(onClick, Modifier.semantics { selected = on; role = Role.Tab }) { press ->
        Row(
            Modifier
                .height(P.ROOM_PILL.dp)
                .clip(RoundedCornerShape(Tokens.Radius.ROOM_PILL.dp))
                .background(if (on) l.strong else l.tint)
                .then(press)
                .padding(start = Micro.pillStart, end = S.S3.dp),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(Tokens.Metrics.Component.CHIP_GAP.dp),
        ) {
            when {
                item.glyph != null -> Glyph(item.glyph!!, mark)
                on && item.icon != null -> Icon(item.icon!!, mark, size = Tokens.Metrics.Component.GLYPH.dp)
                else -> Box(Modifier.size(Micro.pillDot).background(if (on) l.onStrong else l.dot, CircleShape))
            }
            Label(item.name, Tokens.Type.ROOM_PILL, ink, weight = if (on) Tokens.Type.BUTTON.weight else null)
        }
    }
}

@Composable
private fun MorePill(n: Int, onClick: () -> Unit) {
    val r = LocalDesign.current.room
    val shape = RoundedCornerShape(Tokens.Radius.ROOM_PILL.dp)
    TouchTarget(onClick, Modifier.semantics { contentDescription = "$n more rooms" }) { press ->
        Row(
            Modifier
                .height(P.ROOM_PILL.dp)
                .clip(shape)
                .background(r.raised)
                .border(Tokens.Elevation.HAIRLINE.dp, r.lineStrong, shape)
                .then(press)
                .padding(start = S.S3.dp, end = S.S2.dp),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(S.S1.dp),
        ) {
            Label("+$n", Tokens.Type.ROOM_PILL.copy(role = app.todoornot.design.FontRole.MONO, weight = Tokens.Type.BUTTON.weight), r.ink2)
            Icon("chevron-down", r.ink2, size = Tokens.Metrics.Component.ICON_SMALL.dp)
        }
    }
}
