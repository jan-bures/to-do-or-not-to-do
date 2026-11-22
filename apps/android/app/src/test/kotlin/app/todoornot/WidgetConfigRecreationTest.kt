package app.todoornot

import android.app.Activity
import android.app.Application
import android.appwidget.AppWidgetManager
import android.content.Intent
import android.os.Bundle
import android.os.Looper
import androidx.activity.ComponentActivity
import app.todoornot.core.RoomKey
import app.todoornot.widget.WidgetChoicesStore
import app.todoornot.widget.WidgetConfigCommit
import app.todoornot.widget.followWidgetConfig
import kotlinx.coroutines.CompletableDeferred
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.Robolectric
import org.robolectric.RobolectricTestRunner
import org.robolectric.Shadows.shadowOf
import org.robolectric.annotation.Config

@RunWith(RobolectricTestRunner::class)
@Config(application = Application::class, sdk = [35])
class WidgetConfigRecreationTest {
    @Test
    fun the_replacement_activity_accepts_a_committed_selection_and_back_cannot_cancel_it() {
        val controller = Robolectric.buildActivity(ConfigResultActivity::class.java).setup()
        val original = controller.get()
        val store = WidgetChoicesStore(original.applicationContext)
        val chosen = RoomKey.List("another-list")
        val returnFromWrite = CompletableDeferred<Unit>()
        original.commit.save {
            check(store.put(WIDGET_ID, chosen))
            returnFromWrite.await()
            true
        }
        val main = shadowOf(Looper.getMainLooper())
        main.idle()
        assertEquals(chosen, store.get(WIDGET_ID))
        assertEquals(Activity.RESULT_CANCELED, shadowOf(original).resultCode)

        controller.recreate()
        val replacement = controller.get()
        replacement.onBackPressedDispatcher.onBackPressed()
        assertFalse(replacement.isFinishing)

        returnFromWrite.complete(Unit)
        main.idle()
        assertEquals(Activity.RESULT_CANCELED, shadowOf(original).resultCode)
        assertEquals(Activity.RESULT_OK, shadowOf(replacement).resultCode)
        assertEquals(WIDGET_ID, shadowOf(replacement).resultIntent.getIntExtra(AppWidgetManager.EXTRA_APPWIDGET_ID, -1))
        assertEquals(chosen, store.get(WIDGET_ID))
    }
}

private const val WIDGET_ID = 20

// Exercise the real activity binding without initializing the Android (rather than host) Rust library.
class ConfigResultActivity : ComponentActivity() {
    internal lateinit var commit: WidgetConfigCommit

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        val result = Intent().putExtra(AppWidgetManager.EXTRA_APPWIDGET_ID, WIDGET_ID)
        setResult(RESULT_CANCELED, result)
        commit = followWidgetConfig(result) {}
    }
}
