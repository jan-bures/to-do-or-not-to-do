package app.todoornot.widget

import android.graphics.Bitmap
import android.graphics.Canvas
import android.graphics.Paint
import android.util.LruCache
import androidx.compose.runtime.Composable
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.core.graphics.PathParser
import androidx.glance.GlanceComposable
import androidx.glance.GlanceModifier
import androidx.glance.Image
import androidx.glance.ImageProvider
import androidx.glance.LocalContext
import androidx.glance.LocalSize
import androidx.glance.ColorFilter
import androidx.glance.action.Action
import androidx.glance.action.clickable
import androidx.glance.appwidget.appWidgetBackground
import androidx.glance.background
import androidx.glance.color.ColorProvider
import androidx.glance.layout.Alignment
import androidx.glance.layout.Box
import androidx.glance.layout.Column
import androidx.glance.layout.Row
import androidx.glance.layout.Spacer
import androidx.glance.layout.fillMaxSize
import androidx.glance.layout.fillMaxWidth
import androidx.glance.layout.height
import androidx.glance.layout.padding
import androidx.glance.layout.size
import androidx.glance.layout.width
import androidx.glance.text.FontFamily
import androidx.glance.text.FontStyle
import androidx.glance.text.FontWeight
import androidx.glance.text.Text
import androidx.glance.text.TextStyle
import androidx.glance.unit.ColorProvider
import app.todoornot.R
import app.todoornot.core.DueTone
import app.todoornot.design.Design
import app.todoornot.design.FontRole
import app.todoornot.design.IconPaths
import app.todoornot.design.TextToken
import app.todoornot.design.Tokens
import kotlin.math.ceil
import kotlin.math.roundToInt

private val S = Tokens.Space
private val T = Tokens.Type
private val C = Tokens.Metrics.Component
private val touch = Tokens.Metrics.Phone.TOUCH

internal data class WidgetFit(val rowHeight: Float, val footerHeight: Float, val visible: Int, val more: Int)

internal fun widgetFit(height: Float, fontScale: Float, total: Int): WidgetFit {
    fun grid(value: Float) = ceil(value / S.S1) * S.S1
    val row = maxOf(touch, grid((T.TASK_TITLE.lineHeight + T.COUNT.lineHeight) * fontScale + S.S2))
    val footer = maxOf(S.S6, grid(T.COUNT.lineHeight * fontScale + S.S1))
    val body = (height - touch).coerceAtLeast(0f)
    val slots = (body / row).toInt()
    val visible = if (total <= slots) total else ((body - footer) / row).toInt().coerceAtLeast(0)
    return WidgetFit(row, footer, visible, total - visible)
}

