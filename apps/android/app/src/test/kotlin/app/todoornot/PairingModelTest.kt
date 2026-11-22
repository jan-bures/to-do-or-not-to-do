package app.todoornot

import android.content.SharedPreferences
import app.todoornot.core.DeviceInviteTarget
import app.todoornot.core.FfiException
import app.todoornot.core.PairPoll
import app.todoornot.core.PairStarted
import app.todoornot.model.PairingApi
import app.todoornot.model.PairingModel
import app.todoornot.model.PairingState
import app.todoornot.session.KeyValues
import app.todoornot.session.PrefsKeyValues
import app.todoornot.session.SecretCipher
import app.todoornot.session.Session
import app.todoornot.session.SessionPersistenceException
import app.todoornot.session.SessionStore
import java.lang.reflect.Proxy
import java.security.GeneralSecurityException
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.Job
import kotlinx.coroutines.cancel
import kotlinx.coroutines.test.StandardTestDispatcher
import kotlinx.coroutines.test.TestScope
import kotlinx.coroutines.test.advanceUntilIdle
import kotlinx.coroutines.test.runTest
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

private class FakeApi(private val answers: ArrayDeque<() -> PairPoll>) : PairingApi {
    var polls = 0
    var startedWith: String? = null

    override fun normalize(input: String): String {
        if (!input.startsWith("https://")) throw FfiException.BadAddress("Use an https:// address.")
        return input.trimEnd('/')
    }

    override fun start(server: String, deviceName: String): PairStarted {
        startedWith = deviceName
        return PairStarted("WXYZ-2345", "poll-secret", "$server/pair?code=WXYZ-2345", 600u, 2u)
    }

    override fun poll(server: String, pollToken: String): PairPoll {
        polls++
        return answers.removeFirst()()
    }
}

@OptIn(ExperimentalCoroutinesApi::class)
class PairingModelTest {
    private fun TestScope.model(api: PairingApi, paired: MutableList<Session>): PairingModel {
        val io = StandardTestDispatcher(testScheduler)
        return PairingModel(api, this, io, "Test phone", { paired += it }, clock = { testScheduler.currentTime })
    }

    @Test
    fun an_approved_code_pairs() = runTest {
        val api = FakeApi(ArrayDeque(listOf({ PairPoll.Pending(2u) }, { PairPoll.Approved("device-9", "tdd_token") })))
        val paired = mutableListOf<Session>()
        val m = model(api, paired)
        m.submit("https://todo.example.com/")
        testScheduler.runCurrent()
        val waiting = m.state.value as PairingState.Waiting
        assertEquals("WXYZ-2345", waiting.userCode)
        assertEquals("https://todo.example.com/pair?code=WXYZ-2345", waiting.approvalUrl)
        advanceUntilIdle()
        assertEquals(listOf(Session("https://todo.example.com", "device-9", "tdd_token")), paired)
        assertEquals("Test phone", api.startedWith)
        assertEquals(2, api.polls)
    }

    @Test
    fun a_bad_address_is_explained_without_a_request() = runTest {
        val api = FakeApi(ArrayDeque())
        val m = model(api, mutableListOf())
        m.submit("todo.example.com")
        advanceUntilIdle()
        val s = m.state.value as PairingState.Entering
        assertEquals("todo.example.com", s.address)
        assertEquals("Use an https:// address.", s.error)
        assertNull(api.startedWith)
    }

    @Test
    fun a_denied_code_goes_back_with_a_reason() = runTest {
        val m = model(FakeApi(ArrayDeque(listOf({ PairPoll.Denied }))), mutableListOf())
        m.submit("https://todo.example.com")
        advanceUntilIdle()
        assertEquals(PairingModel.DENIED, (m.state.value as PairingState.Entering).error)
    }

    @Test
    fun network_trouble_keeps_polling_and_says_so() = runTest {
        val paired = mutableListOf<Session>()
        val m = model(
            FakeApi(ArrayDeque(listOf({ throw FfiException.Network("down") }, { PairPoll.Approved("d", "tdd_t") }))),
            paired,
        )
        m.submit("https://todo.example.com")
        testScheduler.advanceTimeBy(2_001)
        testScheduler.runCurrent()
        assertTrue((m.state.value as PairingState.Waiting).retrying)
        advanceUntilIdle()
        assertEquals(1, paired.size)
    }

    @Test
    fun the_code_expires() = runTest {
        val pending: () -> PairPoll = { PairPoll.Pending(2u) }
        val m = model(FakeApi(ArrayDeque(List(1000) { pending })), mutableListOf())
        m.submit("https://todo.example.com")
        advanceUntilIdle()
        assertEquals(PairingModel.EXPIRED, (m.state.value as PairingState.Entering).error)
    }

    @Test
    fun a_link_host_needs_confirmation_and_redemption_persists_after_cancellation() = runTest {
        val server = "https://todo.example.com"
        val sent = CompletableDeferred<String>()
        val reply = CompletableDeferred<Session>()
        val persisted = mutableListOf<Session>()
        val dispatcher = StandardTestDispatcher(testScheduler)
        val owner = CoroutineScope(Job() + dispatcher)
        val api = object : PairingApi by FakeApi(ArrayDeque()) {
            override fun inviteTarget(address: String, invite: String) = DeviceInviteTarget(server, "todo.example.com")
            override suspend fun redeem(server: String, invite: String, deviceName: String): Session {
                sent.complete(server)
                return reply.await()
            }
        }
        val m = PairingModel(api, owner, dispatcher, "Phone", { persisted += it })
        m.redeem("", "link")
        testScheduler.runCurrent()
        assertEquals(PairingState.ConfirmInvite("todo.example.com"), m.state.value)
        assertTrue(!sent.isCompleted)
        m.confirmInvite()
        testScheduler.runCurrent()
        assertEquals(server, sent.await())
        m.cancel()
        owner.cancel()
        assertEquals(PairingState.Finishing(server), m.state.value)
        assertTrue(persisted.isEmpty())
        val credential = Session(server, "phone", "device-token")
        reply.complete(credential)
        advanceUntilIdle()
        assertEquals(listOf(credential), persisted)
    }

