package app.todoornot.ui

import android.content.ActivityNotFoundException
import android.content.Intent
import android.net.Uri
import android.widget.Toast
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.key
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.focus.onFocusChanged
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalFocusManager
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.KeyboardCapitalization
import androidx.compose.ui.text.style.TextDecoration
import androidx.compose.ui.unit.dp
import app.todoornot.core.DetailField
import app.todoornot.core.FieldEdit
import app.todoornot.core.StepRow
import app.todoornot.core.SourceLink
import app.todoornot.core.StepCount
import app.todoornot.core.TaskDetail
import app.todoornot.design.Icon
import app.todoornot.design.LocalDesign
import app.todoornot.design.Tokens
import app.todoornot.design.style
import app.todoornot.model.Actions
import app.todoornot.model.MenuAction
import app.todoornot.model.UiState
import app.todoornot.model.priorityMenu
import app.todoornot.model.stepMenu
import app.todoornot.reminders.ReminderNeed
import java.time.ZoneId
import kotlinx.coroutines.delay

private val P = Tokens.Metrics.Phone
private val S = Tokens.Space
private val C = Tokens.Metrics.Component

/** Important is high priority (`!!!`), as the desktop's star sets it. */
private const val IMPORTANT: UByte = 3u

/**
 * A task's detail: breadcrumb, title (editable), steps (tick, rename, move or delete them, or add one),
 * notes, fields (the due date and the priority are edited from their rows) and sources. The bar's actions
 * are My Day (sun), Important (star), task actions (⋯), then Close. Beside the list on the unfolded
 * phone; on its own, with a back arrow and its own Undo strip, on the cover screen. Its checkbox
 * carries the room generation it was read in, so a tap after another room opens only writes.
 * [stepEditing] starts with that step's title being renamed (the gallery).
 */
@Composable
fun DetailPane(
    state: UiState,
    actions: Actions,
    standalone: Boolean,
    modifier: Modifier = Modifier,
    stepEditing: String? = null,
) {
    val detail = state.detail ?: return
    val today = state.today
    val generation = state.generation
    val toast = state.toast
    val r = LocalDesign.current.room
    Column(modifier.fillMaxSize().background(r.pane).then(if (standalone) Modifier.statusBarsPadding() else Modifier)) {
        Row(
            Modifier
                .fillMaxWidth()
                .height(P.APP_BAR.dp)
                .hairline(r.line)
                .padding(start = if (standalone) S.S1.dp else S.S6.dp, end = S.S2.dp),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(S.S2.dp),
        ) {
            if (standalone) IconButton("chevron-left", "Back") { actions.select(null) }
            Row(Modifier.weight(1f), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(S.S2.dp)) {
                detail.breadcrumb.forEachIndexed { i, c ->
                    if (i > 0) Label("/", Tokens.Type.META_PHONE, r.ink4)
                    if (i == 0) ListChip(c) else Label(c.label, Tokens.Type.BUTTON_SMALL, r.ink)
                }
            }
            IconButton(
                "sun",
                if (detail.myDay) "Remove from My Day" else "Add to My Day",
                tint = if (detail.myDay) r.accentStrong else r.ink3,
            ) { actions.setMyDay(detail.id, !detail.myDay) }
            val important = detail.priority >= IMPORTANT
            IconButton(
                "star",
                if (important) "Not important" else "Mark important",
                tint = if (important) r.accentStrong else r.ink3,
            ) { actions.setImportant(detail.id, !important) }
            TaskActionsAnchor(state, actions, detail.id, detail.done, detail.myDay, detail.priority.toInt(), row = false) { open ->
                IconButton("ellipsis", "Task actions", onClick = open)
            }
            if (!standalone) IconButton("x", "Close") { actions.select(null) }
        }
        Box(Modifier.weight(1f)) {
        Column(
            Modifier
                .fillMaxSize()
                .imePadding()
                .verticalScroll(rememberScrollState())
                .padding(start = S.S6.dp, end = S.S6.dp, top = S.S4.dp + S.S1.dp / 2)
                .navigationBarsPadding(),
        ) {
            Row(verticalAlignment = Alignment.Top, horizontalArrangement = Arrangement.spacedBy(S.S3.dp)) {
                Box(Modifier.size(P.COL_CHECK.dp, Tokens.Type.DETAIL_TITLE_PHONE.lineHeight.dp), contentAlignment = Alignment.Center) {
                    Check(detail.done, { actions.setDone(detail.id, it, generation, row = false) }, detail.title)
                }
                TitleField(detail, actions, Modifier.weight(1f))
            }
            Label(
                detail.subtitle,
                Tokens.Type.COUNT,
                r.ink3,
                Modifier.padding(start = P.COL_CHECK.dp + S.S3.dp, top = S.S1.dp, bottom = S.S4.dp),
            )
            Row(
                Modifier.fillMaxWidth().padding(top = S.S1.dp).height(S.S6.dp).hairline(r.line),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Label("Steps", Tokens.Type.CAPS_LABEL, r.ink3, Modifier.weight(1f))
                if (detail.steps.isNotEmpty()) {
                    StepMeter(StepCount(detail.steps.count { it.done }.toUInt(), detail.steps.size.toUInt()))
                }
            }
            key(detail.id) { Steps(detail, actions, stepEditing) }
            AddStep(detail.id, actions)
            key(detail.id) { TaskNotes(detail, actions) }
            if (detail.fields.isNotEmpty()) {
                Spacer(Modifier.height(S.S4.dp))
                Box(Modifier.fillMaxWidth().hairline(r.line, top = true))
                key(detail.id) { Fields(detail, today, actions) }
            }
            if (detail.sources.isNotEmpty()) {
                Caps("Source")
                detail.sources.forEach { s -> SourceCard(s) { actions.organize(app.todoornot.model.OrganizeEdit.RemoveLink(s.id)) } }
            }
            key(detail.id) {
                var adding by rememberSaveable { mutableStateOf(false) }
                Choice("Add link…") { adding = true }
                if (adding) OverlayDialog({ adding = false }) { overlay ->
                    RenameCard("Add a web link", "", { address ->
                        overlay.choose { actions.organize(app.todoornot.model.OrganizeEdit.AddLink(detail.id, address)) }
                    }, overlay::close, webAddress = true)
                }
            }
            // On its own, the toast strip sits over the end of the pane: room to scroll clear of it.
            Spacer(Modifier.height(S.S8.dp + if (standalone && toast != null) P.TOAST_STRIP.dp else 0.dp))
        }
        if (standalone) {
            Box(Modifier.align(Alignment.BottomCenter).imePadding().navigationBarsPadding()) { ToastStrip(toast, actions) }
        }
        }
    }
}

