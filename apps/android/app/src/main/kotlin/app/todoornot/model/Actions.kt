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
        override fun undo(t: Toast) = Unit
        override fun dismissToast(t: Toast) = Unit
        override fun dismissFailed(id: String) = Unit
        override fun nudge() = Unit
        override fun unpair() = Unit
        override fun confirmUnpair(unsent: Int) = Unit
        override fun cancelUnpair() = Unit
    }
}
