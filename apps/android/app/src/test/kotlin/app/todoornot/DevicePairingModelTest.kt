package app.todoornot

import app.todoornot.core.DeviceInvitation
import app.todoornot.core.DeviceInviteStatus
import app.todoornot.core.DevicePairingReview
import app.todoornot.core.FfiException
import app.todoornot.model.DevicePairingApi
import app.todoornot.model.DevicePairingModel
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.test.runTest
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

@OptIn(ExperimentalCoroutinesApi::class)
class DevicePairingModelTest {
    @Test
    fun a_missing_status_after_resume_past_retention_is_unknown_not_expired() = runTest {
        val issued = DeviceInvitation("invite-id", "ABCDEF-GHJKMN", "https://todo.example.com/pair/invite#fixture", 3u)
        val reply = CompletableDeferred<DeviceInviteStatus>()
        val api = object : DevicePairingApi {
            override val pollMs = 1_000L
            override suspend fun create() = issued
            override suspend fun status(id: String) = reply.await()
            override suspend fun cancel(id: String) = error("not expected")
            override suspend fun review(code: String, approveAt: Long?): DevicePairingReview = error("not expected")
        }
        var suspendedMs = 0L
        val m = DevicePairingModel(api, this) { testScheduler.currentTime + suspendedMs }
        m.create()
        testScheduler.advanceTimeBy(1_001)
        suspendedMs = 65_000
        reply.completeExceptionally(FfiException.Expired())
        testScheduler.runCurrent()
        assertEquals("Couldn’t confirm whether the device joined. Check Devices.", m.status)
        assertNull(m.invite)
    }

    @Test
    fun credentials_disappear_at_the_deadline_while_status_is_in_flight() = runTest {
        val issued = DeviceInvitation("invite-id", "ABCDEF-GHJKMN", "https://todo.example.com/pair/invite#fixture", 3u)
        val reply = CompletableDeferred<DeviceInviteStatus>()
        val calls = mutableListOf<String>()
        val api = object : DevicePairingApi {
            override val pollMs = 1_000L
            override suspend fun create() = issued
            override suspend fun status(id: String): DeviceInviteStatus {
                calls += id
                return reply.await()
            }
            override suspend fun cancel(id: String) = error("not expected")
            override suspend fun review(code: String, approveAt: Long?): DevicePairingReview = error("not expected")
        }
        val m = DevicePairingModel(api, this) { testScheduler.currentTime }
        m.create()
        testScheduler.runCurrent()
        assertEquals(issued, m.invite)
        testScheduler.advanceTimeBy(3_000)
        testScheduler.runCurrent()
        assertEquals(listOf(issued.id), calls)
        assertEquals(0L, m.left)
        assertNull(m.invite)
        assertTrue(m.checking)
        reply.complete(DeviceInviteStatus.Used("Last-second phone"))
        testScheduler.runCurrent()
        assertEquals("Last-second phone signed in.", m.status)
        assertTrue(!m.checking)
    }
}