/**
 * The steps. Tap a title to rename it in place (Done or leaving the field keeps it, a blank title
 * doesn't); the step's menu (its ⋯ button, or a long press) renames, moves or deletes it. A moved step
 * and the one it swaps with glide to their places (`ROW_ENTER`, `STANDARD`); nothing else moves.
 */
@Composable
private fun Steps(detail: TaskDetail, actions: Actions, startEditing: String?) {
    var editing by rememberSaveable { mutableStateOf(startEditing) }
    var menu by remember { mutableStateOf<String?>(null) }
    // The steps a local move swaps: only they glide (a change from elsewhere never moves rows).
    var moving by remember { mutableStateOf(emptySet<String>()) }
    LaunchedEffect(moving) {
        if (moving.isNotEmpty()) {
            delay(Tokens.Motion.ROW_ENTER * 4L)
            moving = emptySet()
        }
    }
    fun perform(action: MenuAction) {
        when (action) {
            is MenuAction.RenameStep -> editing = action.step
            is MenuAction.MoveStep -> {
                val i = detail.steps.indexOfFirst { it.id == action.step }
                val other = detail.steps.getOrNull(i + action.by)
                if (other != null) {
                    moving = setOf(action.step, other.id)
                    actions.moveStep(action.step, action.by)
                }
            }
            is MenuAction.DeleteStep -> actions.deleteStep(action.step)
            else -> Unit
        }
    }
    detail.steps.forEachIndexed { i, step ->
        key(step.id) {
            StepLine(
                step,
                i,
                editing = editing == step.id,
                onEdit = { on -> editing = if (on) step.id else null },
                actions = actions,
                modifier = Modifier.slideOnMove(step.id in moving),
            ) {
                Box {
                    IconButton("ellipsis", "Step actions: ${step.title}", tint = LocalDesign.current.room.ink3) { menu = step.id }
                    if (menu == step.id) {
                        MenuPopup(stepMenu(detail.steps, step.id), ::perform, onDismiss = { if (menu == step.id) menu = null }, title = "Step")
                    }
                }
            }
        }
    }
}

