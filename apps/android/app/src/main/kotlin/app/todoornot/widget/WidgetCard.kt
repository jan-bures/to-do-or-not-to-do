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
import androidx.glance.appwidget.lazy.LazyColumn
import androidx.glance.appwidget.lazy.items
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
private val leadingIconInset = S.S2

// Widgets are glanceable and dense: rows and the header use the 40dp compact target instead of the app's 48dp
// touch height, so a typical 4x3 widget shows five tasks. Rows with a metadata line grow to fit it.
private val rowMin = S.S10
private val headerHeight = S.S8

internal fun widgetRowHeight(fontScale: Float, hasMeta: Boolean = true): Float {
    val lines = T.TASK_TITLE.lineHeight + if (hasMeta) T.COUNT.lineHeight else 0f
    val content = lines * fontScale + S.S2
    return maxOf(rowMin, ceil(content / S.S1) * S.S1)
}

private fun WidgetRow.meta() = listOfNotNull(list?.label, due?.text).joinToString(" · ")
private fun WidgetRow.hasMeta() = meta().isNotEmpty()

// Keep the hash out of Glance's reserved range for automatically generated IDs.
private fun WidgetRow.itemId() = id.fold(0L) { hash, char -> 31 * hash + char.code } and Long.MAX_VALUE

@Composable
@GlanceComposable
internal fun WidgetCard(data: WidgetData, clicks: WidgetClicks? = null, darkOverride: Boolean? = null) {
    val width = LocalSize.current.width.value
    val compact = width < 5 * touch
    val design = WidgetPalette((data as? WidgetData.Room)?.item?.hue ?: "my-day", darkOverride)
    val card = GlanceModifier.fillMaxSize().appWidgetBackground().background(
        ImageProvider(R.drawable.todo_widget_card), colorFilter = ColorFilter.tint(design.room { it.pane }),
    )
    val inset = if (compact) S.S2 else S.S3
    // The header and rows already centre their content, so the card only needs a small vertical padding; more
    // would push the list down and leave a tall band above the first row.
    val padded = card.padding(horizontal = inset.dp, vertical = S.S2.dp)
    Column(if (data is WidgetData.Room) padded else card) {
        if (data is WidgetData.Room) {
            // The narrowest launcher sizes need a compact toolbar to leave room for the name.
            val showSettings = width >= 6 * touch + S.S3
            val actionWidth = if (compact) S.S10 else touch
            Header(data, clicks, design, showSettings, actionWidth)
            if (data.rows.isEmpty()) {
                Message(data.empty, design, italic = true, inset = leadingIconInset)
            } else {
                val fontScale = LocalContext.current.resources.configuration.fontScale
                LazyColumn(GlanceModifier.fillMaxWidth().defaultWeight()) {
                    items(data.rows, itemId = { it.itemId() }) {
                        TaskLine(it, widgetRowHeight(fontScale, it.hasMeta()), design, clicks)
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
private fun Header(
    data: WidgetData.Room, clicks: WidgetClicks?, palette: WidgetPalette, showSettings: Boolean, actionWidth: Float,
) {
    val item = data.item
    val ink = palette.room { it.ink3 }
    val pick = clicks?.pick()
    Row(GlanceModifier.fillMaxWidth().height(headerHeight.dp), verticalAlignment = Alignment.CenterVertically) {
        Row(
            GlanceModifier.defaultWeight().height(headerHeight.dp).onClick(pick).padding(start = leadingIconInset.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            val glyph = item.glyph ?: item.icon
            if (glyph != null) {
                WidgetIcon(glyph, palette.glyph(item.hue), size = C.GLYPH, handDrawn = item.glyph != null)
                Spacer(GlanceModifier.width(S.S1.dp))
            }
            WidgetText(item.name, T.NAV, palette.room { it.accentStrong }, GlanceModifier.defaultWeight())
        }
        WidgetButton("chevron-down", "Choose list or collection", ink, pick, width = actionWidth, height = headerHeight)
        WidgetButton("refresh-cw", "Refresh", ink, clicks?.refresh(), width = actionWidth, height = headerHeight)
        WidgetButton("plus", "Add task", ink, clicks?.add(item.key), width = actionWidth, height = headerHeight)
        if (showSettings) WidgetButton("settings", "Widget settings", ink, pick, width = actionWidth, height = headerHeight)
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
        Box(
            GlanceModifier.width(touch.dp).height(height.dp).onClick(clicks?.complete(row.id)).padding(start = leadingIconInset.dp),
            contentAlignment = Alignment.CenterStart,
        ) {
            WidgetIcon("checkbox", ring, label = "Complete ${row.title}", size = C.CHECKBOX)
        }
        Column(
            GlanceModifier.defaultWeight().height(height.dp).onClick(clicks?.task(row.id)),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            WidgetText(row.title, T.TASK_TITLE, palette.room { it.ink }, GlanceModifier.fillMaxWidth())
            val meta = row.meta()
            if (meta.isNotEmpty()) WidgetText(meta, T.COUNT, metaInk, GlanceModifier.fillMaxWidth())
        }
        WidgetButton(
            "star", if (row.important) "Remove importance from ${row.title}" else "Mark ${row.title} as important",
            if (row.important) palette.room { it.accentStrong } else palette.room { it.ink3 },
            clicks?.important(row.id, !row.important), filled = row.important, height = height,
        )
    }
}

@Composable
@GlanceComposable
private fun Message(
    text: String, palette: WidgetPalette, action: Action? = null, italic: Boolean = false, linked: Boolean = false,
    inset: Float = S.S3,
) {
    Row(
        GlanceModifier.fillMaxWidth().height(touch.dp).onClick(action).padding(horizontal = inset.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        WidgetText(text, T.BODY, palette.room { if (linked) it.accentStrong else it.ink3 }, lines = 2, italic = italic)
    }
}

private fun GlanceModifier.onClick(action: Action?) = if (action == null) this else clickable(action)

@Composable
@GlanceComposable
private fun WidgetButton(
    key: String, label: String, tint: ColorProvider, action: Action?, filled: Boolean = false, width: Float = touch,
    height: Float = touch,
) {
    Box(GlanceModifier.width(width.dp).height(height.dp).onClick(action), contentAlignment = Alignment.Center) {
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
            // Glance can only name system families; Geist and Geist Mono map to their nearest ones.
            fontFamily = when (token.role) {
                FontRole.MONO -> FontFamily.Monospace
                FontRole.UI -> FontFamily.SansSerif
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
                icon.paths.forEach {
                    paint.style = if (filled || it.filled) Paint.Style.FILL_AND_STROKE else Paint.Style.STROKE
                    canvas.drawPath(PathParser.createPathFromPathData(it.d), paint)
                }
            }
        }
        widgetIcons.put(cacheKey, bitmap)
        bitmap
    }
    Image(ImageProvider(image), label, GlanceModifier.size(size.dp), colorFilter = ColorFilter.tint(tint))
}
