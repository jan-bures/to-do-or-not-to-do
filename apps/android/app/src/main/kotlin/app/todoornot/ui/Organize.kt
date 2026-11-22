package app.todoornot.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.verticalScroll
import androidx.compose.runtime.*
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.dp
import app.todoornot.core.*
import app.todoornot.design.*
import app.todoornot.model.Actions
import app.todoornot.model.OrganizeEdit
import java.util.Locale

private val S = Tokens.Space
private val P = Tokens.Metrics.Phone

/** All picker content can scroll on a sideways cover, including with the keyboard visible. */
@Composable
internal fun PickerCard(title: String, content: @Composable ColumnScope.() -> Unit) {
    DialogCard(title) {
        Column(Modifier.fillMaxWidth().verticalScroll(rememberScrollState()), verticalArrangement = Arrangement.spacedBy(S.S2.dp)) {
            Label(title, Tokens.Type.DETAIL_TITLE_PHONE, LocalDesign.current.room.ink)
            content()
        }
    }
}

@Composable
internal fun Choice(label: String, selected: Boolean = false, onClick: () -> Unit) {
    val r = LocalDesign.current.room
    Row(Modifier.fillMaxWidth().heightIn(min = P.TOUCH.dp).clickable(onClick = onClick).padding(horizontal = S.S2.dp), verticalAlignment = Alignment.CenterVertically) {
        Label(label, Tokens.Type.NAV, r.ink, Modifier.weight(1f))
        if (selected) Icon("check", r.accentStrong)
    }
}

@Composable
private fun Input(label: String, value: String, edit: (String) -> Unit) {
    val r = LocalDesign.current.room
    Box(Modifier.fillMaxWidth().heightIn(min = P.TOUCH.dp).border(Tokens.Elevation.HAIRLINE.dp, r.lineStrong, RoundedCornerShape(Tokens.Radius.MD.dp)).padding(S.S3.dp), contentAlignment = Alignment.CenterStart) {
        if (value.isEmpty()) Label(label, Tokens.Type.NAV, r.ink3)
        BasicTextField(value, edit, Modifier.fillMaxWidth().semantics { contentDescription = label }, singleLine = true, textStyle = Tokens.Type.NAV.style(r.ink))
    }
}

@Composable
internal fun RepeatCard(detail: TaskDetail, custom: Boolean, onCustom: () -> Unit, choose: (RepeatChoice) -> Unit, cancel: () -> Unit, preview: (CustomRepeat) -> RepeatPreview) {
    val draft = detail.customRepeat
    if (!custom) {
        PickerCard("Repeat") {
            detail.repeatOptions.forEach { option -> Choice(option.label, option.selected) { choose(option.choice) } }
            Choice("Custom…", detail.repeats && detail.repeatOptions.none { it.selected }, onCustom)
            Choice("Don’t repeat", !detail.repeats) { choose(RepeatChoice.Stop) }
            if (detail.due == null) Label("Due starts today or on the next matching day.", Tokens.Type.META_PHONE, LocalDesign.current.room.ink3, maxLines = Int.MAX_VALUE)
            Button("Cancel", false, onClick = cancel)
        }
    } else {
        var interval by rememberSaveable { mutableStateOf(draft.interval.toString()) }
        var unit by rememberSaveable { mutableStateOf(draft.unit) }
        var days by rememberSaveable { mutableIntStateOf(draft.weekdays.toInt()) }
        PickerCard("Custom repeat") {
            Label("Interval", Tokens.Type.CAPS_LABEL, LocalDesign.current.room.ink3)
            Input("Interval", interval) { interval = it.filter(Char::isDigit) }
            Row(horizontalArrangement = Arrangement.spacedBy(S.S1.dp)) {
                RepeatUnit.entries.forEach { u -> Button(u.name.lowercase(Locale.ROOT).replaceFirstChar(Char::uppercaseChar), unit == u, Modifier.weight(1f)) { unit = u } }
            }
            if (unit == RepeatUnit.WEEK) {
                Label("On weekdays", Tokens.Type.CAPS_LABEL, LocalDesign.current.room.ink3)
                listOf("Mo", "Tu", "We", "Th", "Fr", "Sa", "Su").withIndex().chunked(4).forEach { group -> Row(horizontalArrangement = Arrangement.spacedBy(S.S1.dp)) {
                    group.forEach { (i, day) -> val bit = 1 shl i; Button(day, days and bit != 0, Modifier.weight(1f)) { days = days xor bit } }
                } }
            }
            val edit = CustomRepeat(unit, interval.toUIntOrNull() ?: 0u, days.toUByte())
            val result = preview(edit)
            val r = LocalDesign.current.room
            Label(result.description, Tokens.Type.META_PHONE, if (result.valid) r.ink2 else r.dangerInk, maxLines = Int.MAX_VALUE)
            result.dueNote?.let { Label(it, Tokens.Type.META_PHONE, r.ink2, maxLines = Int.MAX_VALUE) }
            Button("Apply repeat", true, enabled = result.valid) { choose(RepeatChoice.Custom(edit)) }
            Button("Cancel", false, onClick = cancel)
        }
    }
}

