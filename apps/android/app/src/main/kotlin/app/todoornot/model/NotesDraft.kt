package app.todoornot.model

import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue

/** One task's in-place editor. Null means reading, while an empty string is an edit that clears notes. */
class NotesDraft(val task: String) {
    var text by mutableStateOf<String?>(null)
        private set
    var saving by mutableStateOf(false)
        private set
    var problem by mutableStateOf<String?>(null)
        private set

    fun edit(value: String?) {
        if (saving) return
        text = value
        problem = null
    }

    fun save(actions: Actions) {
        val value = text ?: return
        if (saving) return
        saving = true
        actions.setNotes(task, value) { error ->
            saving = false
            problem = error
            if (error == null) text = null
        }
    }
}
