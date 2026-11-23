package app.todoornot.design

import androidx.compose.animation.core.Animatable
import androidx.compose.animation.core.CubicBezierEasing
import androidx.compose.animation.core.tween
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.remember
import androidx.compose.runtime.snapshotFlow
import androidx.compose.ui.Modifier
import androidx.compose.ui.MotionDurationScale
import androidx.compose.ui.draw.drawBehind
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.graphics.StrokeJoin
import androidx.compose.ui.graphics.TransformOrigin
import androidx.compose.ui.graphics.drawscope.DrawScope
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.graphics.drawscope.scale
import androidx.compose.ui.graphics.drawscope.translate
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.graphics.lerp
import androidx.compose.ui.graphics.vector.PathParser
import androidx.compose.ui.semantics.clearAndSetSemantics
import androidx.compose.ui.unit.dp
import app.todoornot.core.PhoneLandscape
import app.todoornot.core.PhonePanorama
import app.todoornot.core.phoneArcColor
import app.todoornot.core.phoneLandscape
import app.todoornot.model.DuskLook
import app.todoornot.ui.HeaderChoreography
import app.todoornot.ui.HeaderCollapse
import kotlinx.coroutines.flow.collectLatest
import kotlinx.coroutines.withContext
import kotlin.math.PI
import kotlin.math.cos
import kotlin.math.sin

fun Easing.cubic(): CubicBezierEasing = CubicBezierEasing(x1, y1, x2, y2)

private val L = Tokens.Landscape
private val S = Tokens.Space
private val nightBatches = Tokens.Motion.STAGGER_MAX_ITEMS.toInt()

private enum class Sky { Day, Twilight, Night }
private fun sky(dark: Boolean, hour: Int): Sky = when {
    dark && hour !in 6..19 -> Sky.Night
    !dark && hour !in 5..17 -> Sky.Twilight
    else -> Sky.Day
}

private fun mix(from: Float, to: Float, p: Float): Float = from + (to - from) * p
private fun Color.fade(alpha: Float) = copy(alpha = this.alpha * alpha)

private data class Spark(val x: Float, val y: Float, val radius: Float)
private val zenith = arrayOf(
    Spark(0.64f, 0.14f, 0.9f), Spark(0.76f, 0.09f, 1.1f), Spark(0.85f, 0.21f, 0.8f),
    Spark(0.92f, 0.12f, 1.0f), Spark(0.56f, 0.23f, 0.8f), Spark(0.71f, 0.27f, 0.9f),
)
private val reachStars = arrayOf(
    Spark(0.10f, 0.16f, 1.0f), Spark(0.22f, 0.11f, 0.8f), Spark(0.35f, 0.18f, 1.1f),
    Spark(0.48f, 0.12f, 0.9f), Spark(0.60f, 0.17f, 1.0f), Spark(0.74f, 0.13f, 0.8f),
    Spark(0.86f, 0.19f, 1.1f), Spark(0.94f, 0.14f, 0.9f),
)

