package app.todoornot.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.selection.selectable
import androidx.compose.foundation.selection.selectableGroup
import androidx.compose.foundation.selection.toggleable
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Switch
import androidx.compose.material3.SwitchDefaults
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.dp
import app.todoornot.core.ListFeatures
import app.todoornot.core.ListFeaturesPatch
import app.todoornot.core.ListView
import app.todoornot.core.WorkflowStage
import app.todoornot.design.Icon
import app.todoornot.design.LocalDesign
import app.todoornot.design.Tokens
import app.todoornot.model.Actions
import app.todoornot.model.UiState
import app.todoornot.model.label

private val S = Tokens.Space
private val P = Tokens.Metrics.Phone
private val C = Tokens.Metrics.Component

internal val WorkflowStage.icon: String get() = when (this) {
    WorkflowStage.BACKLOG -> "status-backlog"
    WorkflowStage.TODO -> "status-todo"
    WorkflowStage.IN_PROGRESS -> "status-progress"
    WorkflowStage.DONE -> "status-done"
}

@Composable
internal fun WorkflowStage.ink(onWash: Boolean = false): Color {
    val r = LocalDesign.current.room
    return when (this) {
        WorkflowStage.BACKLOG -> r.ink3
        WorkflowStage.TODO -> r.ink2
        WorkflowStage.IN_PROGRESS -> r.accentStrong
        WorkflowStage.DONE -> if (onWash) r.checkSelected else r.accent
    }
}

@Composable
internal fun StatusMark(stage: WorkflowStage) {
    val r = LocalDesign.current.room
    val active = stage == WorkflowStage.IN_PROGRESS
    Row(
        (if (active) Modifier.background(r.wash, RoundedCornerShape(Tokens.Radius.SM.dp)).padding(horizontal = S.S1.dp) else Modifier)
            .heightIn(min = C.CHIP.dp).semantics { contentDescription = "Status: ${stage.label}" },
        verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(S.S1.dp),
    ) {
        Icon(stage.icon, stage.ink(), size = C.ICON_SMALL.dp)
        Label(stage.label, Tokens.Type.META_PHONE, if (stage == WorkflowStage.DONE) r.ink2 else stage.ink())
    }
}

@Composable
internal fun StatusOptions(current: WorkflowStage, includeDone: Boolean, choose: (WorkflowStage) -> Unit) {
    Column(Modifier.fillMaxWidth().selectableGroup()) {
        WorkflowStage.entries.filter { includeDone || it != WorkflowStage.DONE }.forEach { stage ->
            WorkflowOption(stage.label, stage.icon, stage == current, tint = stage.ink(), onClick = { choose(stage) })
        }
    }
}

@Composable
private fun WorkflowOption(label: String, icon: String, selected: Boolean, enabled: Boolean = true,
    tint: Color = LocalDesign.current.room.ink2, onClick: () -> Unit,
) {
    val r = LocalDesign.current.room
    Row(Modifier.fillMaxWidth().heightIn(min = P.TOUCH.dp)
        .background(if (selected) r.wash else Color.Transparent)
        .selectable(selected, enabled = enabled, role = Role.RadioButton, onClick = onClick)
        .padding(horizontal = S.S3.dp), verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(S.S2.dp)) {
        Icon(icon, if (enabled) tint else r.ink3)
        Label(label, Tokens.Type.BUTTON, if (enabled) r.ink else r.ink3, Modifier.weight(1f))
        if (selected) Icon("check", r.accentStrong)
    }
}

private val ListView.label: String get() = when (this) {
    ListView.LIST -> "List"
    ListView.BOARD -> "Board"
    ListView.TIMELINE -> "Timeline"
}

private val ListView.icon: String get() = when (this) {
    ListView.LIST -> "list"
    ListView.BOARD -> "columns-3"
    ListView.TIMELINE -> "gantt-chart"
}

private fun ListFeatures.views(): List<ListView> = buildList {
    add(ListView.LIST)
    if (workflow) add(ListView.BOARD)
    if (timeline) add(ListView.TIMELINE)
}

