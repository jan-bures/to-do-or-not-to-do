package app.todoornot.widget

import android.app.Activity
import android.appwidget.AppWidgetManager
import android.content.Intent
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.addCallback
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.compose.foundation.background
import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.safeDrawing
import androidx.compose.foundation.layout.windowInsetsPadding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import androidx.glance.appwidget.GlanceAppWidgetManager
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.ViewModel
import androidx.lifecycle.ViewModelProvider
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.lifecycle.lifecycleScope
import androidx.lifecycle.repeatOnLifecycle
import androidx.lifecycle.viewModelScope
import app.todoornot.MainActivity
import app.todoornot.core.RoomKey
import app.todoornot.design.LocalDesign
import app.todoornot.design.TodoTheme
import app.todoornot.design.Tokens
import app.todoornot.session.Host
import app.todoornot.ui.Label
import app.todoornot.ui.NavRow
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext

class WidgetConfigActivity : ComponentActivity() {
    private var selected by mutableStateOf<RoomKey?>(null)
    private lateinit var commit: WidgetConfigCommit

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        val manager = GlanceAppWidgetManager(this)
        val id = intent.getIntExtra(AppWidgetManager.EXTRA_APPWIDGET_ID, AppWidgetManager.INVALID_APPWIDGET_ID)
        val result = Intent().putExtra(AppWidgetManager.EXTRA_APPWIDGET_ID, id)
        setResult(RESULT_CANCELED, result)
        // The launcher must be able to open this activity; don't let another widget's ID be written.
        if (id !in todoWidgetIds()) {
            finish()
            return
        }
        val glanceId = manager.getGlanceIdBy(intent) ?: run { finish(); return }
        val context = applicationContext
        commit = followWidgetConfig(result) { context.todoGraph.widgets.configured(glanceId) }
        lifecycleScope.launch {
            val previous = withContext(Dispatchers.IO) { WidgetChoicesStore(context).get(id) }
            if (!commit.state.value.locked) selected = previous
        }
        fun save(room: RoomKey?, ready: Host.Ready?) {
            if (isFinishing) return
            commit.save {
                withContext(Dispatchers.IO) {
                    if (id !in context.todoWidgetIds()) return@withContext false
                    val store = WidgetChoicesStore(context)
                    if (room == null) {
                        // Signed out: keep the existing selection, or add an unpersonalized My Day.
                        store.get(id) != null || store.put(id, defaultWidgetRoom)
                    } else if (ready != null && context.todoGraph.host.isAuthorized(ready)) {
                        store.put(id, room)
                    } else false
                }
            }
        }
        enableEdgeToEdge()
        setContent { Picker(::save) }
    }

    @Composable
    private fun Picker(save: (RoomKey?, Host.Ready?) -> Unit) {
        val transaction by commit.state.collectAsStateWithLifecycle()
        val graph = todoGraph
        val loaded by graph.loaded.collectAsStateWithLifecycle()
        val host by graph.host.state.collectAsStateWithLifecycle()
        val model by graph.model.collectAsStateWithLifecycle()
        val ready = host as? Host.Ready
        val ui = model?.takeIf { it.engine === ready?.engine }?.ui?.collectAsStateWithLifecycle()?.value
        TodoTheme(isSystemInDarkTheme(), "my-day") {
            val r = LocalDesign.current.room
            val pad = Tokens.Space.S4.dp
            Column(Modifier.fillMaxSize().background(r.canvas).windowInsetsPadding(WindowInsets.safeDrawing).padding(pad)) {
                Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) {
                    Label("Choose what to show", Tokens.Type.ROOM_TITLE_BAR, r.ink, Modifier.weight(1f))
                    TextButton(onClick = { commit.cancel(::finish) }, enabled = !transaction.locked) { Label("Cancel", Tokens.Type.NAV, r.ink) }
                }
                if (transaction == WidgetConfigState.Failed) Label("Couldn’t save the widget. Try again.", Tokens.Type.BODY, r.dangerInk)
                when {
                    !loaded || (host is Host.Ready && ui == null) -> Label("Loading lists…", Tokens.Type.BODY, r.ink3)
                    host is Host.CantOpen -> Label("Open the app to load your tasks", Tokens.Type.BODY, r.ink3)
                    host is Host.SignedOut || ui?.signedOut == true -> {
                        TextButton(onClick = { startActivity(Intent(this@WidgetConfigActivity, MainActivity::class.java)) }) {
                            Label("Open the app to sign in", Tokens.Type.BODY, r.accentStrong)
                        }
                        TextButton(onClick = { save(null, null) }, enabled = !transaction.locked) {
                            Label(if (selected == null) "Add My Day widget" else "Keep current selection", Tokens.Type.BODY, r.ink)
                        }
                    }
                    ui != null -> {
                        Label("Smart lists, lists and collections", Tokens.Type.BODY, r.ink3, Modifier.padding(bottom = pad))
                        LazyColumn {
                            items(widgetChoices(ui.home), key = { it.item.key.widgetKey() }) { choice ->
                                NavRow(choice.item, selected == choice.item.key, choice.indented, { save(it, ready) }, heading = choice.item.key is RoomKey.Collection)
                            }
                        }
                    }
                }
            }
        }
    }
}

/** Registers this activity to deliver the retained transaction's result, only while started. */
internal fun ComponentActivity.followWidgetConfig(result: Intent, afterSave: () -> Unit): WidgetConfigCommit {
    val commit = ViewModelProvider(this)[WidgetConfigCommit::class.java]
    onBackPressedDispatcher.addCallback(this) { commit.cancel(::finish) }
    lifecycleScope.launch {
        repeatOnLifecycle(Lifecycle.State.STARTED) {
            commit.state.collect {
                if (it == WidgetConfigState.Saved && !isFinishing) {
                    setResult(Activity.RESULT_OK, result)
                    finish()
                    afterSave()
                }
            }
        }
    }
    return commit
}

internal enum class WidgetConfigState {
    Idle, Saving, Saved, Failed;

    val locked get() = this == Saving || this == Saved
}

/** The write and its outcome outlive rotation/fold. Only the current activity delivers a result. */
internal class WidgetConfigCommit : ViewModel() {
    private val _state = MutableStateFlow(WidgetConfigState.Idle)
    val state = _state.asStateFlow()

    fun cancel(finish: () -> Unit) {
        if (!state.value.locked) finish()
    }

    fun save(store: suspend () -> Boolean) {
        if (state.value.locked) return
        _state.value = WidgetConfigState.Saving
        viewModelScope.launch {
            _state.value = if (store()) WidgetConfigState.Saved else WidgetConfigState.Failed
        }
    }
}
