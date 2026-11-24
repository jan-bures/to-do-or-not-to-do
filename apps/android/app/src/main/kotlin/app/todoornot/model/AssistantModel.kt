package app.todoornot.model

import app.todoornot.core.AndroidAssistant
import app.todoornot.core.AssistantAccess
import app.todoornot.core.AssistantAccessRow
import app.todoornot.core.AssistantPanelState
import app.todoornot.core.AssistantProposal
import app.todoornot.core.AssistantServer
import app.todoornot.core.ChangeListener
import app.todoornot.core.FfiException
import app.todoornot.core.LocalNow
import java.io.Closeable
import kotlinx.coroutines.CoroutineDispatcher
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock

internal interface AssistantCore : Closeable {
    fun listen(onChange: () -> Unit)
    fun panel(now: LocalNow): AssistantPanelState
    fun resume()
    fun pause()
    fun shutdown()
    fun save(server: AssistantServer?)
    fun test()
    fun access(row: AssistantAccessRow, access: AssistantAccess)
    fun send(generation: ULong, sequence: ULong, prompt: String, now: LocalNow)
    fun stop()
    fun newChat()
    fun select(proposal: AssistantProposal, indices: List<UInt>)
    fun reject(proposal: AssistantProposal)
    fun approve(proposal: AssistantProposal)
    fun undo(proposal: AssistantProposal)
}

internal class NativeAssistant(private val native: AndroidAssistant) : AssistantCore {
    override fun listen(onChange: () -> Unit) = native.listen(object : ChangeListener {
        override fun onChange() = onChange()
    })
    override fun panel(now: LocalNow) = native.panel(now)
    override fun resume() = native.resume()
    override fun pause() = native.pause()
    override fun shutdown() = native.shutdown()
    override fun close() = native.close()
    override fun save(server: AssistantServer?) = native.saveServer(server)
    override fun test() = native.testServer()
    override fun access(row: AssistantAccessRow, access: AssistantAccess) = native.setAccess(row.id, row.collection, access)
    override fun send(generation: ULong, sequence: ULong, prompt: String, now: LocalNow) =
        native.send(generation, sequence, prompt, now)
    override fun stop() = native.stop()
    override fun newChat() = native.newChat()
    override fun select(proposal: AssistantProposal, indices: List<UInt>) = native.selectItems(proposal.id, proposal.digest, indices)
    override fun reject(proposal: AssistantProposal) = native.reject(proposal.id, proposal.digest)
    override fun approve(proposal: AssistantProposal) = native.approve(proposal.id, proposal.digest)
    override fun undo(proposal: AssistantProposal) = native.undo(proposal.id)
}

internal data class AssistantUi(
    val panel: AssistantPanelState,
    val draft: String = "",
    val busy: Boolean = false,
    val problem: String? = null,
)

/** Only editor state and deliberate gestures belong here. Rust owns scope, streaming and review. */
internal class AssistantModel(
    private val core: AssistantCore,
    private val scope: CoroutineScope,
    private val io: CoroutineDispatcher,
    private val now: () -> LocalNow,
) {
    private val _ui = MutableStateFlow(AssistantUi(core.panel(now())))
    val ui = _ui.asStateFlow()
    private val changes = Channel<Unit>(Channel.CONFLATED)
    private val serial = Mutex()
    private var sequence = 0uL
    @Volatile private var foreground = false
    @Volatile private var closed = false
    private var resumed = false

    init {
        core.listen { changes.trySend(Unit) }
        scope.launch(io) {
            for (ignored in changes) serial.withLock {
                if (!closed) {
                    followLifecycle()
                    publish()
                }
            }
        }
        changes.trySend(Unit)
    }

    fun foreground(active: Boolean) {
        foreground = active
        changes.trySend(Unit)
    }

    /** Revoke live inference before deleting the pairing directory. No new dispatch can start. */
    fun shutdown() {
        closed = true
        changes.close()
        core.shutdown()
    }

    /** Release the foreign handle after any refresh or gesture already holding it has returned. */
    suspend fun close() {
        serial.withLock { shutdown(); core.close() }
    }

    private fun followLifecycle() {
        val active = foreground
        if (active != resumed) {
            if (active) core.resume() else core.pause()
            resumed = active
        }
    }

    private fun publish() {
        val latest = core.panel(now())
        _ui.update { old ->
            if (old.panel.generation != latest.generation) old.copy(panel = latest, draft = "", problem = null)
            else old.copy(panel = latest)
        }
    }

    fun draft(text: String) { _ui.update { it.copy(draft = text, problem = null) } }

    private fun act(action: () -> Unit) {
        if (!foreground || closed) return
        // Consume the gesture before dispatching to IO, so a double tap cannot send or approve twice.
        while (true) {
            val before = _ui.value
            if (before.busy) return
            if (_ui.compareAndSet(before, before.copy(busy = true, problem = null))) break
        }
        scope.launch(io) {
            serial.withLock {
                if (!closed) {
                    followLifecycle()
                    if (foreground) {
                        val problem = try {
                            action()
                            null
                        } catch (error: FfiException.Invalid) {
                            error.detail
                        } catch (_: FfiException) {
                            "Couldn't complete that assistant action."
                        }
                        publish()
                        _ui.update { it.copy(problem = problem) }
                    }
                }
                _ui.update { it.copy(busy = false) }
            }
        }
    }

    fun send() {
        val shown = _ui.value
        send(shown, shown.draft, clearDraft = true)
    }

    fun retry(turnId: String) {
        val shown = _ui.value
        val turn = shown.panel.turns.lastOrNull() ?: return
        if (shown.draft.isBlank() && turn.id == turnId && turn.problem != null) send(shown, turn.prompt, clearDraft = false)
    }

    private fun send(shown: AssistantUi, prompt: String, clearDraft: Boolean) {
        if (prompt.isBlank() || shown.panel.streaming || shown.panel.server == null) return
        act {
            core.send(shown.panel.generation, ++sequence, prompt, now())
            if (clearDraft) _ui.update { if (it.draft == prompt) it.copy(draft = "") else it }
        }
    }

    fun stop() = act(core::stop)
    fun newChat() = act(core::newChat)
    fun save(server: AssistantServer?) = act { core.save(server) }
    fun test() = act(core::test)
    fun access(row: AssistantAccessRow, access: AssistantAccess) = act { core.access(row, access) }
    fun select(proposal: AssistantProposal, indices: List<UInt>) = act { core.select(proposal, indices) }
    fun reject(proposal: AssistantProposal) = act { core.reject(proposal) }
    fun approve(proposal: AssistantProposal) = act { core.approve(proposal) }
    fun undo(proposal: AssistantProposal) = act { core.undo(proposal) }
}
