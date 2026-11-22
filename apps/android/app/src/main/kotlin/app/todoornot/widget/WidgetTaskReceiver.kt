package app.todoornot.widget

import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.net.Uri
import android.util.Log
import app.todoornot.AppGraph
import app.todoornot.core.FfiException
import app.todoornot.core.LocalNow
import app.todoornot.core.localNow
import app.todoornot.model.TodoModel
import app.todoornot.session.EngineHost
import app.todoornot.session.Host
import app.todoornot.session.PairingFiles
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withTimeoutOrNull

/** A launcher's retained PendingIntent must still mean the same thing after a reorder or re-pair. */
internal data class WidgetTaskAction(val pairingId: String, val edit: WidgetEdit) {
    fun intent(context: Context): Intent {
        val operation = when (edit) {
            is WidgetEdit.Complete -> "complete"
            is WidgetEdit.Important -> if (edit.on) "mark-important" else "clear-importance"
        }
        val data = Uri.Builder().scheme(SCHEME).authority(AUTHORITY)
            .appendPath(pairingId).appendPath(edit.task).appendPath(operation).build()
        return Intent(ACTION, data, context, WidgetTaskReceiver::class.java)
    }

    suspend fun apply(host: EngineHost, model: TodoModel, now: LocalNow): Boolean {
        val ready = host.state.value as? Host.Ready ?: return false
        if (ready.session.pairingId != pairingId || ready.engine !== model.engine) return false
        return model.applyExternalTaskEdit(isCurrent = { host.isAuthorized(ready) }) { edit.apply(it, now) }
    }

    companion object {
        private const val ACTION = "app.todoornot.widget.EDIT_TASK"
        private const val SCHEME = "todo-widget"
        private const val AUTHORITY = "edit"

        fun from(intent: Intent): WidgetTaskAction? {
            if (intent.action != ACTION) return null
            val data = intent.data ?: return null
            if (data.scheme != SCHEME || data.authority != AUTHORITY || data.pathSegments.size != 3) return null
            val (pairingId, task, operation) = data.pathSegments
            if (!PairingFiles.isPairingId(pairingId)) return null
            val edit = when (operation) {
                "complete" -> WidgetEdit.Complete(task)
                "mark-important" -> WidgetEdit.Important(task, true)
                "clear-importance" -> WidgetEdit.Important(task, false)
                else -> return null
            }
            return WidgetTaskAction(pairingId, edit)
        }
    }
}

/** Explicit and non-exported. Glance's immutable PendingIntent includes the action URI in its identity. */
class WidgetTaskReceiver : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        val action = WidgetTaskAction.from(intent) ?: return
        val app = context.applicationContext
        val pending = goAsync()
        CoroutineScope(Dispatchers.IO).launch {
            try {
                withTimeoutOrNull(AppGraph.RECEIVER_BUDGET_MS) {
                    val graph = app.todoGraph
                    val model = graph.widgetModel() ?: return@withTimeoutOrNull
                    if (action.apply(graph.host, model, localNow())) WidgetSyncWorker.enqueue(app)
                    graph.widgets.invalidate()
                }
            } catch (e: FfiException) {
                Log.d("TodoWidget", "widget edit failed: ${e::class.simpleName}")
            } catch (e: IllegalStateException) {
                Log.d("TodoWidget", "widget edit ended: ${e::class.simpleName}")
            } finally {
                pending.finish()
            }
        }
    }
}
