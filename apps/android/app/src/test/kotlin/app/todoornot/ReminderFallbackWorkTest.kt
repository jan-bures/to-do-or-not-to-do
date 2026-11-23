package app.todoornot

import android.app.Application
import androidx.work.NetworkType
import androidx.work.WorkInfo
import androidx.work.WorkManager
import androidx.work.testing.WorkManagerTestInitHelper
import app.todoornot.push.PushWork
import java.util.concurrent.TimeUnit
import org.junit.Assert.assertEquals
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.RuntimeEnvironment
import org.robolectric.annotation.Config

@RunWith(RobolectricTestRunner::class)
@Config(application = Application::class, sdk = [35])
class ReminderFallbackWorkTest {
    @Test
    fun a_new_pairing_reuses_the_installation_schedule_if_sign_out_cleanup_never_ran() {
        val context = RuntimeEnvironment.getApplication()
        WorkManagerTestInitHelper.initializeTestWorkManager(context)
        val manager = WorkManager.getInstance(context)
        fun fallback(): List<WorkInfo> = manager.getWorkInfosForUniqueWork(PushWork.FALLBACK_WORK).get(10, TimeUnit.SECONDS)
        fun schedule() { PushWork.scheduleFallback(manager).result.get(10, TimeUnit.SECONDS) }

        schedule() // Old pairing.
        val original = fallback().single()
        assertEquals(NetworkType.CONNECTED, original.constraints.requiredNetworkType)
        schedule() // New pairing, after a crash before cancellation could run.
        assertEquals(original.id, fallback().single().id)
        PushWork.cancelFallback(context).result.get(10, TimeUnit.SECONDS)
        assertEquals(WorkInfo.State.CANCELLED, fallback().single().state)
    }
}
