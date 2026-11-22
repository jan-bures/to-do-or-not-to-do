package app.todoornot.widget

import android.appwidget.AppWidgetManager
import android.content.Context
import android.content.Intent
import android.net.Uri
import android.util.Log
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.remember
import androidx.glance.GlanceId
import androidx.glance.action.Action
import androidx.glance.action.ActionParameters
import androidx.glance.appwidget.action.actionStartActivity
import androidx.glance.action.actionParametersOf
import androidx.glance.appwidget.GlanceAppWidget
import androidx.glance.appwidget.GlanceAppWidgetManager
import androidx.glance.appwidget.GlanceAppWidgetReceiver
import androidx.glance.appwidget.SizeMode
import androidx.glance.appwidget.action.ActionCallback
import androidx.glance.appwidget.action.actionRunCallback
import androidx.glance.appwidget.provideContent
import app.todoornot.MainActivity
import app.todoornot.core.FfiException
import app.todoornot.core.RoomKey
import app.todoornot.core.localNow
import app.todoornot.session.Host
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.flow.filterNotNull
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.flow.map
import kotlinx.coroutines.withContext
import kotlinx.coroutines.withTimeoutOrNull

class TodoWidgetReceiver : GlanceAppWidgetReceiver() {
    override val glanceAppWidget = TodoWidget()

    override fun onUpdate(context: Context, appWidgetManager: AppWidgetManager, appWidgetIds: IntArray) {
        super.onUpdate(context, appWidgetManager, appWidgetIds)
        // The launcher's infrequent updates also recalculate local dates, including while offline.
        context.todoGraph.widgets.invalidate()
        WidgetSyncWorker.enqueue(context)
    }
}

class TodoWidget : GlanceAppWidget() {
    override val sizeMode = SizeMode.Exact

    override suspend fun provideGlance(context: Context, id: GlanceId) {
        val instance = GlanceAppWidgetManager(context).getAppWidgetId(id)
        val graph = context.todoGraph
        val initial = readWidget(context, instance)
        provideContent {
            val changes = remember { graph.widgets.changes.map { readWidget(context, instance) } }
            val data by changes.collectAsState(initial)
            WidgetCard(data, WidgetClicks(context, instance))
        }
    }

    override suspend fun onDelete(context: Context, glanceId: GlanceId) {
        withContext(Dispatchers.IO) {
            WidgetChoicesStore(context).remove(GlanceAppWidgetManager(context).getAppWidgetId(glanceId))
        }
    }
}

private suspend fun readWidget(context: Context, instance: Int): WidgetData = withContext(Dispatchers.IO) {
    val graph = context.todoGraph
    graph.loaded.first { it }
    val host = graph.host.state.value
    when (host) {
        Host.SignedOut -> WidgetData.SignedOut
        is Host.CantOpen -> WidgetData.Unavailable
        is Host.Ready -> try {
            if (!host.widgetAuthorized()) WidgetData.SignedOut
            else widgetData(host.engine, WidgetChoicesStore(context).get(instance), localNow()).let {
                // Do not hand a stale read from before unpair back to the launcher.
                if (graph.host.state.value === host) it else WidgetData.SignedOut
            }
        } catch (e: IllegalStateException) {
            WidgetData.SignedOut // Engine closed during unpair.
        }
    }
}

private val TASK = ActionParameters.Key<String>("task")
internal const val WIDGET_ADD = "app.todoornot.widget.ADD"
internal const val WIDGET_ROOM = "app.todoornot.widget.ROOM"

/** With no lazy collection, Glance uses immutable PendingIntents to these explicit components. */
internal class WidgetClicks(private val context: Context, private val id: Int) {
    fun pick() = actionStartActivity(
        Intent(AppWidgetManager.ACTION_APPWIDGET_CONFIGURE, uri("configure", id.toString()), context, WidgetConfigActivity::class.java)
            .putExtra(AppWidgetManager.EXTRA_APPWIDGET_ID, id),
    )

    fun app() = actionStartActivity(main(Intent.ACTION_MAIN))
    fun add(room: RoomKey) = actionStartActivity(
        main(WIDGET_ADD).setData(uri("add", room.widgetKey())).putExtra(WIDGET_ROOM, room.widgetKey()),
    )
    fun task(id: String) = actionStartActivity(
        main(Intent.ACTION_VIEW).setData(uri("task", id)).putExtra(MainActivity.EXTRA_TASK, id),
    )

    fun complete(task: String): Action = actionRunCallback<WidgetCompleteCallback>(actionParametersOf(TASK to task))

    fun important(task: String, on: Boolean): Action = if (on) {
        actionRunCallback<WidgetStarCallback>(actionParametersOf(TASK to task))
    } else {
        actionRunCallback<WidgetUnstarCallback>(actionParametersOf(TASK to task))
    }

    fun refresh(): Action = actionRunCallback<WidgetRefreshCallback>()

    private fun main(action: String) = Intent(action, null, context, MainActivity::class.java)
        .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_SINGLE_TOP or Intent.FLAG_ACTIVITY_CLEAR_TOP)

    private fun uri(action: String, id: String): Uri =
        Uri.Builder().scheme("todo-widget").authority(action).appendPath(id).build()
}

class WidgetCompleteCallback : ActionCallback {
    override suspend fun onAction(context: Context, glanceId: GlanceId, parameters: ActionParameters) {
        parameters[TASK]?.let { applyWidgetEdit(context, WidgetEdit.Complete(it)) }
    }
}

class WidgetStarCallback : ActionCallback {
    override suspend fun onAction(context: Context, glanceId: GlanceId, parameters: ActionParameters) {
        parameters[TASK]?.let { applyWidgetEdit(context, WidgetEdit.Important(it, true)) }
    }
}

class WidgetUnstarCallback : ActionCallback {
    override suspend fun onAction(context: Context, glanceId: GlanceId, parameters: ActionParameters) {
        parameters[TASK]?.let { applyWidgetEdit(context, WidgetEdit.Important(it, false)) }
    }
}

private suspend fun applyWidgetEdit(context: Context, edit: WidgetEdit) {
    val graph = context.todoGraph
    val model = withTimeoutOrNull(WidgetSyncWorker.MODEL_WAIT_MS) {
        graph.loaded.first { it }
        if (graph.host.state.value is Host.Ready) graph.model.filterNotNull().first() else null
    } ?: return
    val ready = graph.host.state.value as? Host.Ready ?: return
    if (model.engine !== ready.engine) return
    try {
        val saved = withContext(Dispatchers.IO) {
            ready.widgetAuthorized() && model.applyExternalTaskEdit { edit.apply(it, localNow()) }
        }
        if (saved) WidgetSyncWorker.enqueue(context)
    } catch (e: FfiException) {
        Log.d("TodoWidget", "widget edit failed: ${e::class.simpleName}")
    } catch (e: IllegalStateException) {
        Log.d("TodoWidget", "widget edit ended: ${e::class.simpleName}")
    }
    graph.widgets.publish()
}

class WidgetRefreshCallback : ActionCallback {
    override suspend fun onAction(context: Context, glanceId: GlanceId, parameters: ActionParameters) {
        context.todoGraph.widgets.publish()
        WidgetSyncWorker.enqueue(context)
    }
}
