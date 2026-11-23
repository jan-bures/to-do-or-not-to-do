package app.todoornot.design

import androidx.compose.foundation.Canvas
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.runtime.Composable
import androidx.compose.runtime.remember
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clipToBounds
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.drawscope.scale
import androidx.compose.ui.graphics.vector.PathParser
import androidx.compose.ui.unit.dp
import app.todoornot.core.landscape

/** The legacy hills-only welcome strip: pairing has no room or progress. */
@Composable
internal fun PairingLandscape() {
    val d = LocalDesign.current
    val r = d.room
    val height = Tokens.Landscape.HEIGHT_COVER
    BoxWithConstraints(Modifier.fillMaxWidth().height(height.dp)) {
        val width = maxWidth.value
        val shapes = remember(width, d.dark) { landscape(width, height, false, 0u, 0u, d.dark) }
        val far = remember(shapes) { PathParser().parsePathString(shapes.farHill).toPath() }
        val near = remember(shapes) { PathParser().parsePathString(shapes.nearHill).toPath() }
        Canvas(Modifier.fillMaxSize().clipToBounds()) {
            scale(density, density, Offset.Zero) {
                if (d.dark) drawRect(r.horizon, Offset(0f, shapes.horizonTop), Size(width, height - shapes.horizonTop))
                for (star in shapes.stars) {
                    drawCircle(r.star.copy(alpha = shapes.starOpacity), star.radius, Offset(star.center.x, star.center.y))
                }
                drawPath(far, r.hillFar)
                drawPath(near, r.hillNear)
            }
        }
    }
}