/** One full-bleed gradient under the status bar, lettering and the panorama. */
internal fun Modifier.headerSky(
    design: Design, hour: Int, collapse: HeaderCollapse, dusk: () -> DuskLook, restScene: Float,
): Modifier = drawBehind {
    if (size.height <= 0f) return@drawBehind
    val look = dusk()
    val p = collapse.reachFraction
    val phase = collapse.fraction
    val fade = HeaderChoreography.compact(phase)
    val r = design.room
    val dc = design.dusk()
    val time = sky(design.dark, hour)
    val dayTop = if (design.dark) r.canvas else r.sky
    val dayMid = when {
        design.dark -> mix(0.48f, 0.44f, p)
        time == Sky.Twilight -> mix(0.44f, 0.42f, p)
        else -> mix(0.52f, 0.42f, p)
    }
    val dayLow = if (design.dark) mix(0.86f, 0.78f, p) else 1f
    val sceneH = (mix(restScene, L.HEIGHT_FINISHED, look.height)).dp.toPx() + collapse.reachToScene
    val sceneTop = (size.height - sceneH).coerceAtLeast(0f)
    val middle = mix(dayMid, 0.38f, look.scene)
    // Keep the gradient's warm end out of the lettering when accessibility text grows the header.
    val guard = ((sceneTop - S.S8.dp.toPx()) / size.height).coerceIn(0f, 1f)
    val midStop = if (look.nightInk) maxOf(middle, guard) else middle
    // For light rooms the text's backdrop changes with its ink, rather than passing through a muddy,
    // low-contrast midtone. The horizon and silhouettes still crossfade throughout the dusk motion.
    val upperNight = if (design.dark) look.scene else if (look.nightInk) 1f else 0f
    val compactSky = if (look.nightInk) dc.top else r.sky
    val top = lerp(lerp(dayTop, dc.top, upperNight), compactSky, fade)
    val mid = lerp(lerp(r.sky, dc.top, upperNight), compactSky, fade)
    val dayHorizon = if (!design.dark && look.nightInk) dc.top else r.horizon
    val low = lerp(lerp(dayHorizon, dc.low, look.scene), compactSky, fade)
    if (fade >= 1f) drawRect(compactSky) else {
        drawRect(Brush.verticalGradient(
            0f to top,
            midStop to mid,
            maxOf(midStop, mix(dayLow, 1f, look.scene)) to low,
            endY = size.height,
        ))
    }
    val sceneAlpha = 1f - HeaderChoreography.scene(phase)
    if (sceneAlpha <= 0f) return@drawBehind
    val band = maxOf(sceneTop + S.S3.dp.toPx(), 110.dp.toPx())
    val dayStars = if (design.dark) L.ZENITH_STAR_OPACITY else if (time == Sky.Twilight) 0.25f else 0f
    for (i in zenith.indices) {
        val s = zenith[i]
        val day = if (design.dark || i < 4) dayStars * (1f - look.scene) else 0f
        val night = look.scene * look.night(i % nightBatches) * L.ZENITH_STAR_OPACITY
        val alpha = (day + night) * sceneAlpha
        if (alpha > 0f) drawCircle(if (night > day) dc.star.fade(alpha) else r.star.fade(alpha),
            s.radius.dp.toPx(), Offset(size.width * s.x, band * s.y + 26.dp.toPx()))
    }
}

private const val ARC_STEPS = 64
private val arcSines = FloatArray(ARC_STEPS + 1) { sin(PI * it / ARC_STEPS).toFloat() }
private val rayX = FloatArray(8) { sin(PI * it / 4).toFloat() }
private val rayY = FloatArray(8) { -cos(PI * it / 4).toFloat() }
private val layerLag = floatArrayOf(0.35f, 0.65f, 1f)
private val arcLine = Stroke(L.ARC_STROKE, cap = StrokeCap.Round, join = StrokeJoin.Round)
private val travelledLine = Stroke(L.TRAVELLED_STROKE_PHONE, cap = StrokeCap.Round, join = StrokeJoin.Round)
private val duskLine = Stroke(L.TRAVELLED_STROKE_FINISHED, cap = StrokeCap.Round, join = StrokeJoin.Round)
private val ringLine = Stroke(L.RING_STROKE)
private val reachRing = Stroke(L.ARC_STROKE)
private val duskRadii = floatArrayOf(44f, 64f, 86f)
private val duskOpacities = floatArrayOf(0.32f, 0.18f, 0.09f)

private fun pathOf(svg: String): Path = PathParser().parsePathString(svg).toPath()

/** The only mutable geometry. Size changes within a drag don't invalidate either set of endpoints. */
private class PanoramaPaths(inner: Boolean) {
    private var width = -1f
    private var scene: PhoneLandscape? = null
    private val innerPane = inner
    private val hills = Array(3) { Path() }
    private val travelled = Path()
    private val remaining = Path()
    private var cloud: Path? = null
    private var ambientMoon: Path? = null
    private var duskMoon: Path? = null

    fun geometry(width: Float): PhoneLandscape {
        scene?.takeIf { this.width == width }?.let { return it }
        return phoneLandscape(width, innerPane).also {
            this.width = width
            scene = it
            if (cloud == null) {
                cloud = pathOf(it.cloud)
                ambientMoon = pathOf(it.ambientMoon)
                duskMoon = pathOf(it.duskMoon)
            }
        }
    }

    fun DrawScope.hills(g: PhoneLandscape, a: PhonePanorama, b: PhonePanorama, w: Float, h: Float,
        p: Float, reduced: Boolean, far: Color, near: Color, ridgeAlpha: Float, alpha: Float,
    ) {
        for (layer in hills.indices) {
            val path = hills[layer]
            val t = HeaderChoreography.parallax(p, layerLag[layer], reduced)
            val top = h - mix(a.hills[layer].depth, b.hills[layer].depth, t)
            val amp = mix(a.hills[layer].amplitude, b.hills[layer].amplitude, t)
            val pts = g.hillControls[layer]
            path.rewind()
            path.moveTo(0f, h)
            path.lineTo(0f, top)
            for (i in 0..6 step 3) {
                val c1 = pts[i]; val c2 = pts[i + 1]; val end = pts[i + 2]
                path.cubicTo(w * c1.x, top + amp * c1.y, w * c2.x, top + amp * c2.y, w * end.x, top + amp * end.y)
            }
            path.lineTo(w, h)
            path.close()
            drawPath(path, when (layer) { 0 -> far.fade(alpha * ridgeAlpha); 1 -> far.fade(alpha); else -> near.fade(alpha) })
        }
    }

