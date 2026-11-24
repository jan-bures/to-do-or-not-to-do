package app.todoornot

import app.todoornot.core.AssistantAccess
import app.todoornot.core.AssistantAccessRow
import app.todoornot.core.AssistantConnection
import app.todoornot.core.AssistantDeviceCode
import app.todoornot.core.AssistantHosted
import app.todoornot.core.AssistantHostedAccount
import app.todoornot.core.AssistantPanelState
import app.todoornot.core.AssistantScopeReview
import app.todoornot.core.AssistantProposal
import app.todoornot.core.AssistantProposalStatus
import app.todoornot.core.AssistantServer
import app.todoornot.core.FfiException
import app.todoornot.core.LocalNow
import app.todoornot.model.AssistantCore
import app.todoornot.model.AssistantModel
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.test.StandardTestDispatcher
import kotlinx.coroutines.test.advanceTimeBy
import kotlinx.coroutines.test.runCurrent
import kotlinx.coroutines.test.runTest
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertSame
import org.junit.Assert.assertTrue
import org.junit.Test

@OptIn(ExperimentalCoroutinesApi::class)
class AssistantModelTest {
    private class Core : AssistantCore {
        private lateinit var changed: () -> Unit
        var state = AssistantPanelState(3uL, AssistantServer("http://127.0.0.1:11434", "test-model", false), false,
            AssistantConnection.UNTESTED, "This device", null, emptyList(), 0u, false, emptyList(), null, null)
        val prompts = mutableListOf<Triple<ULong, ULong, String>>()
        var review: AssistantProposal? = null
        var rejection: AssistantProposal? = null
        var failed = false
        var foreground = false
        var closed = false
        var shownNow: LocalNow? = null
        var scope: AssistantScopeReview? = null
        var approvedScope: AssistantScopeReview? = null
        var pendingPolls = 0
        var connected: AssistantHostedAccount? = null
        override fun listen(onChange: () -> Unit) { changed = onChange }
        override fun panel(now: LocalNow): AssistantPanelState { shownNow = now; return state }
        override fun resume() { foreground = true }
        override fun pause() { foreground = false; stop() }
        override fun shutdown() { foreground = false }
        override fun save(server: AssistantServer?, replacementKey: String?) { replace(state.copy(server = server)) }
        override fun test() = Unit
        override fun access(row: AssistantAccessRow, access: AssistantAccess) = newChat()
        override fun send(generation: ULong, sequence: ULong, prompt: String, now: LocalNow) {
            if (failed) throw FfiException.Invalid("The message is too long.")
            assertTrue(foreground)
            prompts.add(Triple(generation, sequence, prompt))
            replace(state.copy(streaming = true))
        }
        override fun stop() = replace(state.copy(streaming = false))
        override fun newChat() = replace(state.copy(generation = state.generation + 1uL, turns = emptyList(), streaming = false))
        override fun select(proposal: AssistantProposal, indices: List<UInt>) = Unit
        override fun reject(proposal: AssistantProposal) { rejection = proposal }
        override fun approve(proposal: AssistantProposal) { review = proposal }
        override fun undo(proposal: AssistantProposal) = Unit
        override fun close() { closed = true }
        override fun startChatgpt() = AssistantDeviceCode("fixture-attempt", "EXAMPLE-1", "https://auth.openai.com/codex/device")
        override fun pollChatgpt(attempt: String): AssistantHostedAccount? {
            pendingPolls++
            return connected
        }
        override fun scopeReview() = scope
        override fun approveScope(review: AssistantScopeReview) { approvedScope = review }
        fun replace(panel: AssistantPanelState) { state = panel; changed() }
    }

    @Test fun draftDoesNotSendAndDoubleTapOnlySendsOnce() = runTest {
        val core = Core()
        val model = AssistantModel(core, backgroundScope, StandardTestDispatcher(testScheduler)) { FIXTURE_NOW }
        model.foreground(true)
        model.draft("What’s due?")
        runCurrent()
        assertTrue(core.prompts.isEmpty())
        model.send()
        model.send()
        runCurrent()
        assertEquals(listOf(Triple(3uL, 1uL, "What’s due?")), core.prompts)
        assertTrue(model.ui.value.panel.streaming)
        assertEquals("", model.ui.value.draft)
        model.draft("Keep this draft")
        model.stop()
        runCurrent()
        assertFalse(model.ui.value.panel.streaming)
        assertEquals("Keep this draft", model.ui.value.draft)
    }

