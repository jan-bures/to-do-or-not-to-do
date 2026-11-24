package app.todoornot.ui

import androidx.activity.compose.BackHandler
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.selection.selectable
import androidx.compose.foundation.selection.selectableGroup
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.heading
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.input.PasswordVisualTransformation
import androidx.compose.ui.text.input.VisualTransformation
import androidx.compose.ui.unit.dp
import app.todoornot.core.AssistantAccess
import app.todoornot.core.AssistantAccessRow
import app.todoornot.core.AssistantConnection
import app.todoornot.core.AssistantServer
import app.todoornot.core.FfiException
import app.todoornot.core.assistantEndpoint
import app.todoornot.core.AssistantTurn
import app.todoornot.design.Icon
import app.todoornot.design.LocalDesign
import app.todoornot.design.Tokens
import app.todoornot.design.style
import app.todoornot.model.AssistantModel
import app.todoornot.model.AssistantProvider
import app.todoornot.model.AssistantUi

private val S = Tokens.Space
private val T = Tokens.Type
private val P = Tokens.Metrics.Phone
private val C = Tokens.Metrics.Component

@Composable
internal fun AssistantSettings(ui: AssistantUi, model: AssistantModel?, initialProvider: AssistantProvider?, close: () -> Unit) {
    val r = LocalDesign.current.room
    val state = ui.panel
    val usingRelay = state.hosted?.let { it.privateSelected || it.selectedAccount != null } == true
    val server = state.server
    // Don't send a draft to the Activity's saved-state Bundle or show it outside the app.
    var address by remember(server) { mutableStateOf(server?.address.orEmpty()) }
    var name by remember(server) { mutableStateOf(server?.model.orEmpty()) }
    var apiKey by remember(server) { mutableStateOf("") }
    var removeKey by remember(server) { mutableStateOf(false) }
    val clearKeyDraft = { apiKey = ""; removeKey = false }
    var editing by remember { mutableStateOf<String?>(null) }
    var provider by remember { mutableStateOf(initialProvider) }
    val shownProvider = provider ?: AssistantProvider.selected(state.hosted)
    val shape = RoundedCornerShape(Tokens.Radius.LG.dp)
    val draft = AssistantServer(address.trim(), name.trim(), false)
    val info = remember(draft.address) {
        if (draft.address.isEmpty()) null else runCatching { assistantEndpoint(draft.address, false) }
    }
    val endpoint = info?.getOrNull()
    val keySaved = server?.address == draft.address && state.hasApiKey && !removeKey
    val replacementKey = apiKey.takeIf { it.isNotBlank() } ?: "".takeIf { removeKey }
    val changed = draft != server || replacementKey != null
    LaunchedEffect(model) { model?.refreshHosted() }
    DisposableEffect(model) { onDispose { model?.cancelChatgpt() } }
    BottomSheet("AI settings", close, tall = true, color = r.pane, defaultFraction = P.SHEET_MAX_FRACTION) { overlay ->
        BackHandler(enabled = editing != null) { editing = null }
        Column(Modifier.weight(1f).fillMaxWidth().verticalScroll(rememberScrollState()).padding(horizontal = S.S4.dp, vertical = S.S2.dp),
            verticalArrangement = Arrangement.spacedBy(S.S3.dp)) {
            Label("AI settings", T.DETAIL_TITLE, r.ink, Modifier.semantics { heading() })
            ProviderPicker(shownProvider) {
                provider = it
                editing = null
                model?.clearSignInStatus()
                if (it != AssistantProvider.CHATGPT) model?.cancelChatgpt()
            }
            if (shownProvider != AssistantProvider.DEVICE) HostedAssistantSettings(shownProvider, ui, model)
            else Column(Modifier.fillMaxWidth().clip(shape).background(r.raised)
                .border(Tokens.Elevation.HAIRLINE.dp, r.accentStrong, shape)
                .padding(S.S3.dp), verticalArrangement = Arrangement.spacedBy(S.S2.dp)) {
                Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(S.S2.dp), verticalAlignment = Alignment.CenterVertically) {
                    Icon("link", r.accentStrong)
                    Label("This device", T.TASK_TITLE, r.ink, Modifier.weight(1f), maxLines = 2)
                    endpoint?.let { Label(it.label, T.META_PHONE, r.ink2) }
                }
                Label("Connect directly to a model on this device. There is no cloud fallback.",
                    T.BODY, r.ink2, maxLines = Int.MAX_VALUE)
                SettingsField("Base URL", "http://127.0.0.1:11434", address, {
                    if (it != address) { address = it; clearKeyDraft() }
                }, KeyboardType.Uri)
                (info?.exceptionOrNull() as? FfiException.Invalid)?.let {
                    Label(it.detail, T.META_PHONE, r.dangerInk, maxLines = Int.MAX_VALUE)
                }
                Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(S.S2.dp), verticalAlignment = Alignment.Bottom) {
                    SettingsField("API key", if (keySaved) "Key saved · enter to replace" else "Optional bearer key", apiKey,
                        { apiKey = it }, KeyboardType.Password, Modifier.weight(1f))
                    if (keySaved) AiButton("Remove", { apiKey = ""; removeKey = true }, quiet = true)
                }
                SettingsField("Model", "Model name", name, { name = it }, KeyboardType.Text)
                Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(S.S2.dp)) {
                    AiButton("Test connection", { model?.test() }, Modifier.weight(1f),
                        enabled = !ui.busy && !state.streaming && !changed && state.destination == "This device" && state.connection != AssistantConnection.TESTING)
                    AiButton("Use this device", { model?.save(draft, replacementKey, clearKeyDraft) }, Modifier.weight(1f), primary = true,
                        enabled = !ui.busy && (changed || state.destination != "This device") && endpoint != null && name.isNotBlank())
                }
                val health = when (state.connection) {
                    AssistantConnection.UNTESTED -> null
                    AssistantConnection.TESTING -> "Testing connection…" to r.accent
                    AssistantConnection.CONNECTED -> listOfNotNull("Connected", state.support).joinToString(" · ") to r.success
                    AssistantConnection.UNREACHABLE -> "Server unreachable" to r.danger
                }
                health?.takeUnless { usingRelay }?.let { (text, color) ->
                    Row(horizontalArrangement = Arrangement.spacedBy(S.S2.dp)) {
                        Box(Modifier.padding(top = S.S1.dp).size(C.STATUS_LIGHT.dp).background(color, CircleShape))
                        Label(text, T.META_PHONE, r.ink2, maxLines = 2)
                    }
                }
            }
            (ui.problem ?: state.problem)?.let { AssistantProblem(it) }
            Label("Lists the assistant can see", T.TASK_TITLE, r.ink, Modifier.semantics { heading() }, maxLines = 2)
            Label("Lists start hidden. Private only excludes ChatGPT. Collection rules also apply to new lists; changing a collection replaces its lists’ overrides.",
                T.BODY, r.ink2, maxLines = Int.MAX_VALUE)
            if (state.access.isEmpty()) Label("No synced lists yet.", T.BODY, r.ink3)
            state.access.forEach { row ->
                val active = editing == row.id
                AccessRow(row, active) {
                    model?.clearSignInStatus()
                    editing = if (active) null else row.id
                }
                if (active) {
                    AssistantAccess.entries.forEach { access ->
                        val picked = access == row.access
                        AiButton(access.label(), {
                            model?.access(row, access)
                            editing = null
                        }, Modifier.padding(start = S.S4.dp).fillMaxWidth(), icon = if (picked) "check" else null,
                            enabled = !ui.busy, quiet = !picked)
                    }
                }
            }
        }
        Row(Modifier.fillMaxWidth().heightIn(min = P.BOTTOM_BAR.dp).background(r.canvas).hairline(r.line, top = true)
            .padding(horizontal = S.S4.dp), horizontalArrangement = Arrangement.SpaceBetween, verticalAlignment = Alignment.CenterVertically) {
            if (server != null || usingRelay) AiButton("Disconnect model", { model?.save(null, saved = clearKeyDraft) }, enabled = !ui.busy, quiet = true)
            else Spacer(Modifier.weight(1f))
            AiButton("Done", overlay::close, quiet = true)
        }
    }
}

