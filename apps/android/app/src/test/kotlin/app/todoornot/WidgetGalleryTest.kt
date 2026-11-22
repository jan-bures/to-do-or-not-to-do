package app.todoornot

import android.view.ViewGroup.LayoutParams.MATCH_PARENT
import android.widget.FrameLayout
import androidx.compose.ui.unit.DpSize
import androidx.compose.ui.unit.dp
import androidx.glance.appwidget.ExperimentalGlanceRemoteViewsApi
import androidx.glance.appwidget.GlanceRemoteViews
import app.cash.paparazzi.DeviceConfig
import app.cash.paparazzi.Paparazzi
import app.todoornot.core.RoomKey
import app.todoornot.core.SmartList
import app.todoornot.widget.WidgetCard
import app.todoornot.widget.WidgetData
import app.todoornot.widget.widgetData
import com.android.resources.Density
import com.android.resources.ScreenOrientation
import kotlinx.coroutines.runBlocking
import org.junit.Rule
import org.junit.Test

/** Snapshot the real Glance composition, inflated as RemoteViews just as a launcher inflates it. */
@OptIn(ExperimentalGlanceRemoteViewsApi::class)
class WidgetGalleryTest {
    @get:Rule
    val paparazzi = Paparazzi(
        deviceConfig = DeviceConfig.PIXEL_5, theme = "android:Theme.Material.Light.NoActionBar", maxPercentDifference = 0.1,
    )

    private fun shot(name: String, width: Int, height: Int, data: WidgetData, dark: Boolean, fontScale: Float) {
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
        val parent = FrameLayout(context)
        parent.addView(views.apply(context, parent), FrameLayout.LayoutParams(MATCH_PARENT, MATCH_PARENT))
        paparazzi.snapshot(parent, name)
    }

    private fun themes(name: String, width: Int, height: Int, data: WidgetData, fontScale: Float = 1f) {
        shot("${name}_light", width, height, data, false, fontScale)
        shot("${name}_dark", width, height, data, true, fontScale)
    }

    @Test
    fun widget_gallery() = fixtureEngine().use { engine ->
        themes("small", 216, 132, widgetData(engine, RoomKey.Smart(SmartList.MY_DAY), FIXTURE_NOW))
        themes("medium_collection", 304, 232, widgetData(engine, engine.collection("Personal"), FIXTURE_NOW))
        themes("large", 364, 404, widgetData(engine, RoomKey.Smart(SmartList.ALL), FIXTURE_NOW))
        themes("empty", 216, 132, widgetData(engine, engine.list("Reading list"), FIXTURE_NOW))
        themes("signed_out", 216, 132, WidgetData.SignedOut)
        themes("deleted", 216, 132, WidgetData.Missing(collection = true))
        val reading = engine.list("Reading list")
        engine.addTask(reading, "Vyřídit žádost o přeplatek na zdravotním pojištění a doložit potvrzení", FIXTURE_NOW)
        themes("large_text", 304, 152, widgetData(engine, reading, FIXTURE_NOW), fontScale = 1.5f)
    }
}
