package app.todoornot

import app.todoornot.core.PushOptions
import app.todoornot.push.PushStatus
import app.todoornot.push.registerForPush
import app.todoornot.push.syncTickle
import java.time.Instant
import java.time.ZoneOffset
import kotlinx.coroutines.test.runTest
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class PushTest {
    @Test
    fun only_data_only_sync_hints_are_accepted() {
        val data = mapOf("t" to "sync", "seq" to "71")
        assertTrue(syncTickle(data, false))
        assertFalse(syncTickle(data, true))
        assertFalse(syncTickle(emptyMap(), false))
        assertFalse(syncTickle(data + ("t" to "other"), false))
        for (invalid in listOf("0", "-1", "no", "18446744073709551616")) {
            assertFalse(syncTickle(data + ("seq" to invalid), false))
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
    fun last_wake_is_reported_in_the_phones_time_zone() {
        val status = PushStatus(PushStatus.State.ON, Instant.parse("2026-09-05T15:10:00Z"))
        assertTrue(status.label(ZoneOffset.ofHours(2)).endsWith("17:10"))
        assertEquals("Push: Off", PushStatus().label())
        assertEquals("Push: Not configured", PushStatus(PushStatus.State.NOT_CONFIGURED).label())
    }
}
