package app.todoornot

import android.app.Application
import android.app.PendingIntent
import app.todoornot.core.RoomKey
import app.todoornot.core.SmartList
import app.todoornot.model.TodoModel
import app.todoornot.session.EngineHost
import app.todoornot.session.Host
import app.todoornot.session.PairingFiles
import app.todoornot.session.Session
import app.todoornot.session.SessionStore
import app.todoornot.widget.WidgetData
import app.todoornot.widget.WidgetEdit
import app.todoornot.widget.WidgetTaskAction
import app.todoornot.widget.widgetData
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.test.runTest
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.RuntimeEnvironment
import org.robolectric.Shadows.shadowOf
import org.robolectric.annotation.Config

@RunWith(RobolectricTestRunner::class)
@Config(application = Application::class, sdk = [35])
class WidgetTaskActionTest {
    @Test
    fun a_retained_checkbox_can_only_complete_the_task_it_showed_before_a_reorder() = runTest {
        val context = RuntimeEnvironment.getApplication()
        val host = EngineHost(SessionStore(MapValues(), FakeCipher()), PairingFiles(context.noBackupFilesDir), forgetKey = {}) { _, _ ->
            fixtureEngine()
        }
        host.paired(Session("https://todo.example.com", "device-1", "tdd_example"))
        val ready = host.state.value as Host.Ready
        ready.engine.use { engine ->
            val list = engine.createList("Test list", RoomKey.Smart(SmartList.ALL)) as RoomKey.List
            engine.addTask(list, "Another task", FIXTURE_NOW)
            engine.addTask(list, "Original task", FIXTURE_NOW)
            fun rows() = (widgetData(engine, list, FIXTURE_NOW) as WidgetData.Room).rows.map { it.id }
            val (original, another) = rows()
            val pairing = ready.session.pairingId
            fun pending(action: WidgetTaskAction): PendingIntent = PendingIntent.getBroadcast(
                context, 0, action.intent(context), PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
            )
            val retained = pending(WidgetTaskAction(pairing, WidgetEdit.Complete(original)))
            engine.reorderTask(list.id, original, another, 1)?.close()
            assertEquals(listOf(another, original), rows())
            val replacement = pending(WidgetTaskAction(pairing, WidgetEdit.Complete(another)))
            val mark = pending(WidgetTaskAction(pairing, WidgetEdit.Important(original, true)))
            val unmark = pending(WidgetTaskAction(pairing, WidgetEdit.Important(original, false)))
            val pairedAgain = pending(WidgetTaskAction(PairingFiles.newPairingId(), WidgetEdit.Complete(original)))
            assertEquals(5, setOf(retained, replacement, mark, unmark, pairedAgain).size)
            val action = WidgetTaskAction.from(shadowOf(retained).savedIntent)!!
            assertEquals(WidgetTaskAction(pairing, WidgetEdit.Complete(original)), action)
            val model = TodoModel(engine, backgroundScope, Dispatchers.Unconfined, now = { FIXTURE_NOW })
            assertTrue(action.apply(host, model, FIXTURE_NOW))
            assertTrue(engine.task(original, FIXTURE_NOW)!!.done)
            assertFalse(engine.task(another, FIXTURE_NOW)!!.done)
        }
    }
}