    /** Sample the arc from a fixed sine table, with the exact animated endpoint for the sun. */
    fun DrawScope.arc(pad: Float, w: Float, base: Float, peak: Float, progress: Float,
        track: Color, ink: Color, dusk: Boolean,
    ): Offset {
        val inset = minOf(pad, w * 0.25f)
        val span = w - 2f * inset
        val rise = base - peak
        val center = Offset(inset + span * progress, base - rise * sin(PI * progress).toFloat())
        val split = (progress * ARC_STEPS).toInt()
        if (!dusk && progress < 1f) {
            remaining.rewind()
            remaining.moveTo(center.x, center.y)
            for (i in split + 1..ARC_STEPS) remaining.lineTo(inset + span * i / ARC_STEPS, base - rise * arcSines[i])
            drawPath(remaining, track, style = arcLine)
        }
        if (progress > 0f) {
            travelled.rewind()
            travelled.moveTo(inset, base)
            for (i in 1..split) travelled.lineTo(inset + span * i / ARC_STEPS, base - rise * arcSines[i])
            travelled.lineTo(center.x, center.y)
            drawPath(travelled, ink, style = if (dusk) duskLine else travelledLine)
        }
        return center
    }

    fun DrawScope.cloud(x: Float, y: Float, w: Float, h: Float, fill: Color) {
        val p = cloud ?: return
        translate(x, y) { scale(w / 100f, h / 100f, Offset.Zero) { drawPath(p, fill) } }
    }

    fun DrawScope.moon(x: Float, y: Float, fill: Color, dusk: Boolean) {
        val p = (if (dusk) duskMoon else ambientMoon) ?: return
        translate(x, y) { drawPath(p, fill) }
    }
}

/** Reduced motion snaps geometry but gives the extra atmospheric details their short opacity fade. */
private object KeepFade : MotionDurationScale { override val scaleFactor: Float = 1f }

