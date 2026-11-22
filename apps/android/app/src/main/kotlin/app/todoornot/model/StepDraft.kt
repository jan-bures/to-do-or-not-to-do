package app.todoornot.model

import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.saveable.Saver
import androidx.compose.runtime.saveable.listSaver
import androidx.compose.runtime.setValue

/**
 * A task's "+ Add step" draft. It lives in [TodoModel] (one per task, see [Actions.stepDraft]), which
 * outlives the Activity: a draft sent just before a rotation is the same draft after it, still marked
 * as sending, and the write's answer lands on it. One submission is in flight at a time, so Done
 * pressed again, before or after a recreation, never adds a step twice. The draft is kept until the step
 * is added; a refused one keeps it, with the reason, and can be sent again.
 */
class StepDraft(val task: String) {
    var text by mutableStateOf("")
        private set

    /** Why the last submission was refused, until the draft is edited. */
    var problem by mutableStateOf<String?>(null)
        private set

    /** A submission is on its way. */
    var sending by mutableStateOf(false)
        private set

    /** Edited, sent or restored in this process: the model's draft is then the truth, not a saved copy. */
    private var used = false

    fun edit(value: String) {
        used = true
        text = value.replace("\n", "")
        problem = null
    }

    /**
     * After process death only the field's saved copy survives, and the model is new: its text comes
     * back if it was typed for this task (saved state is restored by position, so another task's pane
     * can be handed it), and nothing is marked as sending. A draft already used in this process ignores
     * it: after a rotation the saved copy may be older than the draft (a step added while the Activity
     * was being recreated).
     */
    fun restore(saved: SavedStep) {
        if (!used && saved.task == task && saved.text.isNotEmpty()) edit(saved.text)
    }

    /** Sends the trimmed draft through [send], which calls back with null once added, else the reason. */
    fun submit(send: (title: String, done: (String?) -> Unit) -> Unit) {
        val t = text.trim()
        if (t.isEmpty() || sending) return
        sending = true
        send(t) { error ->
            sending = false
            problem = error
            // Clear only what was sent, so anything typed meanwhile stays.
            if (error == null && text.trim() == t) text = ""
        }
    }
}

/** What the step field saves for after process death: the task it was typed for, and the text. */
data class SavedStep(val task: String, val text: String) {
    companion object {
        val NONE = SavedStep("", "")
        val Saver: Saver<SavedStep, Any> = listSaver(save = { listOf(it.task, it.text) }, restore = { SavedStep(it[0], it[1]) })
    }
}
