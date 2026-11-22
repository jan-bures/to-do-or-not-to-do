package app.todoornot.model

import app.todoornot.core.RoomKey

/** What the main screen can ask for. [TodoModel] does the work; the screenshot gallery passes [None]. */
interface Actions {
    fun open(room: RoomKey)
    fun select(id: String?)
    fun add(title: String)
    fun setDone(id: String, done: Boolean)
    fun rename(id: String, title: String)
    fun setMyDay(id: String, on: Boolean)

    /** The star: important is high priority, as on the desktop. */
    fun setImportant(id: String, on: Boolean)

    /** Adds a step; [onResult] gets null once it is added, else why it wasn't. */
    fun addStep(task: String, title: String, onResult: (String?) -> Unit)

    /** The task's "+ Add step" draft, kept where it outlives the Activity. */
    fun stepDraft(task: String): StepDraft

    /** A pane let go of the task's draft: dropped if no pane shows it, it is empty and nothing is on its way. */
    fun releaseStepDraft(task: String)
    fun setStepDone(id: String, done: Boolean)
    fun createList(name: String)

    /** Filters the open room: the query language when it parses, else words in the title. */
    fun setFilter(text: String)

    /** Opens or closes the filter field; closing clears the filter. */
    fun setSearching(on: Boolean)

    /** Folds or unfolds a collection in the drawer. */
    fun toggleCollection(id: String)
    fun undo(t: Toast)
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
        override fun setDone(id: String, done: Boolean) = Unit
        override fun rename(id: String, title: String) = Unit
        override fun setMyDay(id: String, on: Boolean) = Unit
        override fun setImportant(id: String, on: Boolean) = Unit
        override fun addStep(task: String, title: String, onResult: (String?) -> Unit) = Unit
        override fun stepDraft(task: String) = StepDraft(task)
        override fun releaseStepDraft(task: String) = Unit
        override fun setStepDone(id: String, done: Boolean) = Unit
        override fun createList(name: String) = Unit
        override fun setFilter(text: String) = Unit
        override fun setSearching(on: Boolean) = Unit
        override fun toggleCollection(id: String) = Unit
        override fun undo(t: Toast) = Unit
        override fun dismissToast(t: Toast) = Unit
        override fun dismissFailed(id: String) = Unit
        override fun nudge() = Unit
        override fun unpair() = Unit
        override fun confirmUnpair(unsent: Int) = Unit
        override fun cancelUnpair() = Unit
    }
}
