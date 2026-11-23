package app.todoornot.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.semantics.LiveRegionMode
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.clearAndSetSemantics
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.liveRegion
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.dp
import app.todoornot.core.BulkControl
import app.todoornot.core.BulkEdit
import app.todoornot.core.DestinationGroup
import app.todoornot.design.Icon
import app.todoornot.design.LocalDesign
import app.todoornot.design.Tokens
import app.todoornot.model.Actions
import app.todoornot.model.UiState

@Composable
internal fun BulkStatusStrip(label: String, left: Boolean, clear: () -> Unit) {
    val r = LocalDesign.current.room
    Row(
        Modifier.fillMaxWidth().heightIn(min = Tokens.Metrics.Phone.BULK_STATUS_BAR.dp)
            .background(r.wash).hairline(r.line, top = true).padding(horizontal = Tokens.Space.S1.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        if (!left) IconButton("x", "Clear selection", tint = r.accentStrong, onClick = clear)
        Label(label, Tokens.Type.ROOM_TITLE_BAR, r.accentStrong,
            Modifier.weight(1f).padding(horizontal = Tokens.Space.S2.dp).semantics { liveRegion = LiveRegionMode.Polite })
        if (left) IconButton("x", "Clear selection", tint = r.accentStrong, onClick = clear)
    }
}

enum class BulkPicker { MOVE, DUE }

@Composable
internal fun BulkSelectionTools(state: UiState, onPicker: (BulkPicker) -> Unit, onApply: (BulkEdit) -> Unit) {
    val bar = state.bulk ?: return
    val r = LocalDesign.current.room
    val s = Tokens.Space
    Row(
        Modifier.fillMaxWidth().background(r.wash).hairline(r.line, top = true)
            .heightIn(min = Tokens.Metrics.Phone.TOAST_STRIP.dp)
            .horizontalScroll(rememberScrollState()).padding(horizontal = s.S3.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(s.S1.dp, Alignment.CenterHorizontally),
    ) {
        if (bar.tools.isEmpty()) Label("Select another task", Tokens.Type.META_PHONE, r.ink2)
        bar.tools.forEach { tool ->
            val control = tool.control
            val icon = when (control) {
                BulkControl.Move -> "list"
                BulkControl.Due -> "calendar"
                is BulkControl.Apply -> when (val edit = control.edit) {
                    is BulkEdit.Completion -> if (edit.done) "check" else "undo-2"
                    is BulkEdit.Important -> "star"
                    is BulkEdit.MyDay -> "sun"
                    BulkEdit.Delete -> "trash-2"
                    is BulkEdit.Due -> "calendar"
                    is BulkEdit.Move -> "list"
                }
            }
            val touch = Tokens.Metrics.Phone.TOUCH.dp
            val ink = when {
                state.signedOut -> r.ink4
                icon == "trash-2" -> r.dangerInk
                else -> r.ink2
            }
            Column(
                Modifier.widthIn(min = touch).heightIn(min = touch).clip(RoundedCornerShape(Tokens.Radius.BUTTON.dp))
                    .clickable(enabled = !state.signedOut, role = Role.Button) {
                        when (control) {
                            BulkControl.Move -> onPicker(BulkPicker.MOVE)
                            BulkControl.Due -> onPicker(BulkPicker.DUE)
                            is BulkControl.Apply -> onApply(control.edit)
                        }
                    }.semantics { contentDescription = tool.label }.padding(horizontal = s.S1.dp, vertical = s.S2.dp),
                horizontalAlignment = Alignment.CenterHorizontally,
                verticalArrangement = Arrangement.spacedBy(s.S1.dp, Alignment.CenterVertically),
            ) {
                Icon(icon, ink)
                Label(tool.shortLabel, Tokens.Type.BUTTON, ink, Modifier.clearAndSetSemantics {})
            }
        }
    }
}

@Composable
internal fun BulkSelectionPicker(picker: BulkPicker, state: UiState, actions: Actions,
    initialDestinations: List<DestinationGroup>? = null,
    close: () -> Unit,
) {
    val bar = state.bulk ?: return
    if (state.bulkIds.size >= 2 && !state.signedOut) when (picker) {
        BulkPicker.MOVE -> BottomSheet("Move tasks", close) { overlay ->
            var filter by remember { mutableStateOf("") }
            var groups by remember(filter) { mutableStateOf(initialDestinations.takeIf { filter.isEmpty() }) }
            LaunchedEffect(state, filter) { groups = actions.bulkDestinations(state.bulkIds, filter) }
            MoveTaskCard(filter, { filter = it }, groups, { to ->
                overlay.choose { actions.editBulk(BulkEdit.Move(to)) }
            }, overlay::close, inSheet = true)
        }
        BulkPicker.DUE -> BottomSheet("Set due date", close) { overlay ->
            DueDateCard(null, state.today, bar.hasRepeat, dateFormat = actions.composerNow().dateFormat, onSave = { date ->
                overlay.choose { actions.editBulk(BulkEdit.Due(date)) }
            }, onCancel = overlay::close, clearAvailable = bar.hasDue, markToday = !LocalSheetAtRest.current,
                repeatNote = "Clearing dates also stops any repeats on these tasks.", inSheet = true)
        }
    }
}
