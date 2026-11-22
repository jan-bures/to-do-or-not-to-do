package app.todoornot.design

import androidx.compose.animation.core.CubicBezierEasing
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.runtime.Composable
import androidx.compose.runtime.derivedStateOf
import androidx.compose.runtime.getValue
import androidx.compose.runtime.remember
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clipToBounds
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.graphics.StrokeJoin
import androidx.compose.ui.graphics.drawscope.DrawScope
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.graphics.drawscope.scale
import androidx.compose.ui.graphics.drawscope.translate
import androidx.compose.ui.graphics.vector.PathParser
import androidx.compose.ui.layout.layout
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import app.todoornot.core.LandscapeShapes
import app.todoornot.core.Pt
import app.todoornot.model.DuskLook
import app.todoornot.core.landscape as landscapeGeometry

fun Easing.cubic(): CubicBezierEasing = CubicBezierEasing(x1, y1, x2, y2)

/** What a header's landscape shows. */
data class Scene(val done: Int, val total: Int, val finished: Boolean)

private class Geometry(
    /** The scene's height in dp: it sits at the bottom of the header, however tall that is. */
    val height: Float,
    val shapes: LandscapeShapes,
    /** The whole arc at uniform progress steps, for moving the sun without new geometry per frame. */
    val arc: List<Pt>,
    val farHill: Path,
    val nearHill: Path,
    val moon: Path?,
)

private fun pathOf(d: String): Path = PathParser().parsePathString(d).toPath()

private fun at(arc: List<Pt>, t: Float): Offset {
    if (arc.isEmpty()) return Offset.Zero
    val x = t.coerceIn(0f, 1f) * (arc.size - 1)
    val i = x.toInt().coerceAtMost(arc.size - 2).coerceAtLeast(0)
    val f = x - i
    val a = arc[i]
    val b = arc[minOf(i + 1, arc.size - 1)]
    return Offset(a.x + (b.x - a.x) * f, a.y + (b.y - a.y) * f)
}

private fun DrawScope.polyline(points: List<Offset>, color: Color, width: Float) {
    if (points.size < 2) return
    val p = Path().apply {
        moveTo(points[0].x, points[0].y)
        for (q in points.drop(1)) lineTo(q.x, q.y)
    }
    drawPath(p, color, style = Stroke(width, cap = StrokeCap.Round, join = StrokeJoin.Round))
}

private fun Color.faded(a: Float): Color = copy(alpha = alpha * a)

/**
 * The header landscape: hills, and the sun on its arc at done / total, or the finished day's dusk scene.
 * The geometry is the shared `todo_design::landscape` (through the Rust core), built once per size and
 * scene, never per frame. The sun's place comes from [sun] (the motion, gliding along that arc) or else
 * the count; [dusk] says where the header is between the everyday strip and the dusk scene: while dusk
 * falls (or lifts) the landscape grows from [height] to the dusk scene's height, the strip fades out as
 * the scene fades in over it, and the moon and the stars come out in their batches. Both are read at
 * layout and draw time, so a frame only lays out and draws. [design] is the room's own theme (the header
 * text may already be in the night's ink).
 */
@Composable
fun Landscape(
    scene: Scene,
    height: Dp,
    modifier: Modifier = Modifier,
    design: Design = LocalDesign.current,
    sun: () -> Float? = { null },
    dusk: () -> DuskLook = { DuskLook.rest(scene.finished) },
) {
    val r = design.room
    val dc = design.dusk()
    val target = if (scene.total == 0) 0f else scene.done.coerceAtMost(scene.total).toFloat() / scene.total
    val finishedH = Tokens.Landscape.HEIGHT_FINISHED
    val strip by remember(dusk) { derivedStateOf { dusk().scene < 1f } }
    val night by remember(dusk) { derivedStateOf { dusk().scene > 0f } }
    BoxWithConstraints(
        modifier.fillMaxWidth().layout { measurable, constraints ->
            val grown = height.value + (finishedH - height.value) * dusk().height
            val h = grown.dp.roundToPx()
            val p = measurable.measure(constraints.copy(minHeight = h, maxHeight = h))
            layout(p.width, h) { p.place(0, 0) }
        },
    ) {
        val w = maxWidth.value
        val slim = height.value
        val showsSun = scene.total > 0
        val stripGeo = if (!strip) {
            null
        } else {
            remember(w, slim, showsSun, design.dark) {
                val shapes = landscapeGeometry(w, slim, false, if (showsSun) 1u else 0u, if (showsSun) 1u else 0u, design.dark)
                val arc = landscapeGeometry(w, slim, false, 1u, 1u, false).travelled
                Geometry(slim, shapes, arc, pathOf(shapes.farHill), pathOf(shapes.nearHill), shapes.moon?.let(::pathOf))
            }
        }
        val duskGeo = if (!night) {
            null
        } else {
            remember(w, design.dark) {
                val shapes = landscapeGeometry(w, finishedH, true, 1u, 1u, design.dark)
                Geometry(finishedH, shapes, shapes.travelled, pathOf(shapes.farHill), pathOf(shapes.nearHill), shapes.moon?.let(::pathOf))
            }
        }
        Canvas(Modifier.fillMaxSize().clipToBounds()) {
            val look = dusk()
            val s = look.scene
            scale(density, density, pivot = Offset.Zero) {
                val now = size.height / density
                stripGeo?.let { g ->
                    translate(top = now - g.height) { strip(g, r, dc, design.dark, w, sun() ?: target, 1f - s) }
                }
                duskGeo?.let { g ->
                    translate(top = now - g.height) { duskScene(g, dc, s, look) }
                }
            }
        }
    }
}