/** One step: its box, its title (or the field renaming it), its number and its menu button. */
@Composable
private fun StepLine(
    step: StepRow,
    index: Int,
    editing: Boolean,
    onEdit: (Boolean) -> Unit,
    actions: Actions,
    modifier: Modifier = Modifier,
    menuButton: @Composable () -> Unit,
) {
    val r = LocalDesign.current.room
    Row(
        modifier.fillMaxWidth().heightIn(min = P.TOUCH.dp).hairline(r.line),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(S.S3.dp),
    ) {
        StepMark(step.done, step.title) { actions.setStepDone(step.id, it) }
        if (editing) {
            StepTitleField(step, actions, onDone = { onEdit(false) }, modifier = Modifier.weight(1f))
        } else {
            Box(
                Modifier.weight(1f).heightIn(min = P.TOUCH.dp).clickable(onClickLabel = "Rename step") { onEdit(true) },
                contentAlignment = Alignment.CenterStart,
            ) {
                Label(
                    step.title,
                    Tokens.Type.STEP,
                    if (step.done) r.ink3 else r.ink,
                    decoration = if (step.done) TextDecoration.LineThrough else null,
                )
            }
        }
        Label("%02d".format(index + 1), Tokens.Type.COUNT, r.ink3)
        menuButton()
    }
}

/** Renames a step in place: Done or leaving the field keeps a changed, non-blank title. */
@Composable
private fun StepTitleField(step: StepRow, actions: Actions, onDone: () -> Unit, modifier: Modifier) {
    val r = LocalDesign.current.room
    val focus = remember { FocusRequester() }
    var text by remember(step.id) { mutableStateOf(androidx.compose.ui.text.input.TextFieldValue(step.title, androidx.compose.ui.text.TextRange(step.title.length))) }
    var focused by remember { mutableStateOf(false) }
    var done by remember { mutableStateOf(false) }
    fun commit() {
        if (done) return
        done = true
        val t = text.text.trim()
        if (t.isNotEmpty() && t != step.title) actions.renameStep(step.id, t)
        onDone()
    }
    LaunchedEffect(Unit) { runCatching { focus.requestFocus() } }
    BasicTextField(
        value = text,
        onValueChange = {
            if ('\n' in it.text) commit() else text = it
        },
        modifier = modifier
            .focusRequester(focus)
            .semantics { contentDescription = "Step title" }
            .onFocusChanged {
                if (focused && !it.isFocused) commit()
                focused = it.isFocused
            },
        textStyle = Tokens.Type.STEP.style(r.ink),
        singleLine = true,
        cursorBrush = SolidColor(r.accent),
        keyboardOptions = KeyboardOptions(capitalization = KeyboardCapitalization.Sentences, imeAction = ImeAction.Done),
        keyboardActions = KeyboardActions(onDone = { commit() }),
        decorationBox = { inner ->
            Box(Modifier.border(Tokens.Elevation.HAIRLINE.dp, r.accent, RoundedCornerShape(Tokens.Radius.SM.dp)).padding(horizontal = S.S2.dp, vertical = S.S1.dp)) {
                inner()
            }
        },
    )
}

/**
 * The property rows. Due opens the date picker; Priority opens its picker, all four levels with the
 * current one ticked; Reminder, Repeat and Tags open their respective pickers. After the
 * owner sets a reminder, the app asks for what reminders need, if anything ([LocalReminderAccess]).
 */
