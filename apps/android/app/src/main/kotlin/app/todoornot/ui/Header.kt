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
import androidx.compose.runtime.derivedStateOf
import androidx.compose.runtime.getValue
import androidx.compose.runtime.remember
import androidx.compose.ui.draw.drawBehind
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.IntSize
import androidx.compose.ui.unit.dp
import app.todoornot.core.Eyebrow
import app.todoornot.core.RoomSummary
import app.todoornot.core.RoomView
import app.todoornot.design.Design
import app.todoornot.design.Landscape
import app.todoornot.design.LocalDesign
import app.todoornot.design.Scene
import app.todoornot.design.TodoTheme
import app.todoornot.design.Tokens
import app.todoornot.design.cubic
import app.todoornot.model.DuskLook

private val S = Tokens.Space

/**
 * A room's header: the sky in the room's hue with the bar, the eyebrow, the title and the count, and the
 * landscape strip with the sun at done / total. When the day is finished the sky turns to dusk, and the
 * header is drawn in the dark palette so the text stays readable on it.
 *
 * Dusk falls when My Day's last task has left (`docs/design-language.md`, Motion, moment 2): the dusk sky
 * fades in over the room's, the landscape grows into the dusk scene, and the text switches to the night's
 * ink at its cue. Reopening a task lifts it again, more briskly.
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
    val motion = LocalMotion.current
    val dusk: () -> DuskLook = remember(room.key, room.finished, motion) {
        { motion.dusk(room.key) ?: DuskLook.rest(room.finished) }
    }
    val sun: () -> Float? = remember(room.key, motion) { { motion.sun(room.key) } }
    val nightInk by remember(dusk) { derivedStateOf { dusk().nightInk } }
    TodoTheme(dark = outer.dark || nightInk, roomHue = outer.roomHue) {
        HeaderBody(room, landscape, collapse, onMenu, bar, outer, dusk, sun, actions)
    }
}

@Composable
private fun HeaderBody(
    room: RoomView,
    landscape: Dp,
    collapse: HeaderCollapse,
    onMenu: () -> Unit,
    bar: Boolean,
    outer: Design,
    dusk: () -> DuskLook,
    sun: () -> Float?,
    actions: @Composable RowScope.() -> Unit,
) {
    val d = LocalDesign.current
    val r = d.room
    // The room's sky, and the dusk sky fading in over it (read at draw time).
    val daySky = outer.room.sky
    val duskSky = Brush.verticalGradient(outer.dusk().let { listOf(it.top, it.low) })
    val sky = Modifier.drawBehind {
        val scene = dusk().scene
        if (scene < 1f) drawRect(daySky)
        if (scene > 0f) drawRect(duskSky, alpha = scene)
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
            Landscape(Scene(room.done.toInt(), room.total.toInt(), room.finished), landscape, design = outer, sun = sun, dusk = dusk)
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
                Label(e.weekday, Tokens.Type.EYEBROW_DAY_PHONE, r.accentStrong)
                Label(e.date, Tokens.Type.EYEBROW_META_PHONE, r.ink3)
                Box(Modifier.size(Tokens.Elevation.HAIRLINE.dp, Micro.eyebrowBar).background(r.lineStrong))
                Label(e.week, Tokens.Type.EYEBROW_META_PHONE, r.ink3)
            }
            is Eyebrow.Kind -> {
                Label(e.kind, Tokens.Type.EYEBROW_DAY_PHONE, r.accentStrong)
                Label(e.detail, Tokens.Type.EYEBROW_META_PHONE, r.ink3)
            }
        }
    }
}

/**
 * The summary beside the sun, as every client words it (`todo_client::room_header`): "3 of 12 done · 9 open",
 * "12 of 12 done · all clear" for the finished day, "12 done" in Completed, and "2 finished this month" or
 * "nothing yet" for an empty room.
 */
@Composable
private fun Summary(room: RoomView, modifier: Modifier = Modifier) {
    val r = LocalDesign.current.room
    val figures = Tokens.Type.SUMMARY_FIGURES
    val words = Tokens.Type.SUMMARY
    val bold = Tokens.Type.BUTTON.weight
    Row(modifier, verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(S.S1.dp)) {
        @Composable
        fun doneOf(done: UInt, total: UInt) {
            Label("$done", figures, r.accentStrong, weight = bold)
            Label("of", words, r.ink2)
            Label("$total", figures, r.ink, weight = bold)
            Label("done", words, r.ink2)
            Box(Modifier.padding(horizontal = S.S1.dp).size(Micro.separator).background(r.ink4, CircleShape))
        }
        when (val s = room.summary) {
            RoomSummary.None -> Unit
            is RoomSummary.Empty ->
                if (s.finishedThisMonth > 0u) {
                    Label("${s.finishedThisMonth}", figures, r.ink, weight = bold)
                    Label("finished this month", words, r.ink2)
                } else {
                    Label("nothing yet", words, r.ink2)
                }
            is RoomSummary.Done -> {
                Label("${s.done}", figures, r.accentStrong, weight = bold)
                Label("done", words, r.ink2)
            }
            is RoomSummary.AllClear -> {
                doneOf(s.done, s.total)
                Label("all clear", words, r.accentStrong, weight = bold)
            }
            is RoomSummary.Progress -> {
                doneOf(s.done, s.total)
                Label("${s.open}", figures, r.ink, weight = bold)
                Label("open", words, r.ink2)
            }
        }
    }
}
