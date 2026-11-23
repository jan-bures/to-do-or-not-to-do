package app.todoornot.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.gestures.awaitEachGesture
import androidx.compose.foundation.gestures.awaitFirstDown
import androidx.compose.foundation.gestures.waitForUpOrCancellation
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.runtime.Composable
import androidx.compose.runtime.derivedStateOf
import androidx.compose.runtime.getValue
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clipToBounds
import androidx.compose.ui.draw.drawBehind
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.layout.Layout
import androidx.compose.ui.layout.layout
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.semantics.clearAndSetSemantics
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.heading
import androidx.compose.ui.semantics.hideFromAccessibility
import androidx.compose.ui.semantics.isTraversalGroup
import androidx.compose.ui.semantics.onClick
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.semantics.traversalIndex
import androidx.compose.ui.unit.Constraints
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.constrainHeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import app.todoornot.core.Eyebrow
import app.todoornot.core.RoomSummary
import app.todoornot.core.RoomView
import app.todoornot.core.SyncState
import app.todoornot.core.SyncStatus
import app.todoornot.design.Design
import app.todoornot.design.HeaderPanorama
import app.todoornot.design.LocalDesign
import app.todoornot.design.TextToken
import app.todoornot.design.TodoTheme
import app.todoornot.design.Tokens
import app.todoornot.design.headerSky
import app.todoornot.model.DuskLook
import kotlin.math.roundToInt

private val S = Tokens.Space
private val P = Tokens.Metrics.Phone
private val T = Tokens.Type

/** Read-only sky, with no controls. A stretched sky can be dismissed without dragging it back. */
@Composable
fun RoomHeader(
    room: RoomView,
    inner: Boolean,
    collapse: HeaderCollapse,
    status: SyncStatus,
    compact: Dp = P.COMPACT_HEADER.dp,
    selected: Boolean = false,
    activitySummary: String? = null,
    viewLabel: String? = null,
    onResetReach: () -> Unit,
) {
    val outer = LocalDesign.current
    val motion = LocalMotion.current
    val dusk = remember(room.key, room.finished, motion) {
        { motion.dusk(room.key) ?: DuskLook.rest(room.finished) }
    }
    val target = if (room.total == 0u) 0f else room.done.toFloat() / room.total.toFloat()
    val progress = remember(room.key, target, motion) { { (motion.sun(room.key) ?: target).coerceIn(0f, 1f) } }
    val nightInk by remember(dusk) { derivedStateOf { dusk().nightInk } }
    val quiet = when {
        status.problem != null -> "Sync error"
        status.state == SyncState.OFFLINE && status.pending > 0u -> "Offline · ${status.pending}"
        status.state == SyncState.OFFLINE -> "Offline"
        status.pending > 0u -> "${status.pending} pending"
        else -> null
    }
    val reset by rememberUpdatedState(onResetReach)
    TodoTheme(dark = outer.dark || nightInk, roomHue = outer.roomHue) {
        val textDesign = LocalDesign.current
        val restScene = if (inner) P.HEADER_SCENE_INNER else P.HEADER_SCENE_COVER
        Layout(
            modifier = Modifier.fillMaxWidth()
                .headerSky(outer, room.localHour.toInt(), collapse, dusk, restScene)
                .hairline(textDesign.room.line)
                .statusBarsPadding()
                .clipToBounds()
                .semantics {
                    if (collapse.visibleReach > 0f) onClick("Return header to rest") { reset(); true }
                }
                .pointerInput(collapse) {
                    awaitEachGesture {
                        awaitFirstDown(requireUnconsumed = false)
                        if (collapse.visibleReach > 0f) {
                            val up = waitForUpOrCancellation()
                            if (up != null) { up.consume(); reset() }
                        }
                    }
                },
            content = {
                HeaderPanorama(outer, room.localHour.toInt(), inner, collapse, room.total > 0u, progress, dusk)
                HeaderText(room, activitySummary, quiet, collapse, viewLabel)
                CompactHeader(room, quiet, compact, selected, collapse, progress, dusk, textDesign)
            },
        ) { (scene, text, bar), constraints ->
            val width = constraints.maxWidth
            val readable = Constraints.fixedWidth(width)
            val textPlaceable = text.measure(readable)
            val look = dusk()
            val sceneRest = (restScene + (Tokens.Landscape.HEIGHT_FINISHED - restScene) * look.height).dp.toPx()
            val base = textPlaceable.height + sceneRest.roundToInt()
            val compactPx = compact.roundToPx().coerceAtMost(constraints.maxHeight)
            if (collapse.compact != compactPx) collapse.compact = compactPx
            if (collapse.body != base) collapse.body = base
            val reachTop = collapse.reachToText.roundToInt()
            val reachTotal = collapse.visibleReach.roundToInt()
            val scenePx = (sceneRest.roundToInt() + reachTotal - reachTop).coerceAtLeast(0)
            val height = constraints.constrainHeight(base - collapse.hidden.roundToInt() + reachTotal)
            val landscape = scene.measure(Constraints.fixed(width, scenePx))
            val compactPlaceable = bar.measure(Constraints.fixed(width, compactPx))
            layout(width, height) {
                if (collapse.fraction < 0.70f) landscape.place(0, height - scenePx)
                if (collapse.fraction < 0.55f) textPlaceable.placeRelative(0, reachTop)
                if (compactPx > 0 && collapse.fraction >= 0.5f) compactPlaceable.placeRelative(0, height - compactPx)
            }
        }
    }
}

