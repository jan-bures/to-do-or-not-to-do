package app.todoornot.ui

import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.saveable.Saver
import androidx.compose.runtime.setValue

/**
 * The "+ Add step" field's draft. One submission is in flight at a time: Done pressed again while the
 * first is still on its way does nothing, so a step is never added twice. The draft is kept until the
 * step is added; a refused one keeps it, with the reason, and can be sent again.
 */
class StepDraft(text: String = "") {
    var text by mutableStateOf(text)
        private set

    /** Why the last submission was refused, until the draft is edited. */
    var problem by mutableStateOf<String?>(null)
        private set

    /** A submission is on its way. */
    var sending by mutableStateOf(false)
        private set

    fun edit(value: String) {
        text = value.replace("\n", "")
        problem = null
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

    companion object {
        /** Only the text survives a restart; nothing is in flight in a new process. */
        val Saver: Saver<StepDraft, String> = Saver(save = { it.text }, restore = { StepDraft(it) })
    }
}
