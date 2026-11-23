package app.todoornot

import android.appwidget.AppWidgetHostView
import android.view.ViewGroup.LayoutParams.MATCH_PARENT
import android.widget.FrameLayout
import android.widget.ListView
import androidx.compose.ui.unit.DpSize
import androidx.compose.ui.unit.dp
import androidx.core.view.descendants
import androidx.glance.appwidget.ExperimentalGlanceRemoteViewsApi
import androidx.glance.appwidget.GlanceRemoteViews
import app.cash.paparazzi.DeviceConfig
import app.cash.paparazzi.Paparazzi
import app.todoornot.core.RoomKey
import app.todoornot.core.SmartList
import app.todoornot.widget.WIDGET_ROW_LIMIT
import app.todoornot.widget.WidgetCard
import app.todoornot.widget.WidgetData
import app.todoornot.widget.widgetData
import com.android.resources.Density
import com.android.resources.ScreenOrientation
import kotlinx.coroutines.runBlocking
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Rule
import org.junit.Test

/** Snapshot the real Glance composition, inflated as RemoteViews just as a launcher inflates it. */
@OptIn(ExperimentalGlanceRemoteViewsApi::class)
class WidgetGalleryTest {
    @get:Rule
    val paparazzi = Paparazzi(
        deviceConfig = DeviceConfig.PIXEL_5, theme = "android:Theme.Material.Light.NoActionBar", maxPercentDifference = 0.1,
    )

    private fun inflate(width: Int, height: Int, data: WidgetData, dark: Boolean, fontScale: Float): FrameLayout {
        val density = 2
        paparazzi.unsafeUpdateConfig(deviceConfig = DeviceConfig.PIXEL_5.copy(
            screenWidth = width * density, screenHeight = height * density,
            orientation = if (width > height) ScreenOrientation.LANDSCAPE else ScreenOrientation.PORTRAIT,
            xdpi = 160 * density, ydpi = 160 * density, density = Density.XHIGH, locale = "en-rUS", fontScale = fontScale,
        ))
        val context = paparazzi.context
        val views = runBlocking {
            GlanceRemoteViews().compose(context, DpSize(width.dp, height.dp)) { WidgetCard(data, darkOverride = dark) }.remoteViews
        }
        // Android only attaches a RemoteViews collection adapter inside an app-widget host.
        val parent = AppWidgetHostView(context)
        parent.addView(views.apply(context, parent), FrameLayout.LayoutParams(MATCH_PARENT, MATCH_PARENT))
        return parent
    }

    private fun FrameLayout.tasks(): ListView = descendants.filterIsInstance<ListView>().single()

    private fun themes(
        name: String, width: Int, height: Int, data: WidgetData, fontScale: Float = 1f, scrollToEnd: Boolean = false,
    ) {
        for (dark in listOf(false, true)) {
            val card = inflate(width, height, data, dark, fontScale)
            if (scrollToEnd) card.tasks().run { setSelection(adapter.count - 1) }
            paparazzi.snapshot(card, "${name}_${if (dark) "dark" else "light"}")
        }
    }

    @Test
    fun widget_gallery() = fixtureEngine().use { engine ->
        themes("small", 216, 132, widgetData(engine, RoomKey.Smart(SmartList.MY_DAY), FIXTURE_NOW))
        val collection = widgetData(engine, engine.collection("Personal"), FIXTURE_NOW)
        themes("medium_collection", 304, 232, collection)
        themes("medium_collection_end", 304, 232, collection, scrollToEnd = true)
        themes("large", 364, 404, widgetData(engine, RoomKey.Smart(SmartList.ALL), FIXTURE_NOW))
        themes("empty", 216, 132, widgetData(engine, engine.list("Reading list"), FIXTURE_NOW))
        themes("signed_out", 216, 132, WidgetData.SignedOut)
        themes("deleted", 216, 132, WidgetData.Missing(collection = true))
        val reading = engine.list("Reading list")
        engine.addTask(reading, "Vyřídit žádost o přeplatek na zdravotním pojištění a doložit potvrzení", FIXTURE_NOW)
        themes("large_text", 304, 152, widgetData(engine, reading, FIXTURE_NOW), fontScale = 1.5f)
    }

    @Test
    fun a_small_widget_registers_every_row_up_to_the_cap_with_stable_ids() = fixtureEngine().use { engine ->
        val room = engine.list("Reading list")
        repeat(WIDGET_ROW_LIMIT + 1) { engine.addTask(room, "Task ${it + 1}", FIXTURE_NOW) }
        val data = widgetData(engine, room, FIXTURE_NOW) as WidgetData.Room
        fun adapter(data: WidgetData.Room) = inflate(216, 132, data, dark = false, fontScale = 1f).tasks().adapter
        val items = adapter(data)
        assertEquals(WIDGET_ROW_LIMIT, items.count)
        assertTrue(items.hasStableIds())
        val ids = (0 until items.count).map(items::getItemId)
        assertEquals(items.count, ids.toSet().size)
        val reordered = adapter(data.copy(rows = data.rows.reversed()))
        assertEquals(ids.reversed(), (0 until reordered.count).map(reordered::getItemId))
    }
}