    @Test
    fun an_encryption_failure_keeps_the_credential_for_save_only_retry() = runTest {
        var fail = true
        val cipher = object : SecretCipher {
            override fun encrypt(plain: ByteArray): ByteArray {
                if (fail) throw GeneralSecurityException("Keystore locked")
                return plain
            }
            override fun decrypt(sealed: ByteArray) = sealed
        }
        val durableValues = mutableMapOf<String, String?>()
        val prefs = object : KeyValues {
            override fun get(key: String) = durableValues[key]
            override fun put(values: Map<String, String?>): Boolean {
                durableValues.putAll(values)
                return true
            }
        }
        checkSaveRetry(SessionStore(prefs, cipher)) { fail = false }
    }

    @Test
    fun a_false_preferences_commit_keeps_the_credential_for_save_only_retry() = runTest {
        var canCommit = false
        var writes = 0
        val visible = mutableMapOf<String, String?>()
        val durable = mutableMapOf<String, String?>()
        val pending = mutableMapOf<String, String?>()
        val editor = proxy<SharedPreferences.Editor> { receiver, name, args ->
            when (name) {
                "putString" -> receiver.also { pending[args[0] as String] = args[1] as String? }
                "remove" -> receiver.also { pending[args[0] as String] = null }
                "commit" -> canCommit.also {
                    visible.putAll(pending)
                    if (it) durable.putAll(pending)
                    pending.clear()
                    writes++
                }
                else -> error("Unexpected editor method $name")
            }
        }
        val prefs = proxy<SharedPreferences> { _, name, args ->
            when (name) {
                "getString" -> visible[args[0]] ?: args[1]
                "edit" -> editor
                else -> error("Unexpected preferences method $name")
            }
        }
        val cipher = object : SecretCipher {
            override fun encrypt(plain: ByteArray) = plain
            override fun decrypt(sealed: ByteArray) = sealed
        }
        checkSaveRetry(SessionStore(PrefsKeyValues(prefs), cipher)) {
            assertEquals(2, writes) // Attempt, then rollback of SharedPreferences' memory state.
            assertTrue(durable.isEmpty())
            canCommit = true
        }
        assertEquals(3, writes)
        assertEquals("phone", durable["device_id"])
    }

    @Test
    fun discarding_a_failed_save_reopens_pairing_without_retrying_the_old_credential() = runTest {
        val server = "https://todo.example.com"
        val credential = Session(server, "phone", "unused")
        var redemptions = 0
        var canSave = false
        val saves = mutableListOf<Session>()
        val api = object : PairingApi by FakeApi(ArrayDeque()) {
            override suspend fun redeem(server: String, invite: String, deviceName: String): Session {
                redemptions++
                return credential.copy(deviceId = invite)
            }
        }
        val m = PairingModel(api, this, StandardTestDispatcher(testScheduler), "Phone", {
            if (!canSave) throw SessionPersistenceException.Write()
            saves.add(it)
        })
        m.redeem(server, "old-device")
        testScheduler.runCurrent()
        assertEquals(PairingState.SaveFailed, m.state.value)
        m.discardSave()
        assertEquals(PairingState.Entering(server, PairingModel.DISCARDED), m.state.value)
        assertTrue(PairingModel.DISCARDED.contains("Check Devices"))
        canSave = true
        m.retrySave()
        assertTrue(saves.isEmpty())
        m.redeem(server, "new-device")
        advanceUntilIdle()
        assertEquals(listOf(credential.copy(deviceId = "new-device")), saves)
        assertEquals(2, redemptions)
    }

    private fun TestScope.checkSaveRetry(store: SessionStore, repair: () -> Unit) {
        val server = "https://todo.example.com"
        val expected = Session(server, "phone", "device-token")
        var redemptions = 0
        val api = object : PairingApi by FakeApi(ArrayDeque()) {
            override suspend fun redeem(server: String, invite: String, deviceName: String): Session {
                redemptions++
                return expected
            }
        }
        val m = PairingModel(api, this, StandardTestDispatcher(testScheduler), "Phone", store::save)
        m.redeem(server, "test-input")
        testScheduler.runCurrent()
        assertEquals(PairingState.SaveFailed, m.state.value)
        assertNull(store.load())
        m.cancel()
        m.redeem(server, "other-input")
        assertEquals(PairingState.SaveFailed, m.state.value)
        repair()
        m.retrySave()
        assertEquals(expected, store.load())
        assertEquals(1, redemptions)
    }

    private inline fun <reified T : Any> proxy(crossinline call: (Any, String, Array<out Any?>) -> Any?): T =
        Proxy.newProxyInstance(T::class.java.classLoader, arrayOf(T::class.java)) { receiver, method, args ->
            call(receiver, method.name, args ?: emptyArray())
        } as T

    @Test
    fun cancelling_keeps_the_address() = runTest {
        val m = model(FakeApi(ArrayDeque(listOf({ PairPoll.Pending(2u) }))), mutableListOf())
        m.submit("https://todo.example.com")
        testScheduler.runCurrent()
        m.cancel()
        assertEquals(PairingState.Entering("https://todo.example.com"), m.state.value)
    }
}
