package app.todoornot

import app.todoornot.core.AssistantAccess
import app.todoornot.core.AssistantAccessRow
import app.todoornot.core.AssistantConnection
import app.todoornot.core.AssistantPanelState
import app.todoornot.core.AssistantProposal
import app.todoornot.core.AssistantProposalStatus
import app.todoornot.core.AssistantServer
import app.todoornot.core.FfiException
import app.todoornot.core.LocalNow
import app.todoornot.model.AssistantCore
import app.todoornot.model.AssistantModel
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.test.StandardTestDispatcher
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
        var state = AssistantPanelState(3uL, AssistantServer("https://todo.example.com", "test-model", true),
            AssistantConnection.UNTESTED, "Private network", null, emptyList(), 0u, false, emptyList(), null)
        val prompts = mutableListOf<Triple<ULong, ULong, String>>()
        var review: AssistantProposal? = null
        var rejection: AssistantProposal? = null
        var failed = false
        var foreground = false
        var closed = false
        override fun listen(onChange: () -> Unit) { changed = onChange }
        override fun panel(now: LocalNow) = state
        override fun resume() { foreground = true }
        override fun pause() { foreground = false; stop() }
        override fun shutdown() { foreground = false }
        override fun save(server: AssistantServer?) { replace(state.copy(server = server)) }
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