@Composable
internal fun HeaderPanorama(
    design: Design, hour: Int, inner: Boolean, collapse: HeaderCollapse,
    showsSun: Boolean, progress: () -> Float, dusk: () -> DuskLook,
) {
    val paths = remember(inner, design.dark) { PanoramaPaths(inner) }
    val r = design.room
    val dc = design.dusk()
    val time = sky(design.dark, hour)
    val arcInk = remember(design.dark, design.roomHue) {
        Color(0xff000000L or phoneArcColor(design.dark, design.roomHue).toLong())
    }
    val reduced = app.todoornot.ui.reducedMotion()
    val reachFade = remember(collapse) { Animatable(0f) }
    if (reduced) LaunchedEffect(collapse) {
        withContext(KeepFade) {
            snapshotFlow { HeaderChoreography.reachFade(collapse.reachFraction) }.collectLatest { target ->
                reachFade.animateTo(target, tween(Tokens.Motion.REDUCED_FADE))
            }
        }
    }
    Canvas(Modifier.fillMaxWidth().clearAndSetSemantics {}.graphicsLayer {
        val phase = collapse.fraction
        alpha = 1f - HeaderChoreography.scene(phase)
        scaleY = HeaderChoreography.sceneScale(phase, reduced)
        transformOrigin = TransformOrigin(0.5f, 1f)
    }) {
        val w = size.width / density
        val h = size.height / density
        val g = paths.geometry(w)
        val p = collapse.reachFraction
        val extra = if (reduced) reachFade.value else HeaderChoreography.reachFade(p)
        val look = dusk()
        val day = 1f - look.scene
        val night = look.scene
        val sunProgress = progress()
        scale(density, density, Offset.Zero) {
            with(paths) {
                if (day > 0f) {
                    if (design.dark) {
                        for (i in g.stars.indices) {
                            val s = g.stars[i]
                            drawCircle(r.star.fade(L.STAR_OPACITY * day), s.radius * mix(1f, 1.15f, p),
                                Offset(s.center.x * w, s.center.y * h * mix(0.46f, 0.54f, p)))
                        }
                        if (extra > 0f) drawReachStars(w, h, r.star, day * extra)
                        val moonAlpha = if (time == Sky.Night) 1f else extra
                        if (moonAlpha > 0f) moon(w * 0.84f, mix(23f, 34f, p), r.star.fade(L.AMBIENT_MOON_OPACITY * moonAlpha * day), false)
                    } else {
                        val primary = if (time == Sky.Twilight) 0.28f else L.CLOUD_OPACITY
                        val secondary = if (time == Sky.Twilight) 0.28f else mix(0.34f, 0.38f, p)
                        cloud(w * mix(0.78f, 0.76f, p), mix(24f, 46f, p), mix(78f, 98f, p), mix(16f, 22f, p), r.star.fade(primary * day))
                        cloud(w * mix(0.20f, 0.23f, p), mix(20f, 68f, p), mix(56f, 76f, p), mix(12f, 17f, p), r.star.fade(secondary * day))
                        if (extra > 0f) cloud(w * 0.54f, 26f, 62f, 13f, r.star.fade(L.ZENITH_STAR_OPACITY * extra * day))
                    }
                    if (showsSun) {
                        val a = g.rest; val b = g.reach
                        val center = arc(a.pad, w, h - mix(a.baseDepth, b.baseDepth, p), mix(a.peak, b.peak, p),
                            sunProgress, arcInk.fade(day), r.sunInk.fade(day), false)
                        val ring = if (reduced || p > 0.20f) extra else 0f
                        drawSun(center, r.sun.fade(day), HeaderChoreography.sunScale(p, reduced), false, ring)
                    }
                    hills(g, g.rest, g.reach, w, h, p, reduced, r.hillFar, r.hillNear,
                        if (design.dark) L.RIDGE_OPACITY_DARK else L.RIDGE_OPACITY_LIGHT, day)
                }
                if (night > 0f) {
                    for (i in g.stars.indices) {
                        val s = g.stars[i]
                        val alpha = night * look.night(i % nightBatches) * L.STAR_OPACITY
                        if (alpha > 0f) drawCircle(dc.star.fade(alpha), s.radius * 1.35f,
                            Offset(s.center.x * w, s.center.y * h * 0.56f))
                    }
                    if (extra > 0f) drawReachStars(w, h, dc.star, night * look.night * extra)
                    moon(w * 0.16f, mix(56f, 76f, p), dc.star.fade(L.MOON_OPACITY * night * look.night), true)
                    val a = g.dusk; val b = g.duskReach
                    val base = h - mix(a.baseDepth, b.baseDepth, p)
                    val peak = mix(a.peak, b.peak, p)
                    // Keep the gold arc pinned to the same endpoint as the day travels into it.
                    val end = arc(a.pad, w, base, peak, 1f, Color.Transparent, dc.sun.fade(night * 0.5f), true)
                    // The setting sun's size belongs to the resting dusk scene, not to reach motion.
                    drawSun(end + Offset(-6f, 10f), dc.sun.fade(night), 2.2f, true, 0f)
                    hills(g, a, b, w, h, p, reduced, dc.silhouetteFar, dc.silhouetteNear, L.RIDGE_OPACITY_DUSK, night)
                }
            }
        }
    }
}

private fun DrawScope.drawReachStars(w: Float, h: Float, color: Color, fade: Float) {
    val fill = color.fade(0.58f * fade)
    for (s in reachStars) drawCircle(fill, s.radius, Offset(w * s.x, h * s.y))
}

private fun DrawScope.drawSun(center: Offset, ink: Color, scale: Float, dusk: Boolean, extra: Float) {
    if (dusk) {
        for (i in duskRadii.indices) drawCircle(ink.fade(duskOpacities[i]), duskRadii[i], center, style = ringLine)
    } else {
        drawCircle(ink.fade(L.OUTER_GLOW_OPACITY), 24f * scale, center)
        if (extra > 0f) drawCircle(ink.fade(0.14f * extra), 36f * scale, center, style = reachRing)
    }
    drawCircle(ink.fade(L.HALO_OPACITY), (if (dusk) 11f else 13.5f) * scale, center)
    val inner = (if (dusk) 8.5f else 9.5f) * scale
    val outer = (if (dusk) 11.5f else 13f) * scale
    for (i in rayX.indices) {
        val x = rayX[i]; val y = rayY[i]
        drawLine(ink, center + Offset(x * inner, y * inner), center + Offset(x * outer, y * outer),
            L.RAY_STROKE, StrokeCap.Round)
    }
    drawCircle(ink, (if (dusk) 6.2f else 7f) * scale, center)
}
