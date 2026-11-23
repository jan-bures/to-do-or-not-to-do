package app.todoornot.push

import android.content.Context
import android.os.Build
import androidx.work.Constraints
import androidx.work.CoroutineWorker
import androidx.work.ExistingPeriodicWorkPolicy
import androidx.work.ExistingWorkPolicy
import androidx.work.NetworkType
import androidx.work.OneTimeWorkRequestBuilder
import androidx.work.OutOfQuotaPolicy
import androidx.work.PeriodicWorkRequestBuilder
import androidx.work.WorkManager
import androidx.work.WorkerParameters
import androidx.work.workDataOf
import app.todoornot.AppGraph
import app.todoornot.session.Host
import app.todoornot.widget.todoGraph
import com.google.firebase.messaging.FirebaseMessagingService
import com.google.firebase.messaging.RemoteMessage
import java.time.Instant
import java.util.concurrent.TimeUnit
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.withContext
import kotlinx.coroutines.withTimeoutOrNull

private const val PAIRING = "pairing"
private const val WAKE = "wake"
private const val MAX_RETRIES = 3
internal const val FALLBACK_MINUTES = 30L

/** The push payload is only a hint; no state or sequence is advanced from it. */
internal fun syncTickle(data: Map<String, String>, hasNotification: Boolean): Boolean =
    !hasNotification && data["t"] == "sync" && (data["seq"]?.toULongOrNull() ?: 0uL) > 0uL

/** The FCM transport never posts a notification. The usual local reminder planner still can. */
class TodoMessagingService : FirebaseMessagingService() {
    override fun onNewToken(token: String) {
        // A queued callback may carry an old token. Ask the current Firebase app for its token instead.
        val graph = todoGraph
        val pairing = (graph.host.state.value as? Host.Ready)?.session?.pairingId ?: graph.push.messagePairing()
        pairing?.let { PushWork.register(this, it, rotation = true) }
    }

    override fun onMessageReceived(message: RemoteMessage) {
        if (syncTickle(message.data, message.notification != null)) catchUp()
    }

    override fun onDeletedMessages() = catchUp()

    private fun catchUp() {
        val graph = todoGraph
        val pairing = graph.push.messagePairing() ?: return
        val at = Instant.now()
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.S) {
            // Before Android 12 expedited WorkManager jobs require a visible foreground notification.
            // FCM already calls this on a background thread with a short execution window.
            val caughtUp = runBlocking(Dispatchers.IO) {
                withTimeoutOrNull(AppGraph.RECEIVER_BUDGET_MS) { graph.syncReminders(pairing, at) } == true
            }
            if (caughtUp) return
        }
        PushWork.wake(this, pairing, at)
    }
}

class ReminderSyncWorker(context: Context, params: WorkerParameters) : CoroutineWorker(context, params) {
    override suspend fun doWork(): Result = withContext(Dispatchers.IO) {
        val pairing = inputData.getString(PAIRING) ?: return@withContext Result.success()
        val wake = inputData.getLong(WAKE, 0L).takeIf { it > 0L }?.let(Instant::ofEpochMilli)
        val graph = applicationContext.todoGraph
        val caughtUp = graph.syncReminders(pairing, wake)
        if (wake == null && (graph.host.state.value as? Host.Ready)?.session?.pairingId == pairing) {
            // Discover newly configured push and recover registration even without foreground starts.
            PushWork.register(applicationContext, pairing)
        }
        if (caughtUp || runAttemptCount >= MAX_RETRIES) Result.success() else Result.retry()
    }
}

class PushRegistrationWorker(context: Context, params: WorkerParameters) : CoroutineWorker(context, params) {
    override suspend fun doWork(): Result = withContext(Dispatchers.IO) {
        val pairing = inputData.getString(PAIRING) ?: return@withContext Result.success()
        val graph = applicationContext.todoGraph
        if (withTimeoutOrNull(5_000L) { graph.loaded.first { it } } == null) return@withContext Result.retry()
        val ready = graph.host.state.value as? Host.Ready ?: return@withContext Result.success()
        if (ready.session.pairingId != pairing || !graph.host.isAuthorized(ready)) return@withContext Result.success()
        val registered = graph.push.refresh(ready.session) { graph.host.isAuthorized(ready) }
        if (registered || runAttemptCount >= MAX_RETRIES) Result.success() else Result.retry()
    }
}

internal object PushWork {
    private val connected = Constraints.Builder().setRequiredNetworkType(NetworkType.CONNECTED).build()
    private fun tag(id: String) = "reminder-push-$id"

    fun start(context: Context, id: String) {
        val request = PeriodicWorkRequestBuilder<ReminderSyncWorker>(FALLBACK_MINUTES, TimeUnit.MINUTES)
            .setConstraints(connected)
            .setInputData(workDataOf(PAIRING to id))
            .addTag(tag(id))
            .build()
        WorkManager.getInstance(context).enqueueUniquePeriodicWork("reminder-sync-$id", ExistingPeriodicWorkPolicy.KEEP, request)
        register(context, id)
    }

    fun register(context: Context, id: String, rotation: Boolean = false) {
        val request = OneTimeWorkRequestBuilder<PushRegistrationWorker>()
            .setConstraints(connected)
            .setInputData(workDataOf(PAIRING to id))
            .addTag(tag(id))
            .build()
        // A rotation during an upload needs another pass after the earlier one.
        val policy = if (rotation) ExistingWorkPolicy.APPEND_OR_REPLACE else ExistingWorkPolicy.KEEP
        WorkManager.getInstance(context).enqueueUniqueWork("reminder-registration-$id", policy, request)
    }

    fun wake(context: Context, id: String, at: Instant) {
        val builder = OneTimeWorkRequestBuilder<ReminderSyncWorker>()
            .setConstraints(connected)
            .setInputData(workDataOf(PAIRING to id, WAKE to at.toEpochMilli()))
            .addTag(tag(id))
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
            builder.setExpedited(OutOfQuotaPolicy.RUN_AS_NON_EXPEDITED_WORK_REQUEST)
        }
        // A newer wake supersedes an older retry and must not sit behind its backoff.
        WorkManager.getInstance(context).enqueueUniqueWork("reminder-wake-$id", ExistingWorkPolicy.REPLACE, builder.build())
    }

    fun cancel(context: Context, id: String) {
        WorkManager.getInstance(context).cancelAllWorkByTag(tag(id))
    }
}