@Composable
internal fun ViewControl(state: UiState, actions: Actions, wide: Boolean, labels: Boolean = false, settings: () -> Unit) {
    val features = state.listFeatures ?: return
    val views = features.views()
    if (views.size < 2) return
    val choose = { view: ListView -> actions.setRoomView(view, state.generation) }
    if (wide) {
        ViewSegments(views, state.view, labels, choose)
    } else {
        var opened by remember(state.generation) { mutableStateOf(false) }
        Box {
            IconButton(if (features.workflow) ListView.BOARD.icon else ListView.TIMELINE.icon,
                "View: ${state.view.label}", expanded = opened || state.view != ListView.LIST) {
                if (views.size == 2 && features.workflow) choose(if (state.view == ListView.BOARD) ListView.LIST else ListView.BOARD)
                else opened = true
            }
            if (opened) AnchoredPopup(onDismiss = { opened = false }, above = true) { overlay ->
                Column(Modifier.widthIn(min = P.INNER_LIST.dp / 2).padding(S.S2.dp)) {
                    views.forEach { view ->
                        WorkflowOption(view.label, view.icon, view == state.view, view != ListView.TIMELINE) {
                            overlay.choose { choose(view) }
                        }
                    }
                    Box(Modifier.fillMaxWidth().hairline(LocalDesign.current.room.line, top = true)) {
                        WorkflowOption("List settings…", "settings", false) { overlay.choose(settings) }
                    }
                }
            }
        }
    }
}

@Composable
private fun ViewSegments(views: List<ListView>, current: ListView, labels: Boolean, choose: (ListView) -> Unit) {
    val r = LocalDesign.current.room
    val shape = RoundedCornerShape(Tokens.Radius.MD.dp)
    Row(Modifier.selectableGroup(), verticalAlignment = Alignment.CenterVertically) {
        views.forEach { view ->
            val selected = current == view
            val enabled = view != ListView.TIMELINE
            Box(Modifier.widthIn(min = P.TOUCH.dp).heightIn(min = P.TOUCH.dp)
                .selectable(selected, enabled = enabled, role = Role.Tab) { choose(view) }
                .semantics { if (!labels) contentDescription = view.label }, contentAlignment = Alignment.Center) {
                Row(Modifier.heightIn(min = Tokens.Metrics.Board.HEADER_HEIGHT.dp)
                    .background(if (selected) r.raised else r.canvas, shape)
                    .border(Tokens.Elevation.HAIRLINE.dp, if (selected) r.lineStrong else r.line, shape)
                    .padding(horizontal = if (labels) S.S2.dp else S.S3.dp),
                    verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(S.S1.dp)) {
                    Icon(view.icon, if (enabled) r.ink else r.ink3, size = C.ICON_SMALL.dp)
                    if (labels) Label(view.label, Tokens.Type.BUTTON_SMALL, if (enabled) r.ink else r.ink3)
                }
            }
        }
    }
}

@Composable
internal fun ListSettings(features: ListFeatures, update: (ListFeaturesPatch) -> Unit, back: () -> Unit) {
    val r = LocalDesign.current.room
    Column(Modifier.fillMaxWidth().verticalScroll(rememberScrollState()).padding(horizontal = S.S3.dp),
        verticalArrangement = Arrangement.spacedBy(S.S2.dp)) {
        Caps("List settings")
        FeatureSwitch("Workflow & Board", "Track Backlog, To do, In progress and Done, and unlock Board view.", features.workflow) {
            update(ListFeaturesPatch(it, null, null))
        }
        FeatureSwitch("Timeline & Start dates", "Add start dates to tasks. Timeline view is not yet available on Android.", features.timeline) {
            update(ListFeaturesPatch(null, it, null))
        }
        if (features.workflow || features.timeline) {
            Column(Modifier.fillMaxWidth().hairline(r.line, top = true).padding(top = S.S3.dp)) {
                Label("Default view", Tokens.Type.BUTTON, r.ink)
                Label("Opens this view when you visit the list.", Tokens.Type.META_PHONE, r.ink3)
                ViewSegments(features.views(), features.defaultView, labels = true) { update(ListFeaturesPatch(null, null, it)) }
            }
        }
        Label("Turning a feature off keeps your task statuses and start dates.", Tokens.Type.META_PHONE, r.ink3, maxLines = Int.MAX_VALUE)
        Button("Back", false, onClick = back)
    }
}

@Composable
private fun FeatureSwitch(label: String, explanation: String, on: Boolean, change: (Boolean) -> Unit) {
    val r = LocalDesign.current.room
    Row(Modifier.fillMaxWidth().heightIn(min = P.TOUCH.dp).toggleable(on, role = Role.Switch, onValueChange = change)
        .padding(vertical = S.S2.dp), verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(S.S3.dp)) {
        Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(S.S1.dp)) {
            Label(label, Tokens.Type.BUTTON, r.ink)
            Label(explanation, Tokens.Type.META_PHONE, r.ink3, maxLines = Int.MAX_VALUE)
        }
        Switch(on, onCheckedChange = null, colors = SwitchDefaults.colors(
            checkedTrackColor = r.accentStrong, checkedThumbColor = r.onAccent,
            uncheckedTrackColor = r.canvas, uncheckedThumbColor = r.control, uncheckedBorderColor = r.control,
        ))
    }
}
