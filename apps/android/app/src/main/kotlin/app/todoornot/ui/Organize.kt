package app.todoornot.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.selection.selectable
import androidx.compose.foundation.selection.toggleable
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.verticalScroll
import androidx.compose.runtime.*
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.semantics.Role
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
internal fun PickerCard(title: String, inSheet: Boolean = false, content: @Composable ColumnScope.() -> Unit) {
    val body: @Composable () -> Unit = {
        Column(Modifier.fillMaxWidth().verticalScroll(rememberScrollState())
            .then(if (inSheet) Modifier.padding(S.S4.dp) else Modifier), verticalArrangement = Arrangement.spacedBy(S.S2.dp)) {
            Label(title, Tokens.Type.DETAIL_TITLE_PHONE, LocalDesign.current.room.ink)
            content()
        }
    }
    if (inSheet) body() else DialogCard(title) { body() }
}

@Composable
internal fun Choice(label: String, selected: Boolean? = null, multiple: Boolean = false, onClick: () -> Unit) {
    val r = LocalDesign.current.room
    val on = selected == true
    val action = when {
        selected == null -> Modifier.clickable(onClick = onClick)
        multiple -> Modifier.toggleable(selected, role = Role.Checkbox) { onClick() }
        else -> Modifier.selectable(selected, role = Role.RadioButton, onClick = onClick)
    }
    Row(Modifier.fillMaxWidth().heightIn(min = P.TOUCH.dp)
        .clip(RoundedCornerShape(Tokens.Radius.MD.dp))
        .then(if (on) Modifier.background(r.wash) else Modifier)
        .then(action).padding(horizontal = S.S2.dp), verticalAlignment = Alignment.CenterVertically) {
        Label(label, Tokens.Type.NAV, if (on) r.accentStrong else r.ink, Modifier.weight(1f),
            weight = if (on) Tokens.Type.BUTTON.weight else null)
        if (on) Icon("check", r.accentStrong)
    }
}