@Composable
private fun HeaderText(room: RoomView, activitySummary: String?, quiet: String?, collapse: HeaderCollapse, viewLabel: String?) {
    val r = LocalDesign.current.room
    fun Modifier.detailMotion() = graphicsLayer {
        val fade = HeaderChoreography.detail(collapse.fraction)
        alpha = 1f - fade
        translationY = if (collapse.reducedMotion) 0f else -S.S2.dp.toPx() * fade
    }
    Column(
        Modifier.fillMaxWidth()
            .semantics {
                isTraversalGroup = true
                if (collapse.fraction > 0.5f) hideFromAccessibility()
            }
            .padding(start = S.S5.dp, end = S.S5.dp, top = S.S3.dp, bottom = S.S2.dp),
        verticalArrangement = Arrangement.spacedBy(S.S1.dp),
    ) {
        Row(
            Modifier.fillMaxWidth().heightIn(min = S.S5.dp).detailMotion(),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(S.S2.dp),
        ) {
            Row(Modifier.weight(1f).semantics(mergeDescendants = true) { traversalIndex = 0f },
                verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(S.S2.dp)) {
                EyebrowLine(room.eyebrow)
                viewLabel?.let { Label("· $it", T.EYEBROW_META_PHONE, r.ink3) }
            }
            quiet?.let { Label(it, T.SUMMARY_FIGURES, r.ink2, Modifier.semantics { traversalIndex = 4f }) }
        }
        Box(Modifier.fillMaxWidth().heightIn(min = T.ROOM_TITLE_PHONE.lineHeight.dp).graphicsLayer {
            val fade = HeaderChoreography.title(collapse.fraction)
            alpha = 1f - fade
            translationY = if (collapse.reducedMotion) 0f else -S.S3.dp.toPx() * fade
        }, contentAlignment = Alignment.CenterStart) {
            HeaderTitle(room.title, T.ROOM_TITLE_PHONE, r.ink, Modifier.semantics { heading(); traversalIndex = 1f })
        }
        FlowRow(
            Modifier.fillMaxWidth().heightIn(min = S.S5.dp).detailMotion(),
            horizontalArrangement = Arrangement.SpaceBetween,
            verticalArrangement = Arrangement.spacedBy(S.S1.dp),
            itemVerticalAlignment = Alignment.CenterVertically,
            maxItemsInEachRow = if (LocalDensity.current.fontScale >= 1.3f) 1 else 2,
        ) {
            val hasCue = room.overdueCount > 0u || room.nextDueToday != null
            val summary = Modifier.then(if (hasCue) Modifier.padding(end = S.S2.dp) else Modifier)
            if (activitySummary != null) {
                Label(activitySummary, T.SUMMARY_FIGURES, r.ink2, summary.semantics { traversalIndex = 2f })
            } else if (room.summary != RoomSummary.None) {
                Summary(room, summary.clearAndSetSemantics { contentDescription = room.summaryText; traversalIndex = 2f })
            }
            if (room.overdueCount > 0u) {
                OverdueBadge("${room.overdueCount} overdue", Modifier.clearAndSetSemantics {
                    contentDescription = "${room.overdueCount} overdue"
                    traversalIndex = 3f
                })
            } else room.nextDueToday?.let { time ->
                Label("Next · $time", T.SUMMARY_FIGURES, r.accentStrong, Modifier.clearAndSetSemantics {
                    contentDescription = "Next due at $time"
                    traversalIndex = 3f
                }, weight = T.ROOM_TITLE_PHONE.weight)
            }
        }
    }
}

