package app.todoornot.updates

import android.content.Context
import androidx.work.BackoffPolicy
import androidx.work.Constraints
import androidx.work.CoroutineWorker
import androidx.work.ExistingPeriodicWorkPolicy
import androidx.work.ExistingWorkPolicy
import androidx.work.NetworkType
import androidx.work.OneTimeWorkRequestBuilder
import androidx.work.PeriodicWorkRequestBuilder
import androidx.work.WorkManager
import androidx.work.WorkerParameters
import androidx.work.workDataOf
import app.todoornot.widget.todoGraph
import java.util.concurrent.TimeUnit
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext

private const val MODE = "mode"
private const val VERSION = "version"
private const val RETRIES = 3
private const val BACKOFF_MINUTES = 15L

internal enum class UpdateMode(val check: Boolean, val userAsked: Boolean) {
    AUTOMATIC(true, false), CHECK_NOW(true, true), WIFI(false, false), DOWNLOAD_NOW(false, true),
}

class AppUpdateWorker(context: Context, params: WorkerParameters) : CoroutineWorker(context, params) {
    override suspend fun doWork(): Result = withContext(Dispatchers.IO) {
        val mode = inputData.getString(MODE)?.let(UpdateMode::valueOf) ?: UpdateMode.AUTOMATIC
        val retry = applicationContext.todoGraph.updates.run(
            mode, inputData.getLong(VERSION, -1L).takeIf { it > 0 }, runAttemptCount > 0,
        )
        if (retry && runAttemptCount < RETRIES) Result.retry() else Result.success()
    }
}

internal object UpdateWork {
    private const val PERIODIC = "android-app-update-check"
    private fun network(type: NetworkType) = Constraints.Builder().setRequiredNetworkType(type).build()

    fun start(context: Context) {
        WorkManager.getInstance(context).enqueueUniquePeriodicWork(
            PERIODIC, ExistingPeriodicWorkPolicy.KEEP,
            PeriodicWorkRequestBuilder<AppUpdateWorker>(CHECK_HOURS, TimeUnit.HOURS)
                .setInitialDelay(CHECK_HOURS, TimeUnit.HOURS)
                .setConstraints(network(NetworkType.CONNECTED))
                .setBackoffCriteria(BackoffPolicy.EXPONENTIAL, BACKOFF_MINUTES, TimeUnit.MINUTES)
                .build(),
        )
    }

    fun enqueue(context: Context, mode: UpdateMode, version: Long? = null) {
        val type = if (mode == UpdateMode.WIFI) NetworkType.UNMETERED else NetworkType.CONNECTED
        val request = OneTimeWorkRequestBuilder<AppUpdateWorker>()
            .setInputData(workDataOf(MODE to mode.name, VERSION to (version ?: -1L)))
            .setConstraints(network(type))
            .setBackoffCriteria(BackoffPolicy.EXPONENTIAL, BACKOFF_MINUTES, TimeUnit.MINUTES)
            .addTag(PERIODIC)
            .build()
        val policy = if (mode.userAsked) ExistingWorkPolicy.REPLACE else ExistingWorkPolicy.KEEP
        WorkManager.getInstance(context).enqueueUniqueWork("$PERIODIC-${mode.name}", policy, request)
    }

    fun cancel(context: Context) {
        val work = WorkManager.getInstance(context)
        work.cancelUniqueWork(PERIODIC)
        work.cancelAllWorkByTag(PERIODIC)
    }
}