@Composable
private fun Fields(detail: TaskDetail, today: String, actions: Actions) {
    var repeat by rememberSaveable { mutableStateOf(false) }
    var custom by rememberSaveable { mutableStateOf(false) }
    var tags by rememberSaveable { mutableStateOf(false) }
    var pickingDue by rememberSaveable { mutableStateOf(false) }
    var pickingPriority by remember { mutableStateOf(false) }
    var pickingReminder by rememberSaveable { mutableStateOf(false) }
    var asking by remember { mutableStateOf<ReminderNeed?>(null) }
    val access = LocalReminderAccess.current
    detail.fields.forEach { f ->
        when (f.edit) {
            FieldEdit.DUE -> FieldRow(f, "Change the due date") { pickingDue = true }
            FieldEdit.PRIORITY -> Box {
                FieldRow(f, "Change the priority") { pickingPriority = true }
                if (pickingPriority) {
                    MenuPopup(
                        priorityMenu(detail.id, detail.priority.toInt()),
                        { a -> if (a is MenuAction.SetPriority) actions.setPriority(a.task, a.marks) },
                        onDismiss = { pickingPriority = false },
                        start = true,
                        title = "Priority",
                    )
                }
            }
            FieldEdit.REMINDER -> FieldRow(f, if (f.empty) "Add a reminder" else "Change the reminder") { pickingReminder = true }
            FieldEdit.REPEAT -> FieldRow(f, "Change repeat") { custom = false; repeat = true }
            FieldEdit.TAGS -> FieldRow(f, "Edit tags", tags = detail.tags.filter { it.selected }) { tags = true }
            null -> FieldRow(f)
        }
    }
    if (tags) TagsDialog(detail, actions) { tags = false }
    if (repeat) OverlayDialog({ repeat = false }) { overlay ->
        RepeatCard(detail, custom, { custom = true }, { choice -> overlay.choose { actions.organize(app.todoornot.model.OrganizeEdit.Repeat(detail.id, choice)) } }, overlay::close)
    }
    if (pickingDue) {
        DueDateDialog(detail.due, today, detail.repeats, onSave = { actions.setDue(detail.id, it) }, onDismiss = { pickingDue = false })
    }
    if (pickingReminder) {
        val now = remember { System.currentTimeMillis() }
        // The edit carries the reminder it was made on, as the picker opened on it.
        val seen = remember { detail.reminder }
        ReminderDialog(
            seen?.atMs,
            detail.due,
            now,
            ZoneId.systemDefault(),
            onSave = { at ->
                actions.setReminder(detail.id, seen, at)
                if (at != null) asking = access.toAsk()
            },
            onDismiss = { pickingReminder = false },
        )
    }
    asking?.let { need -> ReminderAccessDialog(need, access, onDone = { asking = null }) }
}

@Composable
private fun Caps(text: String) {
    val r = LocalDesign.current.room
    Label(text, Tokens.Type.CAPS_LABEL, r.ink3, Modifier.padding(top = S.S4.dp, bottom = S.S2.dp))
}

/** The title, edited in place: Done or leaving the field renames the task. */
@Composable
private fun TitleField(detail: TaskDetail, actions: Actions, modifier: Modifier) {
    val r = LocalDesign.current.room
    val focus = LocalFocusManager.current
    var text by remember(detail.id, detail.title) { mutableStateOf(detail.title) }
    var focused by remember { mutableStateOf(false) }
    fun commit() {
        val t = text.trim()
        if (t.isNotEmpty() && t != detail.title) actions.rename(detail.id, t) else text = detail.title
    }
    // Opening another task (or closing this one) mid-edit keeps the edit.
    val latest by rememberUpdatedState({ commit() })
    DisposableEffect(detail.id) { onDispose { latest() } }
    BasicTextField(
        value = text,
        onValueChange = {
            // The title is one line: Enter (also from a hardware keyboard) finishes the edit.
            if ('\n' in it) {
                text = it.replace("\n", "")
                focus.clearFocus()
            } else {
                text = it
            }
        },
        modifier = modifier
            .semantics { contentDescription = "Title" }
            .onFocusChanged {
                if (focused && !it.isFocused) commit()
                focused = it.isFocused
            },
        textStyle = Tokens.Type.DETAIL_TITLE_PHONE.style(if (detail.done) r.ink3 else r.ink),
        cursorBrush = SolidColor(r.accent),
        keyboardOptions = KeyboardOptions(imeAction = ImeAction.Done),
        keyboardActions = KeyboardActions(onDone = { focus.clearFocus() }),
    )
}

@Composable
private fun TaskNotes(detail: TaskDetail, actions: Actions) {
    val r = LocalDesign.current.room
    val draft = actions.notesDraft(detail.id)
    Caps("Notes")
    val text = draft.text
    if (text == null) {
        Box(
            Modifier.fillMaxWidth().heightIn(min = P.TOUCH.dp).clickable(onClickLabel = "Edit notes") {
                draft.begin(detail.notes)
            },
            contentAlignment = Alignment.CenterStart,
        ) {
            Label(detail.notes.ifEmpty { "Add notes" }, Tokens.Type.NOTES,
                if (detail.notes.isEmpty()) r.ink3 else r.ink2, maxLines = Int.MAX_VALUE)
        }
    } else {
        NotesEditor(text, draft.saving, draft.problem, draft::edit, onCancel = { draft.edit(null) }, onSave = { draft.save(actions) })
    }
}