@Composable
private fun ProviderPicker(selected: AssistantProvider, choose: (AssistantProvider) -> Unit) {
    val r = LocalDesign.current.room
    val shape = RoundedCornerShape(Tokens.Radius.MD.dp)
    Row(Modifier.fillMaxWidth().selectableGroup(), horizontalArrangement = Arrangement.spacedBy(S.S1.dp)) {
        AssistantProvider.entries.forEach { provider ->
            val active = provider == selected
            val fill = if (active) r.accentFill else r.raised
            Box(Modifier.weight(if (provider == AssistantProvider.PRIVATE) 1.65f else 1f)
                .heightIn(min = P.TOUCH.dp).clip(shape).background(fill)
                .border(Tokens.Elevation.HAIRLINE.dp, if (active) fill else r.lineStrong, shape)
                .selectable(active, role = Role.Tab, onClick = { choose(provider) }).padding(horizontal = S.S1.dp),
                contentAlignment = Alignment.Center) {
                Label(provider.label, T.BUTTON_SMALL, if (active) r.onAccent else r.ink2)
            }
        }
    }
}

@Composable
private fun AccessRow(row: AssistantAccessRow, expanded: Boolean, choose: () -> Unit) {
    val r = LocalDesign.current.room
    Row(Modifier.fillMaxWidth().heightIn(min = P.TOUCH.dp).hairline(r.line)
        .clickable(role = Role.Button, onClickLabel = "Change list access", onClick = choose)
        .padding(start = if (row.nested) S.S4.dp else S.S1.dp, end = S.S1.dp),
        horizontalArrangement = Arrangement.spacedBy(S.S2.dp), verticalAlignment = Alignment.CenterVertically) {
        Icon(if (row.collection) "columns-3" else "list", r.ink3, size = C.ICON_SMALL.dp)
        Label(row.name, if (row.collection) T.TASK_TITLE else T.BODY, r.ink, Modifier.weight(1f), maxLines = 2)
        Column(horizontalAlignment = Alignment.End) {
            Label(row.access?.label() ?: "Mixed", T.META_PHONE, r.ink2)
            if (row.inherited) Label("Inherited", T.META_PHONE, r.ink3)
        }
        Icon(if (expanded) "chevron-up" else "chevron-down", r.ink3, size = C.ICON_SMALL.dp)
    }
}

