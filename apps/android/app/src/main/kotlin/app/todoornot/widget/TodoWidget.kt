package app.todoornot.widget

import android.appwidget.AppWidgetManager
import android.content.Context
import android.content.Intent
import android.net.Uri
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.remember
import androidx.glance.GlanceId
import androidx.glance.action.Action
import androidx.glance.action.ActionParameters
import androidx.glance.appwidget.action.actionStartActivity
import androidx.glance.appwidget.action.actionSendBroadcast
import androidx.glance.appwidget.GlanceAppWidget
import androidx.glance.appwidget.GlanceAppWidgetManager
import androidx.glance.appwidget.GlanceAppWidgetReceiver
import androidx.glance.appwidget.SizeMode
import androidx.glance.appwidget.action.ActionCallback
import androidx.glance.appwidget.action.actionRunCallback
import androidx.glance.appwidget.provideContent
import app.todoornot.MainActivity
import app.todoornot.core.RoomKey
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.flow.map
import kotlinx.coroutines.withContext

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
            val snapshot by changes.collectAsState(initial)
            WidgetCard(snapshot.data, WidgetClicks(context, instance, snapshot.pairingId))
        }
    }

    override suspend fun onDelete(context: Context, glanceId: GlanceId) {
        withContext(Dispatchers.IO) {
            WidgetChoicesStore(context).remove(GlanceAppWidgetManager(context).getAppWidgetId(glanceId))
        }
    }
}

private suspend fun readWidget(context: Context, instance: Int): WidgetSnapshot = withContext(Dispatchers.IO) {
    val graph = context.todoGraph
    graph.loaded.first { it }
    widgetSnapshot(graph.host, WidgetChoicesStore(context).get(instance), graph.displayNow())
}

internal const val WIDGET_ADD = "app.todoornot.widget.ADD"
internal const val WIDGET_ROOM = "app.todoornot.widget.ROOM"

/** Header actions use individual PendingIntents; Glance routes row actions through its collection template. */
internal class WidgetClicks(private val context: Context, private val id: Int, private val pairingId: String?) {
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

    fun complete(task: String): Action? = edit(WidgetEdit.Complete(task))
    fun important(task: String, on: Boolean): Action? = edit(WidgetEdit.Important(task, on))

    private fun edit(edit: WidgetEdit): Action? = pairingId?.let {
        actionSendBroadcast(WidgetTaskAction(it, edit).intent(context))
    }

    fun refresh(): Action = actionRunCallback<WidgetRefreshCallback>()

    private fun main(action: String) = Intent(action, null, context, MainActivity::class.java)
        .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_SINGLE_TOP or Intent.FLAG_ACTIVITY_CLEAR_TOP)

    private fun uri(action: String, id: String): Uri =
        Uri.Builder().scheme("todo-widget").authority(action).appendPath(id).build()
}

class WidgetRefreshCallback : ActionCallback {
    override suspend fun onAction(context: Context, glanceId: GlanceId, parameters: ActionParameters) {
        context.todoGraph.widgets.publish()
        WidgetSyncWorker.enqueue(context)
    }
}