@Composable
private fun NotesEditor(text: String, saving: Boolean, problem: String?, onEdit: (String) -> Unit, onCancel: () -> Unit, onSave: () -> Unit) {
    val r = LocalDesign.current.room
    val focus = remember { FocusRequester() }
    LaunchedEffect(Unit) { if (!saving) focus.requestFocus() }
    Column(verticalArrangement = Arrangement.spacedBy(S.S2.dp)) {
        BasicTextField(
            value = text,
            onValueChange = onEdit,
            enabled = !saving,
            modifier = Modifier.fillMaxWidth().heightIn(min = P.TOUCH.dp)
                .border(Tokens.Elevation.HAIRLINE.dp, r.control, RoundedCornerShape(Tokens.Radius.MD.dp))
                .padding(S.S2.dp).focusRequester(focus).semantics { contentDescription = "Notes" },
            textStyle = Tokens.Type.NOTES.style(r.ink),
            cursorBrush = SolidColor(r.accent),
            keyboardOptions = KeyboardOptions(capitalization = KeyboardCapitalization.Sentences),
        )
        if (problem != null) Label(problem, Tokens.Type.META_PHONE, r.dangerInk, maxLines = Int.MAX_VALUE)
        Row(horizontalArrangement = Arrangement.spacedBy(S.S2.dp)) {
            Button("Cancel", false, Modifier.weight(1f), enabled = !saving, onClick = onCancel)
            Button("Save notes", true, Modifier.weight(1f), enabled = !saving, onClick = onSave)
        }
    }
}

/** A step's box: ticking it completes the step. */
@Composable
private fun StepMark(done: Boolean, title: String, onToggle: (Boolean) -> Unit) {
    Box(Modifier.width(P.COL_CHECK.dp), contentAlignment = Alignment.Center) {
        Check(done, onToggle, title, size = C.CHECKBOX.dp - S.S1.dp / 2)
    }
}

/**
 * "+ Add step" at the end of the steps. Done adds it and keeps the field open for the next one. The
 * draft is kept until the step is added; if it is refused (too long, say), the reason shows here, in the
 * detail pane, where a toast would be hidden.
 */
@Composable
private fun AddStep(task: String, actions: Actions) {
    val r = LocalDesign.current.room
    // The draft is the model's only (nothing is saved with the Activity), so it and a submission still
    // on its way survive a rotation, and every pane for this task, compact or wide, shows the same one.
    val draft = actions.stepDraft(task)
    fun submit() = draft.submit { title, done -> actions.addStep(task, title, done) }
    Column(Modifier.fillMaxWidth().hairline(r.line)) {
        Row(
            Modifier.fillMaxWidth().height(P.TOUCH.dp),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(S.S3.dp),
        ) {
            Box(Modifier.width(P.COL_CHECK.dp), contentAlignment = Alignment.Center) {
                Icon("plus", r.accentStrong, size = C.ICON_SMALL.dp)
            }
            BasicTextField(
                value = draft.text,
                onValueChange = draft::edit,
                modifier = Modifier.weight(1f).semantics { contentDescription = "Add step" },
                textStyle = Tokens.Type.STEP.style(r.ink),
                singleLine = true,
                cursorBrush = SolidColor(r.accent),
                keyboardOptions = KeyboardOptions(capitalization = KeyboardCapitalization.Sentences, imeAction = ImeAction.Done),
                keyboardActions = KeyboardActions(onDone = { submit() }),
                decorationBox = { inner ->
                    Box(contentAlignment = Alignment.CenterStart) {
                        if (draft.text.isEmpty()) Label("Add step", Tokens.Type.STEP, r.ink3)
                        inner()
                    }
                },
            )
        }
        draft.problem?.let {
            Label(it, Tokens.Type.META_PHONE, r.dangerInk, Modifier.padding(start = P.COL_CHECK.dp + S.S3.dp, bottom = S.S2.dp), maxLines = 3)
        }
    }
}

