package app.todoornot

import app.todoornot.core.FfiException
import app.todoornot.core.PairPoll
import app.todoornot.core.PairStarted
import app.todoornot.model.PairingApi
import app.todoornot.model.PairingModel
import app.todoornot.model.PairingState
import app.todoornot.session.Session
import kotlinx.coroutines.ExperimentalCoroutinesApi
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
    fun cancelling_keeps_the_address() = runTest {
        val m = model(FakeApi(ArrayDeque(listOf({ PairPoll.Pending(2u) }))), mutableListOf())
        m.submit("https://todo.example.com")
        testScheduler.runCurrent()
        m.cancel()
        assertEquals(PairingState.Entering("https://todo.example.com"), m.state.value)
    }
}
