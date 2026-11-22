package app.todoornot.widget

import android.appwidget.AppWidgetManager
import android.content.ComponentName
import android.content.Context
import android.util.Log
import androidx.glance.GlanceId
import androidx.glance.appwidget.updateAll
import androidx.work.Constraints
import androidx.work.CoroutineWorker
import androidx.work.ExistingWorkPolicy
import androidx.work.NetworkType
import androidx.work.OneTimeWorkRequestBuilder
import androidx.work.WorkManager
import androidx.work.WorkerParameters
import app.todoornot.AppGraph
import app.todoornot.TodoApplication
import app.todoornot.core.FfiException
import app.todoornot.core.SyncState
import app.todoornot.design.Tokens
import app.todoornot.model.TodoModel
import app.todoornot.session.Host
import app.todoornot.session.signedOut
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.delay
import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.filterNotNull
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import kotlinx.coroutines.withTimeoutOrNull

internal val Context.todoGraph get() = (applicationContext as TodoApplication).graph

internal fun Context.todoWidgetIds(): IntArray =
    AppWidgetManager.getInstance(this).getAppWidgetIds(ComponentName(this, TodoWidgetReceiver::class.java))

/** The only durable widget state: each launcher instance's choice, keyed by its framework ID. */
internal class WidgetChoicesStore(context: Context) {
    private val prefs = context.getSharedPreferences("home_widgets", Context.MODE_PRIVATE)
    fun get(id: Int) = widgetRoom(prefs.getString(id.toString(), null))
    fun put(id: Int, room: app.todoornot.core.RoomKey): Boolean = prefs.edit().putString(id.toString(), room.widgetKey()).commit()
    fun remove(id: Int) { prefs.edit().remove(id.toString()).apply() }
}

/** Batches bursts from the process's one engine listener, including reminder and widget sync. */
internal class WidgetRefresh(private val context: Context) {
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.IO)
    private val requests = Channel<Unit>(Channel.CONFLATED)
    private val revision = MutableStateFlow(0L)
    val changes = revision.asStateFlow()

    init {
        scope.launch {
            for (request in requests) {
                delay(Tokens.Motion.HOVER.toLong())
                requests.tryReceive()
                try {
                    publish()
                } catch (e: IllegalStateException) {
                    Log.d("TodoWidget", "could not refresh widget: ${e::class.simpleName}")
                }
            }
        }
    }

    fun invalidate() { requests.trySend(Unit) }

    fun configured(id: GlanceId) {
        scope.launch {
            // Refresh even if the launcher hasn't attached a newly configured widget yet.
            revision.update { it + 1 }
            try {
                TodoWidget().update(context, id)
            } catch (e: IllegalStateException) {
                Log.d("TodoWidget", "could not refresh configured widget: ${e::class.simpleName}")
            }
            WidgetSyncWorker.enqueue(context)
        }
    }

    suspend fun publish() {
        if (context.todoWidgetIds().isEmpty()) return
        revision.update { it + 1 }
        TodoWidget().updateAll(context)
    }
}

/** No stream or polling service for the widget. A launcher update or a user tap can request one catch-up. */
class WidgetSyncWorker(context: Context, params: WorkerParameters) : CoroutineWorker(context, params) {
    override suspend fun doWork(): Result = withContext(Dispatchers.IO) {
        val context = applicationContext
        val graph = context.todoGraph
        val store = WidgetChoicesStore(context)
        if (context.todoWidgetIds().none { store.get(it) != null }) return@withContext Result.success()
        val model = graph.widgetModel() ?: return@withContext Result.success()
        try {
            val ready = graph.host.state.value as? Host.Ready
            if (ready != null && ready.engine === model.engine && graph.host.isAuthorized(ready)) model.syncForWidget()
        } catch (e: FfiException) {
            Log.d("TodoWidget", "widget sync failed: ${e::class.simpleName}")
        } catch (e: IllegalStateException) {
            // The pairing closed during catch-up.
            Log.d("TodoWidget", "widget sync ended: ${e::class.simpleName}")
        }
        graph.widgets.publish()
        Result.success()
    }

    companion object {
        internal fun enqueue(context: Context) {
            val request = OneTimeWorkRequestBuilder<WidgetSyncWorker>()
                .setConstraints(Constraints.Builder().setRequiredNetworkType(NetworkType.CONNECTED).build())
                .build()
            WorkManager.getInstance(context).enqueueUniqueWork("todo-widget-sync", ExistingWorkPolicy.KEEP, request)
        }
    }
}

internal suspend fun AppGraph.widgetModel(): TodoModel? = withTimeoutOrNull(5_000L) {
    loaded.first { it }
    val ready = host.state.value as? Host.Ready ?: return@withTimeoutOrNull null
    model.filterNotNull().first { it.engine === ready.engine }
}

internal suspend fun TodoModel.syncForWidget() {
    nudge()
    syncInBackground(4_000L) {
        // An open SSE response can be Online well before it supplies its hello or backlog.
        it.signedOut || (it.state == SyncState.ONLINE && it.caughtUp && it.pending == 0u)
    }
}
