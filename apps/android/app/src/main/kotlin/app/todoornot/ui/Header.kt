package app.todoornot.ui

import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.core.tween
import androidx.compose.animation.expandVertically
import androidx.compose.animation.shrinkVertically
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.RowScope
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.IntSize
import androidx.compose.ui.unit.dp
import app.todoornot.core.Eyebrow
import app.todoornot.core.RoomView
import app.todoornot.design.Landscape
import app.todoornot.design.LocalDesign
import app.todoornot.design.Scene
import app.todoornot.design.TodoTheme
import app.todoornot.design.Tokens
import app.todoornot.design.cubic

private val S = Tokens.Space

/**
 * A room's header: the sky in the room's hue with the bar, the eyebrow, the title and the count, and the
 * landscape strip with the sun at done / total. When the day is finished the sky turns to dusk, and the
 * header is drawn in the dark palette so the text stays readable on it.
 *
 * The bar (menu, [actions]) stays unless [bar] is false (typing in a short window). Everything under it
 * collapses with [collapse], and the room's name fades into the bar as it does.
 */
@Composable
fun RoomHeader(
    room: RoomView,
    landscape: Dp,
    collapse: HeaderCollapse,
    onMenu: () -> Unit,
    bar: Boolean = true,
    actions: @Composable RowScope.() -> Unit,
) {
    val outer = LocalDesign.current
    if (room.finished && !outer.dark) {
        TodoTheme(dark = true, roomHue = outer.roomHue) { HeaderBody(room, landscape, collapse, onMenu, bar, actions) }
    } else {
        HeaderBody(room, landscape, collapse, onMenu, bar, actions)
    }
}

@Composable
private fun HeaderBody(
    room: RoomView,
    landscape: Dp,
    collapse: HeaderCollapse,
    onMenu: () -> Unit,
    bar: Boolean,
    actions: @Composable RowScope.() -> Unit,
) {
    val d = LocalDesign.current
    val r = d.room
    val sky = if (room.finished) {
        val dusk = d.dusk()
        Modifier.background(Brush.verticalGradient(listOf(dusk.top, dusk.low)))
    } else {
        Modifier.background(r.sky)
    }
    val motion = tween<IntSize>(Tokens.Motion.PANEL, easing = Tokens.Motion.EASE_STANDARD.cubic())
    Column(sky.fillMaxWidth().statusBarsPadding()) {
        AnimatedVisibility(bar, enter = expandVertically(motion), exit = shrinkVertically(motion)) {
            Row(
                Modifier.fillMaxWidth().height(Tokens.Metrics.Phone.APP_BAR.dp).padding(horizontal = S.S2.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                IconButton("menu", "Rooms", onClick = onMenu)
                // The name fades in over the second half of the collapse, once the big title has slid away.
                Label(
                    room.title,
                    Tokens.Type.ROOM_TITLE_BAR,
                    r.ink,
                    Modifier
                        .weight(1f)
                        .padding(start = S.S1.dp, end = S.S2.dp)
                        .graphicsLayer { alpha = ((collapse.fraction - 0.5f) * 2f).coerceIn(0f, 1f) },
                )
                actions()
            }
        }
        Column(Modifier.collapsing(collapse, HeaderCollapse.Part.Body)) {
            Column(Modifier.padding(start = S.S5.dp, end = S.S5.dp, bottom = S.S1.dp)) {
                EyebrowLine(room.eyebrow)
                Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.Bottom) {
                    Label(room.title, Tokens.Type.ROOM_TITLE_PHONE, r.ink, Modifier.weight(1f).padding(end = S.S3.dp))
                    Summary(room, Modifier.padding(bottom = S.S2.dp))
                }
            }
            Landscape(Scene(room.done.toInt(), room.total.toInt(), room.finished), landscape)
        }
    }
}

@Composable
private fun EyebrowLine(e: Eyebrow) {
    val r = LocalDesign.current.room
    Row(
        Modifier.heightIn(min = Tokens.Type.EYEBROW_DAY_PHONE.lineHeight.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(S.S2.dp),
    ) {
        when (e) {
            is Eyebrow.Day -> {
                Label(e.weekday, Tokens.Type.EYEBROW_DAY_PHONE, r.ink2)
                Label(e.date, Tokens.Type.EYEBROW_META_PHONE, r.ink3)
                Box(Modifier.size(Tokens.Elevation.HAIRLINE.dp, Micro.eyebrowBar).background(r.lineStrong))
                Label(e.week, Tokens.Type.EYEBROW_META_PHONE, r.ink3)
            }
            is Eyebrow.Kind -> {
                Label(e.kind, Tokens.Type.EYEBROW_DAY_PHONE, r.ink2)
                Label(e.detail, Tokens.Type.EYEBROW_META_PHONE, r.ink3)
            }
        }
    }
}

/** "3 of 12 done · 9 open", "12 of 12 done · all clear", or "2 finished this month" for an empty list. */
@Composable
private fun Summary(room: RoomView, modifier: Modifier = Modifier) {
    val r = LocalDesign.current.room
    val figures = Tokens.Type.SUMMARY_FIGURES
    val words = Tokens.Type.SUMMARY
    val bold = Tokens.Type.BUTTON.weight
    Row(modifier, verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(S.S1.dp)) {
        if (room.total == 0u) {
            if (room.finishedThisMonth > 0u) {
                Label("${room.finishedThisMonth}", figures, r.ink, weight = bold)
                Label("finished this month", words, r.ink2)
            }
            return@Row
        }
        Label("${room.done}", figures, r.accentStrong, weight = bold)
        Label("of", words, r.ink2)
        Label("${room.total}", figures, r.ink, weight = bold)
        Label("done", words, r.ink2)
        Box(Modifier.padding(horizontal = S.S1.dp).size(Micro.separator).background(r.ink4, CircleShape))
        if (room.open == 0u) {
            Label("all clear", words, r.accentStrong, weight = bold)
        } else {
            Label("${room.open}", figures, r.ink, weight = bold)
            Label("open", words, r.ink2)
        }
    }
}
