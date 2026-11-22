package app.todoornot.design

import androidx.compose.foundation.Image
import androidx.compose.runtime.Composable
import androidx.compose.runtime.remember
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.ColorFilter
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.graphics.StrokeJoin
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.graphics.vector.PathParser
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.foundation.layout.size

private val cache = HashMap<String, ImageVector>()

private fun vector(key: String, data: IconData): ImageVector = synchronized(cache) {
    cache.getOrPut(key) {
        val b = ImageVector.Builder(key, 24.dp, 24.dp, 24f, 24f)
        for (d in data.paths) {
            b.addPath(
                pathData = PathParser().parsePathString(d.d).toNodes(),
                fill = if (d.filled) SolidColor(Color.Black) else null,
                stroke = SolidColor(Color.Black),
                strokeLineWidth = data.strokeWidth,
                strokeLineCap = StrokeCap.Round,
                strokeLineJoin = StrokeJoin.Round,
            )
        }
        b.build()
    }
}

/** A Lucide icon by key (`"sun"`, `"plus"`), tinted like text. 18 dp in rows and bars, 14 in chips. */
@Composable
fun Icon(key: String, tint: Color, modifier: Modifier = Modifier, size: Dp = Tokens.Metrics.Component.ICON.dp, label: String? = null) {
    val data = IconPaths.lucide[key] ?: return
    StrokeImage("lucide/$key", data, tint, size, label, modifier)
}

/** A hand-drawn list glyph by key (`"house"`), in the list's glyph colour. 16 dp. */
@Composable
fun Glyph(key: String, tint: Color, modifier: Modifier = Modifier, size: Dp = Tokens.Metrics.Component.GLYPH.dp) {
    val data = IconPaths.glyphs[key] ?: return
    StrokeImage("glyph/$key", data, tint, size, null, modifier)
}

@Composable
private fun StrokeImage(key: String, data: IconData, tint: Color, size: Dp, label: String?, modifier: Modifier) {
    val v = remember(key) { vector(key, data) }
    Image(
        imageVector = v,
        contentDescription = null,
        colorFilter = ColorFilter.tint(tint),
        modifier = modifier
            .size(size)
            .then(if (label != null) Modifier.semantics { contentDescription = label } else Modifier),
    )
}
