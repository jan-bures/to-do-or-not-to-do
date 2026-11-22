package app.todoornot.model

import app.todoornot.core.DestinationGroup
import app.todoornot.core.ReminderRef
import app.todoornot.core.RoomKey

/** What the main screen can ask for. [TodoModel] does the work; the screenshot gallery passes [None]. */
interface Actions {
    fun open(room: RoomKey)
    fun select(id: String?)
    fun add(title: String)
    fun previewDate(text: String): String?
    /**
     * Completes or reopens a task. A row tap and the detail pane pass the room [generation] they were read
     * in ([UiState.generation]); null (Undo) is the room on screen now. [row] says the tap was on the task's
     * row, which is inert while it's a ghost (the detail pane's checkbox isn't).
     */
    fun setDone(id: String, done: Boolean, generation: Long? = null, row: Boolean = generation != null)
    fun deleteTask(id: String)
    suspend fun moveDestinations(id: String, filter: String): List<DestinationGroup>
    fun moveTaskTo(id: String, list: String)
    fun reorderTask(list: String, id: String, neighbour: String, by: Int)
    fun rename(id: String, title: String)
    fun notesDraft(task: String): NotesDraft
    fun setNotes(id: String, notes: String, onResult: (String?) -> Unit)
    fun setMyDay(id: String, on: Boolean)

    /** The star: important is high priority, as on the desktop. */
    fun setImportant(id: String, on: Boolean)

    /** Adds a step; [onResult] gets null once it is added, else why it wasn't. */
    fun addStep(task: String, title: String, onResult: (String?) -> Unit)

    /** The task's "+ Add step" draft, kept where it outlives the Activity. */
    fun stepDraft(task: String): StepDraft
    fun setStepDone(id: String, done: Boolean)

    /** Sets the due date (ISO, `2026-11-22`) or clears it; clearing it also stops a repeat. */
    fun setDue(id: String, date: String?)

    /** Sets the priority by its `!` marks: 3 (high), 2 (normal), 1 (low) or 0 (none). */
    fun setPriority(id: String, marks: Int)

    /**
     * Sets the reminder the owner saw ([seen]; null: the task had none) to [atMs] (milliseconds since the
     * Unix epoch), or removes it (null). One that changed meanwhile is left alone. The toast offers Undo.
     */
    fun setReminder(id: String, seen: ReminderRef?, atMs: Long?)
    fun renameStep(id: String, title: String)

    /** Moves a step one place up (-1) or down (1). */
    fun moveStep(id: String, by: Int)

    /** Deletes a step; the toast offers Undo. */
    fun deleteStep(id: String)
    fun createList(name: String)
    fun organize(edit: OrganizeEdit) = Unit
    fun organizeQuestion(target: app.todoornot.core.ColorTarget): String? = null
    fun confirmDelete(target: app.todoornot.core.ColorTarget, asked: String, result: (String?) -> Unit) = Unit
    fun tagNamed(name: String): Boolean = false
    fun previewRepeat(id: String, draft: app.todoornot.core.CustomRepeat): app.todoornot.core.RepeatPreview =
        app.todoornot.core.RepeatPreview("No such task.", null, false)
    fun openActivity(agent: String?) = Unit
    suspend fun activityPage(source: ActivitySource, before: ULong?): app.todoornot.core.FeedPage? =
        app.todoornot.core.FeedPage(emptyList(), null, null, false)
    fun undoEvent(event: app.todoornot.core.ActivityRow) = Unit
    suspend fun retiredEvents(ids: List<String>): Map<String, String> = emptyMap()

    /** Renames a list or a collection. */
    fun renameRoom(room: RoomKey, name: String)

    /** Moves a list (among its siblings) or a collection one place up (-1) or down (1). */
    fun moveRoom(room: RoomKey, by: Int)

    /** The question before deleting a list, saying what goes with it; null if it's gone. */
    fun deleteListQuestion(id: String): String?

    /**
     * Deletes a list and its tasks if [asked] (the question the owner confirmed) still reads the same; the
     * toast offers Undo. [onResult] gets null when it's done (or the list is gone), else the new question
     * to ask again.
     */
    fun deleteList(id: String, asked: String, onResult: (String?) -> Unit)

    /** Filters the open room: the query language when it parses, else words in the title. */
    fun setFilter(text: String)

    /** Opens or closes the filter field; closing clears the filter. */
    fun setSearching(on: Boolean)

    /** Folds or unfolds a collection in the drawer. */
    fun toggleCollection(id: String)

    /** Opens or closes the room's Completed section. */
    fun setShowCompleted(on: Boolean)
    fun undo(t: Toast)
    fun toastRemaining(t: Toast): Long? = null
    fun dismissToast(t: Toast)
    fun dismissFailed(id: String)
    fun nudge()

    /** Asks before unpairing (the overflow menu, or "Pair again" after the server signed this phone out). */
    fun unpair()

    /** The owner confirmed, having seen [unsent] unsent changes: unpair, discarding them, if that is still the count. */
    fun confirmUnpair(unsent: Int)

    fun cancelUnpair()

    object None : Actions {
        override fun open(room: RoomKey) = Unit
        override fun select(id: String?) = Unit
        override fun add(title: String) = Unit
        override fun previewDate(text: String): String? = null
        override fun setDone(id: String, done: Boolean, generation: Long?, row: Boolean) = Unit
        override fun deleteTask(id: String) = Unit
        override suspend fun moveDestinations(id: String, filter: String) = emptyList<DestinationGroup>()
        override fun moveTaskTo(id: String, list: String) = Unit
        override fun reorderTask(list: String, id: String, neighbour: String, by: Int) = Unit
        override fun rename(id: String, title: String) = Unit
        override fun notesDraft(task: String) = NotesDraft(task)
        override fun setNotes(id: String, notes: String, onResult: (String?) -> Unit) = Unit
        override fun setMyDay(id: String, on: Boolean) = Unit
        override fun setImportant(id: String, on: Boolean) = Unit
        override fun addStep(task: String, title: String, onResult: (String?) -> Unit) = Unit
        override fun stepDraft(task: String) = StepDraft(task)
        override fun setStepDone(id: String, done: Boolean) = Unit
        override fun setDue(id: String, date: String?) = Unit
        override fun setPriority(id: String, marks: Int) = Unit
        override fun setReminder(id: String, seen: ReminderRef?, atMs: Long?) = Unit
        override fun renameStep(id: String, title: String) = Unit
        override fun moveStep(id: String, by: Int) = Unit
        override fun deleteStep(id: String) = Unit
        override fun createList(name: String) = Unit
        override fun renameRoom(room: RoomKey, name: String) = Unit
        override fun moveRoom(room: RoomKey, by: Int) = Unit
        override fun deleteListQuestion(id: String): String? = null
        override fun deleteList(id: String, asked: String, onResult: (String?) -> Unit) = Unit
        override fun setFilter(text: String) = Unit
        override fun setSearching(on: Boolean) = Unit
        override fun toggleCollection(id: String) = Unit
        override fun setShowCompleted(on: Boolean) = Unit
        override fun undo(t: Toast) = Unit
        override fun dismissToast(t: Toast) = Unit
        override fun dismissFailed(id: String) = Unit
        override fun nudge() = Unit
        override fun unpair() = Unit
        override fun confirmUnpair(unsent: Int) = Unit
        override fun cancelUnpair() = Unit
    }
}
