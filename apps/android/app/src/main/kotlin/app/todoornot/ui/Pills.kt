package app.todoornot.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.lazy.LazyRow
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.drawWithContent
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.role
import androidx.compose.ui.semantics.selected
import androidx.compose.ui.semantics.semantics
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

/** Collections see their peers, and lists see the lists belonging to the same parent. */
fun siblings(home: Home, key: RoomKey): List<NavItem> = when (key) {
    is RoomKey.Smart -> home.smart
    RoomKey.Trash -> emptyList()
    is RoomKey.Collection -> home.groups.mapNotNull { it.collection }
    is RoomKey.List -> home.groups.firstOrNull { group -> group.lists.any { it.key == key } }?.lists.orEmpty()
}

@Composable
internal fun RoomPillsStrip(home: Home, current: RoomKey, modifier: Modifier, onOpen: (RoomKey) -> Unit) {
    val rooms = siblings(home, current)
    val index = rooms.indexOfFirst { it.key == current }
    val state = rememberLazyListState()
    LaunchedEffect(index, rooms.map { it.key }) {
        if (index >= 0 && state.layoutInfo.visibleItemsInfo.none {
                it.index == index && it.offset >= state.layoutInfo.viewportStartOffset &&
                    it.offset + it.size <= state.layoutInfo.viewportEndOffset
            }
        ) state.scrollToItem(index)
    }
    val canvas = LocalDesign.current.room.canvas
    LazyRow(modifier.height(P.TOUCH.dp).drawWithContent {
        drawContent()
        if (state.canScrollForward) {
            val fadeStart = (size.width - S.S4.dp.toPx()).coerceAtLeast(0f)
            drawRect(Brush.horizontalGradient(listOf(canvas.copy(alpha = 0f), canvas), startX = fadeStart, endX = size.width),
                topLeft = Offset(fadeStart, 0f), size = Size(size.width - fadeStart, size.height))
        }
    }, state = state,
        horizontalArrangement = Arrangement.spacedBy(S.S2.dp), verticalAlignment = Alignment.CenterVertically) {
        items(rooms, key = { it.key.toString() }) { item -> RoomPill(item, item.key == current) { onOpen(item.key) } }
    }
}

@Composable
fun RoomPill(item: NavItem, on: Boolean, onClick: () -> Unit) {
    val d = LocalDesign.current
    val r = d.room
    val l = d.list(item.hue)
    val (probe, drop) = item.key.taskDrop()?.let { taskDragNav(it, inSwitcher = false) } ?: (Modifier to false)
    val ink = if (drop) r.ink else if (on) l.onStrong else l.ink
    val mark = if (drop) r.accentStrong else if (on) l.onStrong else l.glyph
    val shape = RoundedCornerShape(Tokens.Radius.ROOM_PILL.dp)
    TouchTarget(onClick, probe.semantics { selected = on; role = Role.Tab }) { press ->
        Row(
            Modifier.height(P.ROOM_PILL.dp).widthIn(max = Tokens.Metrics.Component.NEW_MENU.dp)
                .clip(shape).background(if (drop) r.selectionWash else if (on) l.strong else l.tint)
                .then(if (drop) Modifier.border(Tokens.Elevation.HAIRLINE.dp, r.accentStrong, shape) else Modifier)
                .then(press).padding(start = Micro.pillStart, end = S.S3.dp),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(Tokens.Metrics.Component.CHIP_GAP.dp),
        ) {
            when {
                item.glyph != null -> Glyph(item.glyph!!, mark)
                on && item.icon != null -> Icon(item.icon!!, mark, size = Tokens.Metrics.Component.GLYPH.dp)
                else -> Box(Modifier.size(Micro.pillDot).background(if (on || drop) mark else l.dot, CircleShape))
            }
            Label(item.name, Tokens.Type.ROOM_PILL, ink, weight = if (on) Tokens.Type.BUTTON.weight else null)
        }
    }
}
