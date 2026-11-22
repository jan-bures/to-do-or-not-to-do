package app.todoornot.model

import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue

/**
 * A task's "+ Add step" draft. It lives only in [TodoModel] (one per task, see [Actions.stepDraft]),
 * which outlives the Activity: a draft survives a rotation, and one sent just before it is the same
 * draft after it, still marked as sending, so the write's answer lands on the field on screen. It isn't
 * saved for after process death; a half-typed step is simply gone then. One submission is in flight at a
 * time, so Done pressed again never adds a step twice. The draft is kept until the step is added; a
 * refused one keeps it, with the reason, and can be sent again.
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
}