@Composable
internal fun MoveListCard(home: Home, list: RoomKey.List, choose: (String?) -> Unit, cancel: () -> Unit) {
    val current = home.groups.firstOrNull { g -> g.lists.any { it.key == list } }?.collection?.key
    PickerCard("Move to…") {
        home.groups.mapNotNull { it.collection }.forEach { item -> Choice(item.name, item.key == current) { choose((item.key as RoomKey.Collection).id) } }
        Choice("No collection", current == null) { choose(null) }
        Button("Cancel", false, onClick = cancel)
    }
}

@Composable
internal fun SwatchCard(selected: String, choose: (String) -> Unit, cancel: () -> Unit) {
    val d = LocalDesign.current
    PickerCard("Colour") {
        Tokens.swatches.chunked(6).forEach { group -> Row(Modifier.fillMaxWidth()) {
            group.forEach { hue -> Box(Modifier.weight(1f).height(P.TOUCH.dp).clickable { choose(hue) }.semantics { contentDescription = if (hue == selected) "$hue, selected" else hue }, contentAlignment = Alignment.Center) {
                Box(Modifier.size(S.S8.dp).background(d.list(hue).tint, CircleShape).border(Tokens.Elevation.HAIRLINE.dp, d.list(hue).dot, CircleShape), contentAlignment = Alignment.Center) { if (hue == selected) Icon("check", d.list(hue).ink) }
            } }
        } }
        Button("Cancel", false, onClick = cancel)
    }
}

@Composable
internal fun TagsCard(tags: List<TagChoice>, toggle: (TagChoice) -> Unit, manage: (TagChoice) -> Unit, create: (String) -> Unit, cancel: () -> Unit, existing: (String) -> Boolean, initialText: String = "") {
    var name by rememberSaveable { mutableStateOf(initialText) }
    val typed = name.trim()
    PickerCard("Tags") {
        Input("Type to find or create a tag", name) { name = it }
        if (typed.isNotEmpty() && !existing(typed)) {
            Choice("Create tag “$typed”") { create(typed) }
        }
        tags.filter { it.name.contains(name.trim(), ignoreCase = true) }.forEach { t -> Row(verticalAlignment = Alignment.CenterVertically) {
            Box(Modifier.size(Tokens.Metrics.Component.CHIP_DOT.dp).background(LocalDesign.current.list(t.hue).dot, CircleShape))
            Box(Modifier.weight(1f)) { Choice("#${t.name}", t.selected) { toggle(t) } }
            IconButton("chevron-right", "Options for #${t.name}") { manage(t) }
        } }
        if (tags.isEmpty() && typed.isEmpty()) Label("No tags yet. Type a name to create one.", Tokens.Type.META_PHONE, LocalDesign.current.room.ink3)
        Button("Done", false, onClick = cancel)
    }
}

@Composable
internal fun ConfirmOrganize(target: ColorTarget, actions: Actions, overlay: OverlayState) {
    val noun = if (target is ColorTarget.Tag) "tag" else "collection"
    var question by remember(target) { mutableStateOf(actions.organizeQuestion(target)) }
    var changed by remember(target) { mutableStateOf(false) }
    var busy by remember(target) { mutableStateOf(false) }
    val asked = question
    if (asked == null) { LaunchedEffect(target) { overlay.close() }; return }
    DeleteListCard(asked, noun = noun, changed = changed, busy = busy, onCancel = overlay::close, onDelete = {
        if (overlay.open && !busy) {
            busy = true
            actions.confirmDelete(target, asked) { next -> if (next == null) overlay.close() else { question = next; changed = true }; busy = false }
        }
    })
}

@Composable
internal fun TagOptions(tag: TagChoice, toggle: () -> Unit, navigate: (String) -> Unit) {
    PickerCard("#${tag.name}") {
        Choice(if (tag.selected) "Remove from task" else "Add to task", onClick = toggle)
        Choice("Rename tag") { navigate("Rename") }
        Choice("Colour…") { navigate("Colour") }
        Choice("Delete tag…") { navigate("Delete") }
        Button("All tags", false) { navigate("Tags") }
    }
}

@Composable
internal fun TagsDialog(detail: TaskDetail, actions: Actions, dismiss: () -> Unit) {
    var page by rememberSaveable { mutableStateOf("Tags") }
    var id by rememberSaveable { mutableStateOf<String?>(null) }
    val tag = detail.tags.find { it.id == id }
    OverlayDialog(dismiss) { overlay ->
        fun edit(e: OrganizeEdit) = overlay.choose { actions.organize(e) }
        when {
            page == "Delete" && id != null -> ConfirmOrganize(ColorTarget.Tag(requireNotNull(id)), actions, overlay)
            page == "Tags" || tag == null -> TagsCard(detail.tags, { edit(OrganizeEdit.ToggleTag(detail.id, it.id)) }, { id = it.id; page = "Options" }, { edit(OrganizeEdit.TagNamed(detail.id, it)) }, overlay::close, actions::tagNamed)
            page == "Options" -> TagOptions(tag, { edit(OrganizeEdit.ToggleTag(detail.id, tag.id)) }, { page = it })
            page == "Rename" -> RenameCard("Rename tag", tag.name, { edit(OrganizeEdit.RenameTag(tag.id, it)) }, overlay::close)
            page == "Colour" -> SwatchCard(tag.hue, { edit(OrganizeEdit.Color(ColorTarget.Tag(tag.id), it)) }, overlay::close)
            else -> Unit
        }
    }
}
