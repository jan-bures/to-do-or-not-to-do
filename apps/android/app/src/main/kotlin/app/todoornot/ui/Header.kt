package app.todoornot.ui

import androidx.compose.foundation.clickable
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.semantics.hideFromAccessibility
import androidx.compose.ui.semantics.semantics
import app.todoornot.core.SyncState
import app.todoornot.core.SyncStatus
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
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
 * The compact read-only strip replaces the full scene as it collapses. It can disappear entirely
 * when the keyboard would otherwise leave too little room for tasks.
 */
@Composable
fun RoomHeader(
    room: RoomView,
    landscape: Dp,
    collapse: HeaderCollapse,
    status: SyncStatus,
    compact: Dp = Tokens.Metrics.Phone.COMPACT_HEADER.dp,
    onResetReach: () -> Unit,
) {
    val outer = LocalDesign.current
    val motion = LocalMotion.current
    val dusk: () -> DuskLook = remember(room.key, room.finished, motion) {
        { motion.dusk(room.key) ?: DuskLook.rest(room.finished) }
    }
    val sun: () -> Float? = remember(room.key, motion) { { motion.sun(room.key) } }
    val nightInk by remember(dusk) { derivedStateOf { dusk().nightInk } }
    TodoTheme(dark = outer.dark || nightInk, roomHue = outer.roomHue) {
        HeaderBody(room, landscape, collapse, status, compact, onResetReach, outer, dusk, sun)
    }
}

@Composable
private fun HeaderBody(
    room: RoomView,
    landscape: Dp,
    collapse: HeaderCollapse,
    status: SyncStatus,
    compact: Dp,
    onResetReach: () -> Unit,
    outer: Design,
    dusk: () -> DuskLook,
    sun: () -> Float?,
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
    val density = LocalDensity.current
    val quiet = when {
        status.problem != null -> "Sync error"
        status.state == SyncState.OFFLINE && status.pending > 0u -> "Offline · ${status.pending}"
        status.state == SyncState.OFFLINE -> "Offline"
        status.pending > 0u -> "${status.pending} pending"
        else -> null
    }
    val stretched = collapse.reach > 0f && collapse.forced == 0f
    Column(sky.fillMaxWidth().statusBarsPadding().then(if (stretched) Modifier.clickable(
        remember { MutableInteractionSource() }, indication = null, onClickLabel = "Reset header reach", onClick = onResetReach,
    ) else Modifier)) {
        if (stretched) Spacer(Modifier.height(with(density) { collapse.reach.toDp() }))
        Box(Modifier.fillMaxWidth()) {
            Column(Modifier.collapsing(collapse, with(density) { compact.roundToPx() }).semantics {
                if (collapse.fraction > 0.5f) hideFromAccessibility()
            }) {
                Column(Modifier.padding(start = S.S5.dp, end = S.S5.dp, top = S.S2.dp, bottom = S.S1.dp)) {
                    Row(verticalAlignment = Alignment.CenterVertically) {
                        Box(Modifier.weight(1f)) { EyebrowLine(room.eyebrow) }
                        quiet?.let { Label(it, Tokens.Type.SUMMARY_FIGURES, r.ink2) }
                    }
                    Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.Bottom) {
                        Label(room.title, Tokens.Type.ROOM_TITLE_PHONE, r.ink, Modifier.weight(1f).padding(end = S.S3.dp))
                        Summary(room, Modifier.padding(bottom = S.S2.dp))
                    }
                }
                Landscape(Scene(room.done.toInt(), room.total.toInt(), room.finished), landscape, design = outer, sun = sun, dusk = dusk)
            }
            if (compact > 0.dp) Row(
                Modifier.align(Alignment.BottomCenter).fillMaxWidth().height(compact)
                    .semantics { if (collapse.fraction <= 0.5f) hideFromAccessibility() }
                    .graphicsLayer { alpha = ((collapse.fraction - 0.5f) * 2f).coerceIn(0f, 1f) }
                    .then(sky).hairline(r.line).padding(horizontal = S.S5.dp),
                verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.spacedBy(S.S2.dp),
            ) {
                Label(room.title, Tokens.Type.ROOM_TITLE_BAR, r.ink, Modifier.weight(1f))
                if (room.total > 0u) {
                    if (room.done > 0u) Box(Modifier.size(Tokens.Radius.LG.dp).background(r.sunInk, CircleShape))
                    Label("${room.done}/${room.total}", Tokens.Type.SUMMARY_FIGURES, r.ink2)
                }
                quiet?.let { Label(it, Tokens.Type.SUMMARY_FIGURES, r.ink2) }
            }
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