    @Test fun failedSendKeepsDraftButPrivacyResetClearsIt() = runTest {
        val core = Core()
        val model = AssistantModel(core, backgroundScope, StandardTestDispatcher(testScheduler)) { FIXTURE_NOW }
        model.foreground(true)
        model.draft("A draft")
        core.failed = true
        model.send()
        runCurrent()
        assertEquals("A draft", model.ui.value.draft)
        assertEquals("The message is too long.", model.ui.value.problem)
        core.newChat() // a scope reset arrives from Rust, without an editor gesture
        runCurrent()
        assertEquals("", model.ui.value.draft)
        assertEquals(null, model.ui.value.problem)
    }

    @Test fun dateRefreshUsesTheNewPreferenceWithoutResettingChatOrSending() = runTest {
        val core = Core()
        var clock = FIXTURE_NOW
        val model = AssistantModel(core, backgroundScope, StandardTestDispatcher(testScheduler)) { clock }
        model.foreground(true)
        model.draft("Keep this draft")
        runCurrent()
        clock = clock.copy(dateFormat = "numeric_us")
        model.refreshDateLabels()
        runCurrent()
        assertEquals("numeric_us", core.shownNow?.dateFormat)
        assertEquals(3uL, model.ui.value.panel.generation)
        assertEquals("Keep this draft", model.ui.value.draft)
        assertTrue(core.prompts.isEmpty())
    }

    @Test fun theDeviceCodeSurvivesOpeningTheBrowserAndOnlyPollsInForeground() = runTest {
        val core = Core()
        val model = AssistantModel(core, backgroundScope, StandardTestDispatcher(testScheduler)) { FIXTURE_NOW }
        model.foreground(true)
        model.startChatgpt()
        runCurrent()
        assertEquals("EXAMPLE-1", model.ui.value.code?.userCode)
        model.foreground(false)
        advanceTimeBy(2000)
        runCurrent()
        assertEquals(0, core.pendingPolls)
        assertEquals("EXAMPLE-1", model.ui.value.code?.userCode)
        model.foreground(true)
        advanceTimeBy(1100)
        runCurrent()
        assertEquals(1, core.pendingPolls)
        model.cancelChatgpt()
        runCurrent()
        assertEquals(null, model.ui.value.code)
    }

    @Test fun deviceSignInNamesTheNewAccountWithoutChangingSelection() = runTest {
        val core = Core()
        val existing = AssistantHostedAccount("first", "first@example.com")
        val added = AssistantHostedAccount("second", "second@example.com")
        core.state = core.state.copy(hosted = AssistantHosted(
            true, true, false, listOf(existing, added), existing.id, false, "sample-model", null))
        val model = AssistantModel(core, backgroundScope, StandardTestDispatcher(testScheduler)) { FIXTURE_NOW }
        model.foreground(true)
        model.startChatgpt()
        runCurrent()
        core.connected = added
        advanceTimeBy(1100)
        runCurrent()
        assertEquals(null, model.ui.value.code)
        assertEquals(added, model.ui.value.signedIn)
        assertEquals(existing.id, model.ui.value.panel.hosted?.selectedAccount)
        assertEquals("sample-model", model.ui.value.panel.hosted?.model)
    }

    @Test fun cloudSendWaitsForAnExplicitListScopeApproval() = runTest {
        val core = Core()
        core.state = core.state.copy(destination = "ChatGPT (cloud)",
            hosted = AssistantHosted(true, true, false, emptyList(), "fixture-account", false, "sample-model", null))
        val shown = AssistantScopeReview("fixture-account", "fixture-version", 3uL, listOf("fixture-list"), listOf("Inbox"))
        core.scope = shown
        val model = AssistantModel(core, backgroundScope, StandardTestDispatcher(testScheduler)) { FIXTURE_NOW }
        model.foreground(true)
        model.draft("Cloud message")
        model.send()
        runCurrent()
        assertSame(shown, model.ui.value.scopeReview)
        assertTrue(core.prompts.isEmpty())
        model.approveScope()
        runCurrent()
        assertSame(shown, core.approvedScope)
        assertEquals(listOf(Triple(3uL, 1uL, "Cloud message")), core.prompts)
    }

    @Test fun reviewUsesPresentedProposalAndBackgroundPreventsSending() = runTest {
        val core = Core()
        val model = AssistantModel(core, backgroundScope, StandardTestDispatcher(testScheduler)) { FIXTURE_NOW }
        val proposal = AssistantProposal("shown-id", ByteArray(32) { 8 }, AssistantProposalStatus.PENDING, null, emptyList())
        model.foreground(true)
        model.approve(proposal)
        runCurrent()
        assertSame(proposal, core.review)
        model.reject(proposal)
        runCurrent()
        assertSame(proposal, core.rejection)
        model.draft("Don't send in background")
        model.send()
        model.foreground(false)
        runCurrent()
        assertTrue(core.prompts.isEmpty())
        assertFalse(core.foreground)
        model.close()
        assertTrue(core.closed)
    }
}
