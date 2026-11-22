package app.todoornot

import android.Manifest
import android.app.Activity
import android.app.Application
import android.content.Intent
import android.os.Looper
import androidx.activity.ComponentActivity
import app.todoornot.model.PairingScan
import com.google.zxing.client.android.Intents
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotSame
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.Robolectric
import org.robolectric.RobolectricTestRunner
import org.robolectric.Shadows.shadowOf
import org.robolectric.annotation.Config

/** The process-owned scan survives the replacement of its Activity/result callback. */
class PairingScanTestActivity : ComponentActivity() {
    internal val scanner = PairingScanner(this) { requests }

    companion object {
        internal var requests = PairingScan()
    }
}

@RunWith(RobolectricTestRunner::class)
@Config(application = Application::class, sdk = [35])
class PairingScannerTest {
    @Test
    fun a_scan_started_before_recreation_is_delivered_once_afterwards() {
        PairingScanTestActivity.requests = PairingScan()
        val received = mutableListOf<String>()
        val controller = Robolectric.buildActivity(PairingScanTestActivity::class.java).setup()
        val first = controller.get()
        shadowOf(first.application).grantPermissions(Manifest.permission.CAMERA)
        first.scanner.launch({ received += it }, { error("permission should already be granted") })
        shadowOf(Looper.getMainLooper()).idle()
        val launch = shadowOf(first).nextStartedActivityForResult
        controller.recreate()
        val second = controller.get()
        assertNotSame(first, second)
        val reply = Intent().putExtra(Intents.Scan.RESULT, "ABCD-EFGH")
        second.activityResultRegistry.dispatchResult(launch.requestCode, Activity.RESULT_OK, reply)
        assertEquals(listOf("ABCD-EFGH"), received)
        controller.pause().stop().destroy()
    }
}
