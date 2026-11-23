package app.todoornot.reminders

import app.todoornot.core.SyncState
import app.todoornot.model.TodoModel
import app.todoornot.session.EngineHost
import app.todoornot.session.Host
import java.util.concurrent.atomic.AtomicBoolean
import java.util.concurrent.atomic.AtomicInteger

/** A process can open its cache before a broadcast or WorkManager tells it why it was started. */
internal class ReminderDisplayGate {
    private val starting = AtomicBoolean(true)
    private val catchUps = AtomicInteger()
    val holdDue: Boolean get() = starting.get() || catchUps.get() > 0

    suspend fun initialCatchUp(sync: suspend () -> Unit) {
        try { sync() } finally { starting.set(false) }
    }

    suspend fun <T> catchingUp(sync: suspend () -> T): T {
        catchUps.incrementAndGet()
        return try { sync() } finally { catchUps.decrementAndGet() }
    }
}

internal fun EngineHost.reconcileReminders(reminders: Reminders, force: Boolean = false, holdDue: Boolean = false) {
    val ready = state.value as? Host.Ready ?: return
    reminders.reconcile(ready.engine, force, catchingUp = holdDue) {
        if (state.value !== ready) false
        else if (isAuthorized(ready)) true
        else {
            reminders.clear()
            false
        }
    }
}

/** A live stream isn't enough: the target must be in the confirmed replica. Without a sequence on a
 * push (FCM dropped messages), require the backlog of a fresh connection. */
internal suspend fun TodoModel.syncForReminder(
    budget: Long,
    seq: ULong? = null,
    freshConnection: Boolean = false,
    cursor: () -> ULong = engine::confirmedCursor,
): Boolean {
    engine.nudge()
    return syncInBackground(budget, freshConnection) {
        it.revoked || (it.state == SyncState.ONLINE && it.caughtUp &&
            if (seq == null) it.pending == 0u else cursor() >= seq)
    }
}
