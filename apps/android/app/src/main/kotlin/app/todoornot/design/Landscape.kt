package app.todoornot.design

import androidx.compose.animation.core.CubicBezierEasing
import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.animation.core.tween
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.remember
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clipToBounds
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.graphics.StrokeJoin
import androidx.compose.ui.graphics.drawscope.DrawScope
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.graphics.drawscope.scale
import androidx.compose.ui.graphics.vector.PathParser
import androidx.compose.ui.unit.Dp
import app.todoornot.core.LandscapeShapes
import app.todoornot.core.Pt
import app.todoornot.core.landscape as landscapeGeometry

fun Easing.cubic(): CubicBezierEasing = CubicBezierEasing(x1, y1, x2, y2)

/** What a header's landscape shows. */
data class Scene(val done: Int, val total: Int, val finished: Boolean)

private class Geometry(
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

/**
 * The header landscape: hills, and the sun on its arc at done / total. The geometry is the shared
 * `todo_design::landscape` (through the Rust core), built when the size or the scene changes; while the
 * count changes the sun eases along that arc for [Tokens.Motion.SUN_TRAVEL] ms.
 */
@Composable
fun Landscape(scene: Scene, height: Dp, modifier: Modifier = Modifier) {
    val d = LocalDesign.current
    val r = d.room
    val dusk = d.dusk()
    val target = if (scene.total == 0) 0f else scene.done.coerceAtMost(scene.total).toFloat() / scene.total
    val progress by animateFloatAsState(
        targetValue = target,
        animationSpec = tween(Tokens.Motion.SUN_TRAVEL, easing = Tokens.Motion.EASE_SUN.cubic()),
        label = "sun",
    )
    BoxWithConstraints(modifier.fillMaxWidth().height(if (scene.finished) Tokens.Landscape.HEIGHT_FINISHED.dpOf() else height)) {
        val w = maxWidth.value
        val h = if (scene.finished) Tokens.Landscape.HEIGHT_FINISHED else height.value
        val showsSun = scene.finished || scene.total > 0
        val geo = remember(w, h, scene.finished, showsSun, d.dark) {
            val shapes = landscapeGeometry(w, h, scene.finished, if (showsSun) 1u else 0u, if (showsSun) 1u else 0u, d.dark)
            val arc = if (scene.finished) shapes.travelled else landscapeGeometry(w, h, false, 1u, 1u, false).travelled
            Geometry(shapes, arc, pathOf(shapes.farHill), pathOf(shapes.nearHill), shapes.moon?.let(::pathOf))
        }
        Canvas(Modifier.fillMaxWidth().height(maxHeight).clipToBounds()) {
            scale(density, density, pivot = Offset.Zero) {
                val s = geo.shapes
                if (!scene.finished && d.dark) {
                    drawRect(r.horizon, topLeft = Offset(0f, s.horizonTop), size = androidx.compose.ui.geometry.Size(w, h - s.horizonTop))
                }
                val starColor = if (scene.finished) dusk.star else r.star
                for (star in s.stars) {
                    drawCircle(starColor.copy(alpha = s.starOpacity), star.radius, Offset(star.center.x, star.center.y))
                }
                geo.moon?.let { drawPath(it, dusk.star.copy(alpha = s.moonOpacity)) }
                val sun = s.sun
                if (sun != null) {
                    val travelledColor = if (scene.finished) dusk.sun.copy(alpha = 0.5f) else r.sunInk
                    val center: Offset
                    if (scene.finished) {
                        polyline(geo.arc.map { Offset(it.x, it.y) }, travelledColor, s.travelledStroke)
                        center = Offset(sun.center.x, sun.center.y)
                    } else {
                        val n = geo.arc.size - 1
                        val cut = progress * n
                        val whole = cut.toInt()
                        center = at(geo.arc, progress)
                        val travelled = geo.arc.take(whole + 1).map { Offset(it.x, it.y) } + center
                        val remaining = listOf(center) + geo.arc.drop(whole + 1).map { Offset(it.x, it.y) }
                        polyline(remaining, r.arc, s.arcStroke)
                        if (progress > 0f) polyline(travelled, travelledColor, s.travelledStroke)
                    }
                    val sunColor = if (scene.finished) dusk.sun else r.sun
                    val shift = Offset(center.x - sun.center.x, center.y - sun.center.y)
                    for (ring in sun.rings) {
                        drawCircle(sunColor.copy(alpha = ring.opacity), ring.radius, center, style = Stroke(s.ringStroke))
                    }
                    drawCircle(sunColor.copy(alpha = s.haloOpacity), sun.haloRadius, center)
                    for (ray in sun.rays) {
                        drawLine(
                            sunColor,
                            Offset(ray.from.x, ray.from.y) + shift,
                            Offset(ray.to.x, ray.to.y) + shift,
                            strokeWidth = s.rayStroke,
                            cap = StrokeCap.Round,
                        )
                    }
                    drawCircle(sunColor, sun.discRadius, center)
                }
                drawPath(geo.farHill, if (scene.finished) dusk.silhouetteFar else r.hillFar)
                drawPath(geo.nearHill, if (scene.finished) dusk.silhouetteNear else r.hillNear)
            }
        }
    }
}

private fun Float.dpOf(): Dp = Dp(this)
