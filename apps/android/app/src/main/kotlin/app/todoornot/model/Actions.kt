package app.todoornot.model

import app.todoornot.core.BoardSlot
import app.todoornot.core.ListFeaturesPatch
import app.todoornot.core.ListView
import app.todoornot.core.WorkflowQuickAddDraft
import app.todoornot.core.WorkflowStage
import app.todoornot.core.BulkEdit
import app.todoornot.core.DestinationGroup
import app.todoornot.core.MergeCandidate
import app.todoornot.core.NoteCheckbox
import app.todoornot.core.ReminderRef
import app.todoornot.core.RoomKey
import app.todoornot.core.TaskDrag
import app.todoornot.core.TaskDrop
import app.todoornot.core.TimelineEdit
import app.todoornot.core.TimelineTask

enum class TaskSwipe { COMPLETION, MY_DAY }

/** What the main screen can ask for. [TodoModel] does the work; the screenshot gallery passes [None]. */
interface Actions {
    fun open(room: RoomKey)
    fun select(id: String?)
    fun toggleBulk(id: String, generation: Long) = Unit
    fun swipeTask(id: String, generation: Long, action: TaskSwipe) = Unit
    fun clearBulk() = Unit
    fun editBulk(edit: BulkEdit) = Unit
    suspend fun bulkDestinations(tasks: List<String>, filter: String): List<DestinationGroup> = emptyList()
    fun add(title: String) = add(app.todoornot.core.QuickAddDraft(title, null, false, null))
    fun add(draft: app.todoornot.core.QuickAddDraft)
    fun addWorkflow(draft: WorkflowQuickAddDraft) = add(draft.base)
    fun setListFeatures(list: String, patch: ListFeaturesPatch) = Unit
    fun setRoomView(view: ListView, generation: Long, showCompleted: Boolean = false) = Unit
    fun setTaskStage(list: String, id: String, stage: WorkflowStage, generation: Long) = Unit
    fun moveBoardCard(list: String, id: String, stage: WorkflowStage, slot: BoardSlot, generation: Long) = Unit
    fun setStartDate(id: String, date: String?) = Unit
    fun scheduleTimeline(task: TimelineTask, edit: TimelineEdit, today: String, generation: Long) = Unit
    fun composerNow(): app.todoornot.core.LocalNow = app.todoornot.core.localNow()
    fun setDateFormat(key: String): Boolean = false
    /**
     * Completes or reopens a task. A row tap and the detail pane pass the room [generation] they were read
     * in ([UiState.generation]); null (Undo) is the room on screen now. [row] says the tap was on the task's
     * row, which is inert while it's a ghost (the detail pane's checkbox isn't).
     */
    fun setDone(id: String, done: Boolean, generation: Long? = null, row: Boolean = generation != null)
    fun deleteTask(id: String)
    suspend fun moveDestinations(id: String, filter: String): List<DestinationGroup>
    fun moveTaskTo(id: String, from: String, list: String)
    suspend fun mergeCandidates(id: String, filter: String): List<MergeCandidate> = emptyList()
    suspend fun mergeQuestion(id: String, target: String): String? = null
    fun mergeTasks(id: String, target: String, asked: String, onResult: (String?) -> Unit) = Unit
    fun reorderTask(list: String, id: String, neighbour: String, by: Int)
    fun startTaskDrag(id: String, generation: Long): TaskDrag? = null
    fun taskDropValid(drag: TaskDrag, target: TaskDrop, generation: Long): Boolean = false
    fun dropTasks(drag: TaskDrag, target: TaskDrop, generation: Long) = Unit
    fun startSidebarDrag(source: RoomKey, generation: Long): app.todoornot.core.SidebarDrag? = null
    fun sidebarDropTarget(drag: app.todoornot.core.SidebarDrag, hit: RoomKey, after: Boolean, generation: Long): app.todoornot.core.SidebarDrop? = null
    fun dropSidebarRow(drag: app.todoornot.core.SidebarDrag, target: app.todoornot.core.SidebarDrop, generation: Long) = Unit
    fun rename(id: String, title: String)
    fun notesDraft(task: String): NotesDraft
    fun setNotes(id: String, notes: String, onResult: (String?) -> Unit)
    fun toggleNoteCheckbox(id: String, source: String, checkbox: NoteCheckbox) = Unit
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
    /** Direct transfers use private staging files, never the command queue. */
    fun transfer(action: FileAction) = Unit
    fun attachmentNotice(task: String, text: String) = Unit
    fun organize(edit: OrganizeEdit) = Unit
    fun organizeQuestion(target: app.todoornot.core.ColorTarget): String? = null
    fun confirmDelete(target: app.todoornot.core.ColorTarget, asked: String, result: (String?) -> Unit) = Unit
    fun tagNamed(name: String): Boolean = false
    fun previewRepeat(id: String, draft: app.todoornot.core.CustomRepeat): app.todoornot.core.RepeatPreview =
        app.todoornot.core.RepeatPreview("No such task.", null, false)
    fun restoreTrash(row: app.todoornot.core.TrashRow) = Unit
    fun openActivity(agent: String?) = Unit
    fun loadActivity(pagerId: Long, load: app.todoornot.core.ActivityLoad) = Unit
    fun undoEvent(pagerId: Long, event: app.todoornot.core.ActivityRow) = Unit

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
        override fun add(draft: app.todoornot.core.QuickAddDraft) = Unit
        override fun setDone(id: String, done: Boolean, generation: Long?, row: Boolean) = Unit
        override fun deleteTask(id: String) = Unit
        override suspend fun moveDestinations(id: String, filter: String) = emptyList<DestinationGroup>()
        override fun moveTaskTo(id: String, from: String, list: String) = Unit
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