/** The everyday strip at [alpha]: hills, the arc and the sun at [progress] (a dark room adds a horizon and stars). */
private fun DrawScope.strip(g: Geometry, r: RoomColors, dc: DuskColors, dark: Boolean, w: Float, progress: Float, alpha: Float) {
    val s = g.shapes
    if (dark) drawRect(r.horizon.faded(alpha), topLeft = Offset(0f, s.horizonTop), size = Size(w, g.height - s.horizonTop))
    for (star in s.stars) {
        drawCircle(r.star.faded(s.starOpacity * alpha), star.radius, Offset(star.center.x, star.center.y))
    }
    g.moon?.let { drawPath(it, dc.star.faded(s.moonOpacity * alpha)) }
    val sun = s.sun
    if (sun != null) {
        val n = g.arc.size - 1
        val whole = (progress * n).toInt()
        val center = at(g.arc, progress)
        val travelled = g.arc.take(whole + 1).map { Offset(it.x, it.y) } + center
        val remaining = listOf(center) + g.arc.drop(whole + 1).map { Offset(it.x, it.y) }
        polyline(remaining, r.arc.faded(alpha), s.arcStroke)
        if (progress > 0f) polyline(travelled, r.sunInk.faded(alpha), s.travelledStroke)
        drawSun(sun, center, r.sun, s, alpha)
    }
    drawPath(g.farHill, r.hillFar.faded(alpha))
    drawPath(g.nearHill, r.hillNear.faded(alpha))
}

/**
 * The dusk scene at [alpha] over the strip: the sun set at the end of its arc, the dusk silhouettes, and
 * the moon and the stars as far as the night has come ([DuskLook.night], the stars in batches).
 */
private fun DrawScope.duskScene(g: Geometry, dc: DuskColors, alpha: Float, look: DuskLook) {
    if (alpha <= 0f) return
    val s = g.shapes
    s.stars.forEachIndexed { i, star ->
        val batch = i % Tokens.Motion.STAGGER_MAX_ITEMS.toInt()
        drawCircle(dc.star.faded(s.starOpacity * alpha * look.night(batch)), star.radius, Offset(star.center.x, star.center.y))
    }
    g.moon?.let { drawPath(it, dc.star.faded(s.moonOpacity * alpha * look.night)) }
    val sun = s.sun
    if (sun != null) {
        polyline(g.arc.map { Offset(it.x, it.y) }, dc.sun.copy(alpha = 0.5f).faded(alpha), s.travelledStroke)
        drawSun(sun, Offset(sun.center.x, sun.center.y), dc.sun, s, alpha)
    }
    drawPath(g.farHill, dc.silhouetteFar.faded(alpha))
    drawPath(g.nearHill, dc.silhouetteNear.faded(alpha))
}

private fun DrawScope.drawSun(sun: app.todoornot.core.SunShape, center: Offset, color: Color, s: LandscapeShapes, alpha: Float) {
    val shift = Offset(center.x - sun.center.x, center.y - sun.center.y)
    for (ring in sun.rings) {
        drawCircle(color.faded(ring.opacity * alpha), ring.radius, center, style = Stroke(s.ringStroke))
    }
    drawCircle(color.faded(s.haloOpacity * alpha), sun.haloRadius, center)
    for (ray in sun.rays) {
        drawLine(
            color.faded(alpha),
            Offset(ray.from.x, ray.from.y) + shift,
            Offset(ray.to.x, ray.to.y) + shift,
            strokeWidth = s.rayStroke,
            cap = StrokeCap.Round,
        )
    }
    drawCircle(color.faded(alpha), sun.discRadius, center)
}
