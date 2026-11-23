package app.todoornot

import app.todoornot.core.FfiException
import app.todoornot.core.PushOptions
import app.todoornot.push.PushStatus
import app.todoornot.push.retireFirebase
import app.todoornot.push.registerForPush
import app.todoornot.push.syncTickle
import java.time.Instant
import java.time.ZoneOffset
import com.google.android.gms.tasks.Tasks
import java.io.IOException
import java.util.concurrent.ExecutionException
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.tasks.await
import kotlinx.coroutines.test.runTest
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class PushTest {
    @Test
    fun only_data_only_sync_hints_are_accepted() {
        val data = mapOf("t" to "sync", "seq" to "71")
        assertEquals(71uL, syncTickle(data, false))
        assertNull(syncTickle(data, true))
        assertNull(syncTickle(emptyMap(), false))
        assertNull(syncTickle(data + ("t" to "other"), false))
        for (invalid in listOf("0", "-1", "no", "18446744073709551616")) {
            assertNull(syncTickle(data + ("seq" to invalid), false))
        }
    }

    @Test
    fun unconfigured_never_initializes_firebase_and_new_tokens_replace_previous_ones() = runTest {
        val registered = mutableListOf<String>()
        assertEquals(PushStatus.State.NOT_CONFIGURED, registerForPush({ true }, { null }, { error("not configured") }, registered::add))
        val options = PushOptions("project", "application", "key", "sender")
        for (token in listOf("first", "second")) {
            assertEquals(PushStatus.State.ON, registerForPush({ true }, { options }, {
                assertEquals(options, it)
                token
            }, registered::add))
        }
        assertEquals(listOf("first", "second"), registered)
    }

    @Test
    fun a_pairing_changed_during_token_retrieval_cannot_upload_to_the_old_server() = runTest {
        var paired = true
        val result = registerForPush({ paired }, { PushOptions("project", "application", "key", "sender") }, {
            paired = false
            "old"
        }, { error("pairing was closed") })
        assertEquals(PushStatus.State.OFF, result)
        assertEquals(PushStatus.State.OFF, registerForPush({ false }, { error("no pairing") }, { null }, {}))
    }

    @Test
    fun a_collision_renews_the_local_firebase_token_without_reasserting_the_old_one() = runTest {
        val attempted = mutableListOf<String>()
        val state = registerForPush({ true }, { PushOptions("project", "application", "key", "sender") },
            token = { "old" }, register = {
                attempted += it
                if (it == "old") throw FfiException.Refused(status = 409u, code = "push_token_in_use", detail = "conflict", retryAfterSecs = null)
            }, renew = { "new" })
        assertEquals(PushStatus.State.ON, state)
        assertEquals(listOf("old", "new"), attempted)
    }

    @Test
    fun failed_firebase_delete_task_does_not_abort_local_cleanup_but_cancellation_propagates() = runTest {
        val task = Tasks.forException<Void>(ExecutionException(IOException("token deletion failed")))
        var failures = 0
        var localCleanup = 0
        retireFirebase(deleteToken = { task.await() }, clear = { localCleanup++ }, failed = { failures++ })
        assertEquals(1, failures)
        assertEquals(1, localCleanup)
        val cancelled = CancellationException("work stopped")
        try {
            retireFirebase(deleteToken = { throw cancelled }, clear = { localCleanup++ }, failed = { failures++ })
            error("cancellation was swallowed")
        } catch (e: CancellationException) {
            assertEquals(cancelled, e)
        }
        assertEquals(1, failures)
        assertEquals(2, localCleanup)
    }

    @Test
    fun last_wake_is_reported_in_the_phones_time_zone() {
        val status = PushStatus(PushStatus.State.ON, Instant.parse("2026-09-05T15:10:00Z"))
        assertTrue(status.label(ZoneOffset.ofHours(2)).endsWith("17:10"))
        assertEquals("Push: Off", PushStatus().label())
        assertEquals("Push: Not configured", PushStatus(PushStatus.State.NOT_CONFIGURED).label())
    }
}