/** A property row; one that edits ([onEdit]) is a full touch target with a chevron. */
@Composable
private fun FieldRow(f: DetailField, editLabel: String? = null, tags: List<app.todoornot.core.TagChoice> = emptyList(), onEdit: (() -> Unit)? = null) {
    val r = LocalDesign.current.room
    Row(
        Modifier
            .fillMaxWidth()
            .heightIn(min = if (onEdit != null) P.TOUCH.dp else P.TOUCH.dp - S.S2.dp)
            .hairline(r.line)
            .then(if (onEdit != null) Modifier.clickable(onClickLabel = editLabel, onClick = onEdit) else Modifier),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Box(Modifier.width(S.S6.dp + S.S1.dp)) { Icon(f.icon, r.ink3, size = C.ICON_SMALL.dp) }
        Label(f.label, Tokens.Type.META_PHONE, r.ink3, Modifier.width(S.S10.dp * 2 + S.S3.dp))
        val color = if (f.empty) r.ink4 else r.ink
        Box(Modifier.weight(1f)) {
            if (tags.isNotEmpty()) {
                Row(Modifier.horizontalScroll(rememberScrollState()), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(S.S1.dp)) {
                    tags.forEach { tag ->
                        Box(Modifier.size(C.CHIP_DOT.dp).background(LocalDesign.current.list(tag.hue).dot, CircleShape))
                        Label("#${tag.name}", Tokens.Type.DUE, r.ink)
                    }
                }
            } else if (f.mono) {
                Label(f.value, Tokens.Type.DUE, color, weight = if (f.empty) null else Tokens.Type.OVERDUE_BADGE.weight)
            } else {
                Label(f.value, Tokens.Type.NAV, color, maxLines = Int.MAX_VALUE)
            }
        }
        if (onEdit != null) Icon("chevron-right", r.ink4, size = C.ICON_SMALL.dp)
    }
}

@Composable
internal fun SourceCard(s: SourceLink, remove: () -> Unit) {
    val r = LocalDesign.current.room
    val shape = RoundedCornerShape(Tokens.Radius.LG.dp)
    val context = LocalContext.current
    // The engine returns a canonical HTTP(S) URL, or null for a text-only source.
    val link = s.url
    val open = if (link == null) Modifier else Modifier.clickable(onClickLabel = "Open link") {
        val intent = Intent(Intent.ACTION_VIEW, Uri.parse(link)).addCategory(Intent.CATEGORY_BROWSABLE)
        try {
            context.startActivity(intent)
        } catch (_: ActivityNotFoundException) {
            Toast.makeText(context, "No browser can open this link.", Toast.LENGTH_SHORT).show()
        } catch (_: SecurityException) {
            Toast.makeText(context, "This phone won't open that link.", Toast.LENGTH_SHORT).show()
        }
    }
    Row(
        Modifier
            .fillMaxWidth()
            .padding(bottom = S.S2.dp)
            .clip(shape)
            .background(r.raised)
            .border(Tokens.Elevation.HAIRLINE.dp, r.lineStrong, shape)
            .then(open)
            .heightIn(min = P.TOUCH.dp)
            .padding(horizontal = S.S3.dp, vertical = S.S2.dp + S.S1.dp / 2),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(S.S3.dp),
    ) {
        Box(
            Modifier.size(C.ICON_BUTTON.dp).background(r.wash, RoundedCornerShape(Tokens.Radius.MD.dp)),
            contentAlignment = Alignment.Center,
        ) {
            Icon(s.icon, r.accentStrong, size = C.ICON_SMALL.dp)
        }
        Column(Modifier.weight(1f)) {
            Label(s.title, Tokens.Type.BUTTON_SMALL, r.ink)
            Label(s.subtitle, Tokens.Type.COUNT, r.ink3)
        }
        if (link != null) Icon("arrow-up-right", r.ink3, size = C.ICON_SMALL.dp)
        IconButton("trash-2", "Remove link: ${s.title}", onClick = remove)
    }
}

/** The right pane on the unfolded phone when no task is open. */
@Composable
fun NoDetail(modifier: Modifier = Modifier) {
    val r = LocalDesign.current.room
    Box(modifier.fillMaxSize().background(r.pane).padding(S.S8.dp), contentAlignment = Alignment.Center) {
        Label("Pick a task to see it here.", Tokens.Type.EYEBROW_DAY, r.ink3)
    }
}