@Composable
internal fun AccessCheck(checked: Boolean) {
    val r = LocalDesign.current.room
    Box(Modifier.size(C.CHECKBOX.dp)
        .then(if (checked) Modifier.background(r.accentFill, CircleShape)
            else Modifier.border(Tokens.Elevation.HAIRLINE.dp, r.control, CircleShape)), contentAlignment = Alignment.Center) {
        if (checked) Icon("check", r.onAccent, size = C.ICON_SMALL.dp)
    }
}

@Composable
private fun SettingsField(label: String, hint: String, value: String, onChange: (String) -> Unit, keyboardType: KeyboardType,
    modifier: Modifier = Modifier,
) {
    val r = LocalDesign.current.room
    val shape = RoundedCornerShape(Tokens.Radius.MD.dp)
    Column(modifier) {
        Label(label, T.META_PHONE, r.ink2)
        BasicTextField(value, onChange, Modifier.fillMaxWidth().heightIn(min = P.TOUCH.dp)
            .clip(shape).border(Tokens.Elevation.HAIRLINE.dp, r.control, shape).semantics { contentDescription = label },
            textStyle = (if (keyboardType == KeyboardType.Uri) T.COUNT else T.BODY).style(r.ink), singleLine = true,
            visualTransformation = if (keyboardType == KeyboardType.Password) PasswordVisualTransformation() else VisualTransformation.None,
            cursorBrush = SolidColor(r.accentStrong), keyboardOptions = KeyboardOptions(keyboardType = keyboardType, autoCorrectEnabled = false),
            decorationBox = { field ->
                Box(Modifier.heightIn(min = P.TOUCH.dp).padding(horizontal = S.S3.dp), contentAlignment = Alignment.CenterStart) {
                    if (value.isEmpty()) Label(hint, T.META_PHONE, r.ink3)
                    field()
                }
            })
    }
}

private fun AssistantAccess.label() = when (this) {
    AssistantAccess.HIDDEN -> "Hidden"
    AssistantAccess.PRIVATE_ONLY -> "Private only"
    AssistantAccess.ALL_MODELS -> "All models"
}

@Composable
internal fun AssistantHistory(turns: List<AssistantTurn>, close: () -> Unit, onJump: (Int) -> Unit) {
    val r = LocalDesign.current.room
    BottomSheet("Conversation history", close, color = r.pane) { overlay ->
        Column(Modifier.weight(1f, fill = false).fillMaxWidth().verticalScroll(rememberScrollState()).padding(S.S4.dp)) {
            Label("This conversation", T.DETAIL_TITLE, r.ink, Modifier.semantics { heading() })
            if (turns.isEmpty()) Label("No messages yet.", T.BODY, r.ink2)
            turns.asReversed().forEachIndexed { index, turn ->
                AiButton(turn.prompt, { overlay.choose { onJump(index) } }, Modifier.fillMaxWidth(), icon = "clock", quiet = true)
            }
            Label("Chats stay on this phone while the app is running.", T.META_PHONE, r.ink3, maxLines = 2)
        }
        Row(Modifier.fillMaxWidth().padding(horizontal = S.S4.dp), horizontalArrangement = Arrangement.End) {
            AiButton("Done", overlay::close, quiet = true)
        }
    }
}