@Composable
@GlanceComposable
internal fun WidgetCard(data: WidgetData, clicks: WidgetClicks? = null, darkOverride: Boolean? = null) {
    val size = LocalSize.current
    val design = WidgetPalette((data as? WidgetData.Room)?.item?.hue ?: "my-day", darkOverride)
    Column(
        GlanceModifier.fillMaxSize().appWidgetBackground().background(
            ImageProvider(R.drawable.todo_widget_card), colorFilter = ColorFilter.tint(design.room { it.pane }),
        ),
    ) {
        if (data is WidgetData.Room) {
            Header(data, clicks, design, showSettings = size.width.value >= 6 * touch)
            if (data.rows.isEmpty()) {
                Message(data.empty, design, italic = true)
            } else {
                val fit = widgetFit(size.height.value, LocalContext.current.resources.configuration.fontScale, data.rows.size)
                // A Glance column accepts ten children. Chunk only the layout, not the room's ordering.
                TaskRows(data.rows.take(fit.visible), fit.rowHeight, design, clicks)
                if (fit.more > 0) {
                    Row(
                        GlanceModifier.fillMaxWidth().height(fit.footerHeight.dp).padding(horizontal = S.S3.dp),
                        verticalAlignment = Alignment.CenterVertically,
                    ) {
                        WidgetText("+${fit.more} more", T.COUNT, design.room { it.ink3 })
                    }
                }
            }
        } else {
            Row(
                GlanceModifier.fillMaxWidth().height(touch.dp).padding(horizontal = S.S3.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                WidgetText("To Do or Not to Do", T.NAV, design.room { it.ink }, GlanceModifier.defaultWeight())
            }
            val (message, action) = when (data) {
                WidgetData.SignedOut -> "Open the app to sign in" to clicks?.app()
                WidgetData.Unavailable -> "Open the app to load your tasks" to clicks?.app()
                WidgetData.Unconfigured -> "Choose a list or collection" to clicks?.pick()
                is WidgetData.Missing -> (if (data.collection) "Collection deleted · Pick another" else "List deleted · Pick another") to clicks?.pick()
                is WidgetData.Room -> error("Handled above")
            }
            Message(message, design, action, linked = true)
        }
    }
}

@Composable
@GlanceComposable
private fun TaskRows(rows: List<WidgetRow>, height: Float, palette: WidgetPalette, clicks: WidgetClicks?) {
    Column {
        if (rows.size <= 10) {
            rows.forEach { TaskLine(it, height, palette, clicks) }
        } else {
            // Real launcher sizes need at most a few chunks, but don't drop rows if a larger host fits them.
            val chunk = maxOf(10, (rows.size + 9) / 10)
            rows.chunked(chunk).forEach { TaskRows(it, height, palette, clicks) }
        }
    }
}

private class WidgetPalette(hue: String, private val darkOverride: Boolean?) {
    private val light = Design(false, hue)
    private val dark = Design(true, hue)

    private fun color(value: (Design) -> Color): ColorProvider = when (darkOverride) {
        true -> ColorProvider(value(dark))
        false -> ColorProvider(value(light))
        null -> ColorProvider(day = value(light), night = value(dark))
    }

    fun room(value: (app.todoornot.design.RoomColors) -> Color) = color { value(it.room) }
    fun glyph(hue: String) = color { it.list(hue).glyph }
}

@Composable
@GlanceComposable
private fun Header(data: WidgetData.Room, clicks: WidgetClicks?, palette: WidgetPalette, showSettings: Boolean) {
    val item = data.item
    val ink = palette.room { it.ink3 }
    Row(GlanceModifier.fillMaxWidth().height(touch.dp), verticalAlignment = Alignment.CenterVertically) {
        Row(
            GlanceModifier.defaultWeight().height(touch.dp).onClick(clicks?.pick()).padding(start = S.S2.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            val glyph = item.glyph ?: item.icon
            if (glyph != null) {
                WidgetIcon(glyph, palette.glyph(item.hue), size = C.GLYPH, handDrawn = item.glyph != null)
                Spacer(GlanceModifier.width(S.S1.dp))
            }
            WidgetText(item.name, T.NAV, palette.room { it.accentStrong }, GlanceModifier.defaultWeight())
            WidgetIcon("chevron-down", ink, size = C.ICON_SMALL, label = "Choose list or collection")
        }
        WidgetButton("refresh-cw", "Refresh", ink, clicks?.refresh())
        WidgetButton("plus", "Add task", ink, clicks?.add(item.key))
        if (showSettings) WidgetButton("settings", "Widget settings", ink, clicks?.pick())
    }
}

@Composable
@GlanceComposable
private fun TaskLine(row: WidgetRow, height: Float, palette: WidgetPalette, clicks: WidgetClicks?) {
    val overdue = row.due?.tone == DueTone.OVERDUE
    val ring = if (overdue) palette.room { it.danger } else palette.room { it.control }
    val metaInk = when (row.due?.tone) {
        DueTone.OVERDUE -> palette.room { it.dangerInk }
        DueTone.SOON -> palette.room { it.accentStrong }
        else -> palette.room { it.ink3 }
    }
    Row(GlanceModifier.fillMaxWidth().height(height.dp), verticalAlignment = Alignment.CenterVertically) {
        Box(GlanceModifier.size(touch.dp).onClick(clicks?.complete(row.id)), contentAlignment = Alignment.Center) {
            WidgetIcon("checkbox", ring, label = "Complete ${row.title}", size = C.CHECKBOX)
        }
        Column(
            GlanceModifier.defaultWeight().height(height.dp).onClick(clicks?.task(row.id)),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            WidgetText(row.title, T.TASK_TITLE, palette.room { it.ink }, GlanceModifier.fillMaxWidth())
            val meta = listOfNotNull(row.list?.label, row.due?.text).joinToString(" · ")
            if (meta.isNotEmpty()) WidgetText(meta, T.COUNT, metaInk, GlanceModifier.fillMaxWidth())
        }
        WidgetButton(
            "star", if (row.important) "Remove importance from ${row.title}" else "Mark ${row.title} as important",
            if (row.important) palette.room { it.sunInk } else palette.room { it.ink3 },
            clicks?.important(row.id, !row.important), filled = row.important,
        )
    }
}

@Composable
@GlanceComposable
private fun Message(text: String, palette: WidgetPalette, action: Action? = null, italic: Boolean = false, linked: Boolean = false) {
    Row(
        GlanceModifier.fillMaxWidth().height(touch.dp).onClick(action).padding(horizontal = S.S3.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        WidgetText(text, T.BODY, palette.room { if (linked) it.accentStrong else it.ink3 }, lines = 2, italic = italic)
    }
}

private fun GlanceModifier.onClick(action: Action?) = if (action == null) this else clickable(action)

@Composable
@GlanceComposable
private fun WidgetButton(key: String, label: String, tint: ColorProvider, action: Action?, filled: Boolean = false) {
    Box(GlanceModifier.size(touch.dp).onClick(action), contentAlignment = Alignment.Center) {
        WidgetIcon(key, tint, label = label, filled = filled)
    }
}

@Composable
@GlanceComposable
private fun WidgetText(
    text: String,
    token: TextToken,
    ink: ColorProvider,
    modifier: GlanceModifier = GlanceModifier,
    lines: Int = 1,
    italic: Boolean = token.italic,
) {
    Text(
        text.replace('\n', ' ').replace('\r', ' ').replace('\u2028', ' ').replace('\u2029', ' '),
        modifier,
        maxLines = lines,
        style = TextStyle(
            color = ink,
            fontSize = token.size.sp,
            fontWeight = if (token.weight >= 500) FontWeight.Medium else FontWeight.Normal,
            fontStyle = if (italic) FontStyle.Italic else FontStyle.Normal,
            fontFamily = when {
                italic || token.role == FontRole.DISPLAY || token.role == FontRole.DISPLAY_TEXT -> FontFamily.Serif
                token.role == FontRole.MONO -> FontFamily.Monospace
                else -> FontFamily.SansSerif
            },
        ),
    )
}

private data class IconKey(val key: String, val pixels: Int, val handDrawn: Boolean, val filled: Boolean)

// Reuse the same bitmap in multiple rows so repeated controls aren't copied into RemoteViews.
private val widgetIcons = LruCache<IconKey, Bitmap>(64)

/** RemoteViews can't draw Compose vectors. Rasterize the very same generated paths as the drawer. */
@Composable
@GlanceComposable
private fun WidgetIcon(
    key: String, tint: ColorProvider, label: String? = null, size: Float = C.ICON,
    handDrawn: Boolean = false, filled: Boolean = false,
) {
    val pixels = (size * LocalContext.current.resources.displayMetrics.density).roundToInt().coerceAtLeast(1)
    val cacheKey = IconKey(key, pixels, handDrawn, filled)
    val image = widgetIcons.get(cacheKey) ?: run {
        val bitmap = Bitmap.createBitmap(pixels, pixels, Bitmap.Config.ARGB_8888)
        val canvas = Canvas(bitmap)
        val paint = Paint(Paint.ANTI_ALIAS_FLAG).apply {
            color = android.graphics.Color.BLACK // alpha mask; the visible colour always comes from tokens
            style = if (filled) Paint.Style.FILL_AND_STROKE else Paint.Style.STROKE
            strokeCap = Paint.Cap.ROUND
            strokeJoin = Paint.Join.ROUND
        }
        if (key == "checkbox") {
            val radius = pixels / 2f
            paint.strokeWidth = C.CHECKBOX_RING * pixels / size
            canvas.drawCircle(radius, radius, radius - paint.strokeWidth / 2f, paint)
        } else {
            val icon = if (handDrawn) IconPaths.glyphs[key] else IconPaths.lucide[key]
            if (icon != null) {
                canvas.scale(pixels / 24f, pixels / 24f) // SVG viewBox, not a layout measure
                paint.strokeWidth = icon.strokeWidth
                icon.paths.forEach { canvas.drawPath(PathParser.createPathFromPathData(it), paint) }
            }
        }
        widgetIcons.put(cacheKey, bitmap)
        bitmap
    }
    Image(ImageProvider(image), label, GlanceModifier.size(size.dp), colorFilter = ColorFilter.tint(tint))
}