@Composable
private fun Segment(label: String, on: Boolean, modifier: Modifier, multiple: Boolean = false, choose: () -> Unit) {
    val r = LocalDesign.current.room
    val shape = RoundedCornerShape(Tokens.Radius.BUTTON.dp)
    Box(modifier.heightIn(min = P.TOUCH.dp).clip(shape)
        .background(if (on) r.accentFill else r.raised)
        .then(if (multiple) Modifier.toggleable(on, role = Role.Checkbox) { choose() }
            else Modifier.selectable(on, role = Role.RadioButton, onClick = choose)),
        contentAlignment = Alignment.Center) {
        Label(label, Tokens.Type.BUTTON, if (on) r.onAccent else r.ink2,
            weight = if (on) Tokens.Type.BUTTON.weight else Tokens.Type.NAV.weight)
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
internal fun RepeatCard(detail: TaskDetail, custom: Boolean, onCustom: () -> Unit, choose: (RepeatChoice) -> Unit,
    cancel: () -> Unit, inSheet: Boolean = false, preview: (CustomRepeat) -> RepeatPreview,
) {
    val draft = detail.customRepeat
    if (!custom) {
        PanelCard("Repeat", inSheet) {
            Label("Repeat", Tokens.Type.DETAIL_TITLE_PHONE, LocalDesign.current.room.ink)
            Column(Modifier.weight(1f, fill = false).verticalScroll(rememberScrollState()), verticalArrangement = Arrangement.spacedBy(S.S2.dp)) {
                detail.repeatOptions.forEach { option -> Choice(option.label, option.selected) { choose(option.choice) } }
                Choice("Custom…", detail.repeats && detail.repeatOptions.none { it.selected }, onClick = onCustom)
                Choice("Don’t repeat", !detail.repeats) { choose(RepeatChoice.Stop) }
                if (detail.due == null) Label(undatedRepeatNote(), Tokens.Type.META_PHONE, LocalDesign.current.room.ink3, maxLines = Int.MAX_VALUE)
            }
            Button("Cancel", false, onClick = cancel)
        }
    } else {
        var interval by rememberSaveable { mutableStateOf(draft.interval.toString()) }
        var unit by rememberSaveable { mutableStateOf(draft.unit) }
        var days by rememberSaveable { mutableIntStateOf(draft.weekdays.toInt()) }
        val edit = CustomRepeat(unit, interval.toUIntOrNull() ?: 0u, days.toUByte())
        val result = preview(edit)
        val r = LocalDesign.current.room
        PanelCard("Custom repeat", inSheet) {
            Label("Custom repeat", Tokens.Type.DETAIL_TITLE_PHONE, r.ink)
            Column(Modifier.weight(1f, fill = false).verticalScroll(rememberScrollState()), verticalArrangement = Arrangement.spacedBy(S.S2.dp)) {
            Label("Interval", Tokens.Type.CAPS_LABEL, LocalDesign.current.room.ink3)
            Input("Interval", interval) { interval = it.filter(Char::isDigit) }
            Row(horizontalArrangement = Arrangement.spacedBy(S.S1.dp)) {
                RepeatUnit.entries.forEach { u -> Segment(u.name.lowercase(Locale.ROOT).replaceFirstChar(Char::uppercaseChar), unit == u, Modifier.weight(1f)) { unit = u } }
            }
            if (unit == RepeatUnit.WEEK) {
                Label("On weekdays", Tokens.Type.CAPS_LABEL, LocalDesign.current.room.ink3)
                listOf("Mo", "Tu", "We", "Th", "Fr", "Sa", "Su").withIndex().chunked(4).forEach { group -> Row(horizontalArrangement = Arrangement.spacedBy(S.S1.dp)) {
                    group.forEach { (i, day) -> val bit = 1 shl i; Segment(day, days and bit != 0, Modifier.weight(1f), multiple = true) { days = days xor bit } }
                } }
            }
            Label(result.description, Tokens.Type.META_PHONE, if (result.valid) r.ink2 else r.dangerInk, maxLines = Int.MAX_VALUE)
            result.dueNote?.let { Label(it, Tokens.Type.META_PHONE, r.ink2, maxLines = Int.MAX_VALUE) }
            }
            Row(horizontalArrangement = Arrangement.spacedBy(S.S2.dp)) {
                Button("Cancel", false, Modifier.weight(1f), onClick = cancel)
                Button("Apply repeat", true, Modifier.weight(1f), enabled = result.valid) { choose(RepeatChoice.Custom(edit)) }
            }
        }
    }
}

@Composable
internal fun RepeatSheet(detail: TaskDetail, actions: Actions, close: () -> Unit) {
    var custom by rememberSaveable { mutableStateOf(false) }
    BottomSheet("Repeat", close) { overlay ->
        androidx.activity.compose.BackHandler(custom) { custom = false }
        SheetPages(custom) { showing ->
            RepeatCard(detail, showing, { custom = true }, { choice ->
                overlay.choose { actions.organize(OrganizeEdit.Repeat(detail.id, choice)) }
            }, { if (showing) custom = false else overlay.close() }, inSheet = true) { actions.previewRepeat(detail.id, it) }
        }
    }
}

@Composable
internal fun MoveListCard(home: Home, list: RoomKey.List, choose: (String?) -> Unit, cancel: () -> Unit, inSheet: Boolean = false) {
    val current = home.groups.firstOrNull { g -> g.lists.any { it.key == list } }?.collection?.key
    var query by rememberSaveable { mutableStateOf("") }
    val collections = remember(home, query) {
        val onlyCollections = Home(emptyList(), home.groups.map { it.copy(lists = emptyList()) }, emptyList())
        app.todoornot.core.filterRoomSwitcher(onlyCollections, query).home.groups.mapNotNull { it.collection }
    }
    val noCollection = "No collection"
    val showRoot = noCollection.contains(query.trim(), ignoreCase = true)
    val r = LocalDesign.current.room
    PanelCard("Move to…", inSheet) {
        Label("Move to…", Tokens.Type.DETAIL_TITLE_PHONE, r.ink)
        Column(Modifier.weight(1f, fill = false).verticalScroll(rememberScrollState())) {
            collections.forEach { item -> Choice(item.name, item.key == current) { choose((item.key as RoomKey.Collection).id) } }
            if (showRoot) Choice(noCollection, current == null) { choose(null) }
            if (collections.isEmpty() && !showRoot) Label("No collection matches.", Tokens.Type.BODY, r.ink2)
        }
        Input("Filter collections", query) { query = it }
        Button("Cancel", false, onClick = cancel)
    }
}

@Composable
internal fun SwatchCard(target: ColorTarget, selected: String, choose: (String) -> Unit, cancel: () -> Unit, inSheet: Boolean = false) {
    val d = LocalDesign.current
    PickerCard(colourHeading(target), inSheet) {
        Tokens.swatches.chunked(6).forEach { group -> Row(Modifier.fillMaxWidth()) {
            group.forEach { hue ->
                val on = hue == selected
                val colors = d.list(hue)
                Box(Modifier.weight(1f).height(P.TOUCH.dp)
                    .selectable(on, role = Role.RadioButton) { choose(hue) }
                    .semantics { contentDescription = swatchName(hue) }, contentAlignment = Alignment.Center) {
                    Box(Modifier.size(S.S8.dp).background(if (on) colors.strong else colors.tint, CircleShape)
                        .border(Tokens.Elevation.HAIRLINE.dp, colors.dot, CircleShape), contentAlignment = Alignment.Center) {
                        if (on) Icon("check", colors.onStrong)
                    }
                }
            }
        } }
        Button("Cancel", false, onClick = cancel)
    }
}

@Composable
internal fun TagsCard(tags: List<TagChoice>, toggle: (TagChoice) -> Unit, manage: (TagChoice) -> Unit, create: (String) -> Unit,
    cancel: () -> Unit, existing: (String) -> Boolean, initialText: String = "", inSheet: Boolean = false,
    retainedQuery: String? = null, onQuery: (String) -> Unit = {},
) {
    var local by rememberSaveable { mutableStateOf(initialText) }
    val name = retainedQuery ?: local
    val typed = name.trim()
    val filtered = tags.filter { it.name.contains(typed, ignoreCase = true) }
    PanelCard("Tags", inSheet) {
        Label("Tags", Tokens.Type.DETAIL_TITLE_PHONE, LocalDesign.current.room.ink)
        Column(Modifier.weight(1f, fill = false).verticalScroll(rememberScrollState())) {
            filtered.forEach { t -> Row(verticalAlignment = Alignment.CenterVertically) {
                Box(Modifier.size(Tokens.Metrics.Component.CHIP_DOT.dp).background(LocalDesign.current.list(t.hue).dot, CircleShape))
                Box(Modifier.weight(1f)) { Choice("#${t.name}", t.selected, multiple = true) { toggle(t) } }
                IconButton("chevron-right", "Options for #${t.name}") { manage(t) }
            } }
            if (tags.isEmpty() && typed.isEmpty()) Label("No tags yet. Type a name to create one.", Tokens.Type.META_PHONE, LocalDesign.current.room.ink3)
        }
        if (typed.isNotEmpty() && !existing(typed)) Choice("Create tag “$typed”") { create(typed) }
        Input("Type to find or create a tag", name) { if (retainedQuery == null) local = it else onQuery(it) }
        Button("Done", false, onClick = cancel)
    }
}

@Composable
internal fun ConfirmOrganize(target: ColorTarget, actions: Actions, overlay: OverlayState,
    inSheet: Boolean = false, cancel: () -> Unit = overlay::close,
) {
    val noun = if (target is ColorTarget.Tag) "tag" else "collection"
    var question by remember(target) { mutableStateOf(actions.organizeQuestion(target)) }
    var changed by remember(target) { mutableStateOf(false) }
    var busy by remember(target) { mutableStateOf(false) }
    val asked = question
    if (asked == null) { LaunchedEffect(target) { overlay.close() }; return }
    DeleteListCard(asked, noun = noun, changed = changed, busy = busy, onCancel = cancel, inSheet = inSheet, onDelete = {
        if (overlay.open && !busy) {
            busy = true
            actions.confirmDelete(target, asked) { next -> if (next == null) overlay.close() else { question = next; changed = true }; busy = false }
        }
    })
}

@Composable
internal fun TagOptions(tag: TagChoice, toggle: () -> Unit, navigate: (String) -> Unit, inSheet: Boolean = false) {
    PickerCard("#${tag.name}", inSheet) {
        Choice(if (tag.selected) "Remove from task" else "Add to task", onClick = toggle)
        Choice("Rename tag") { navigate("Rename") }
        Choice("Color…") { navigate("Colour") }
        Choice("Delete tag…") { navigate("Delete") }
        Button("All tags", false) { navigate("Tags") }
    }
}

@Composable
internal fun TagsSheet(detail: TaskDetail, actions: Actions, dismiss: () -> Unit) {
    var page by rememberSaveable { mutableStateOf("Tags") }
    var query by rememberSaveable { mutableStateOf("") }
    var id by rememberSaveable { mutableStateOf<String?>(null) }
    val tag = detail.tags.find { it.id == id }
    val focus = androidx.compose.ui.platform.LocalFocusManager.current
    BottomSheet("Tags", dismiss) { overlay ->
        fun edit(e: OrganizeEdit) = overlay.choose { actions.organize(e) }
        fun navigate(to: String) { focus.clearFocus(); page = to }
        fun back() = navigate(if (page == "Options") "Tags" else "Options")
        androidx.activity.compose.BackHandler(page != "Tags") { back() }
        SheetPages(page) { showing ->
            when {
                showing == "Delete" && id != null -> ConfirmOrganize(ColorTarget.Tag(requireNotNull(id)), actions, overlay, true, ::back)
                showing == "Tags" || tag == null -> TagsCard(detail.tags, { edit(OrganizeEdit.ToggleTag(detail.id, it.id)) },
                    { id = it.id; navigate("Options") }, { edit(OrganizeEdit.TagNamed(detail.id, it)) }, overlay::close, actions::tagNamed,
                    inSheet = true, retainedQuery = query, onQuery = { query = it })
                showing == "Options" -> TagOptions(tag, { edit(OrganizeEdit.ToggleTag(detail.id, tag.id)) }, ::navigate, inSheet = true)
                showing == "Rename" -> RenameCard("Rename tag", tag.name, { edit(OrganizeEdit.RenameTag(tag.id, it)) }, ::back, inSheet = true)
                showing == "Colour" -> SwatchCard(ColorTarget.Tag(tag.id), tag.hue, { edit(OrganizeEdit.Color(ColorTarget.Tag(tag.id), it)) }, ::back, inSheet = true)
            }
        }
    }
}
