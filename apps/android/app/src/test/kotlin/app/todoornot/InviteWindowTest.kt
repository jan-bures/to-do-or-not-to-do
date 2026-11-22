package app.todoornot

import android.app.Activity
import android.app.Application
import android.view.WindowManager.LayoutParams.FLAG_SECURE
import app.todoornot.ui.protectInviteWindow
import org.junit.Assert.assertEquals
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.Robolectric
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config

@RunWith(RobolectricTestRunner::class)
@Config(application = Application::class, sdk = [35])
class InviteWindowTest {
    @Test
    fun the_invite_secures_the_task_window_until_disposal() {
        val controller = Robolectric.buildActivity(Activity::class.java).setup()
        try {
            val window = controller.get().window
            val original = window.attributes.flags
            val restore = protectInviteWindow(window)
            assertEquals(FLAG_SECURE, window.attributes.flags and FLAG_SECURE)
            restore()
            assertEquals(original, window.attributes.flags)

            window.addFlags(FLAG_SECURE)
            protectInviteWindow(window)()
            assertEquals(FLAG_SECURE, window.attributes.flags and FLAG_SECURE)
        } finally {
            controller.pause().stop().destroy()
        }
    }
}