@Composable
private fun CompactHeader(
    room: RoomView, quiet: String?, height: Dp, selected: Boolean, collapse: HeaderCollapse,
    progress: () -> Float, dusk: () -> DuskLook, design: Design,
) {
    val r = design.room
    Row(
        Modifier.fillMaxWidth().clipToBounds()
            .clearAndSetSemantics {
                if (collapse.fraction <= 0.5f || height == 0.dp) hideFromAccessibility()
                else {
                    heading()
                    contentDescription = buildString {
                        append(room.title)
                        if (room.total > 0u) append(", ${room.done} of ${room.total} done")
                        if (room.overdueCount > 0u) append(", ${room.overdueCount} overdue")
                        quiet?.let { append(", $it") }
                    }
                }
            }
            .graphicsLayer { alpha = HeaderChoreography.compact(collapse.fraction) }
            .drawBehind {
                drawRect(if (dusk().nightInk) design.dusk().top else if (selected) r.selectionWash else r.sky)
                if (room.total > 0u) {
                    val h = P.COMPACT_PROGRESS_HEIGHT.dp.toPx()
                    val at = Offset(0f, size.height - h - Tokens.Elevation.HAIRLINE.dp.toPx())
                    drawRect(r.arc.copy(alpha = 0.30f), at, Size(size.width, h))
                    val travelled = size.width * progress()
                    if (travelled > 0f) drawRect(r.sunInk, at, Size(travelled, h))
                }
            }
            .padding(horizontal = S.S5.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(S.S2.dp),
    ) {
        HeaderTitle(room.title, T.ROOM_TITLE_BAR, r.ink, Modifier.weight(1f))
        if (room.overdueCount > 0u) {
            Label("${room.overdueCount} overdue", T.SUMMARY_FIGURES, r.dangerInk, weight = T.ROOM_TITLE_PHONE.weight)
            if (room.total > 0u) Label("·", T.SUMMARY_FIGURES, r.ink4)
        }
        if (room.total > 0u) {
            Label("${room.done}/${room.total}", T.SUMMARY_FIGURES, r.ink2)
        }
        quiet?.let { Label(it, T.SUMMARY_FIGURES, r.ink2) }
    }
}

@Composable
private fun HeaderTitle(title: String, token: TextToken, color: Color, modifier: Modifier) {
    // Centre the taller font box in the optical line without clipping accents or descenders. The
    // slot grows with sp, unlike a fixed-height box, so large-text settings keep the full glyphs.
    Label(title, token, color, modifier.layout { text, constraints ->
        val p = text.measure(constraints.copy(minHeight = 0, maxHeight = Constraints.Infinity))
        val line = constraints.constrainHeight(token.lineHeight.sp.roundToPx())
        layout(p.width, line) { p.placeRelative(0, (line - p.height) / 2) }
    })
}

@Composable
private fun EyebrowLine(e: Eyebrow) {
    val r = LocalDesign.current.room
    FlowRow(
        Modifier.heightIn(min = T.EYEBROW_DAY_PHONE.lineHeight.dp),
        horizontalArrangement = Arrangement.spacedBy(S.S2.dp),
        itemVerticalAlignment = Alignment.CenterVertically,
    ) {
        when (e) {
            is Eyebrow.Day -> {
                Label(e.weekday, T.EYEBROW_DAY_PHONE, r.accentStrong)
                Label(e.date, T.EYEBROW_META_PHONE, r.ink3)
                Box(Modifier.size(Tokens.Elevation.HAIRLINE.dp, Micro.eyebrowBar).background(r.lineStrong))
                Label(e.week, T.EYEBROW_META_PHONE, r.ink3)
            }
            is Eyebrow.Kind -> {
                Label(e.kind, T.EYEBROW_DAY_PHONE, r.accentStrong)
                Label(e.detail, T.EYEBROW_META_PHONE, r.ink3)
            }
        }
    }
}

/** The shared summary, with its figures in mono but announced as one line. */
@Composable
private fun Summary(room: RoomView, modifier: Modifier) {
    val r = LocalDesign.current.room
    val figures = T.SUMMARY_FIGURES
    val words = T.SUMMARY
    val bold = T.BUTTON.weight
    Row(modifier, verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(S.S1.dp)) {
        @Composable
        fun doneOf(done: UInt, total: UInt) {
            Label("$done", figures, r.accentStrong, weight = bold)
            Label("of", words, r.ink2)
            Label("$total", figures, r.ink, weight = bold)
            Label("done", words, r.ink2)
            Label("·", figures, r.ink4, Modifier.padding(horizontal = S.S1.dp))
        }
        when (val s = room.summary) {
            RoomSummary.None -> Unit
            is RoomSummary.Empty -> if (s.finishedThisMonth > 0u) {
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
