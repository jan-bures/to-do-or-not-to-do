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
import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.core.tween
import androidx.compose.animation.expandVertically
import androidx.compose.animation.shrinkVertically
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.isImeVisible
import androidx.compose.foundation.layout.imePadding
import androidx.compose.runtime.Stable
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
import androidx.compose.runtime.saveable.listSaver
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
import androidx.compose.runtime.SideEffect
import androidx.compose.ui.semantics.clearAndSetSemantics
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.heading
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.semantics.stateDescription
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.KeyboardCapitalization
import androidx.compose.ui.text.style.TextDecoration
import androidx.compose.ui.unit.dp
import app.todoornot.core.DetailField
import app.todoornot.core.FieldEdit
import app.todoornot.core.StepRow
import app.todoornot.core.SourceLink
import app.todoornot.core.StepCount
import app.todoornot.core.TaskActivityLine
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
import app.todoornot.session.ThumbHand
import java.time.ZoneId
import kotlinx.coroutines.delay

private val P = Tokens.Metrics.Phone
private val S = Tokens.Space
private val C = Tokens.Metrics.Component

/**
 * A task's detail: breadcrumb, title (editable), steps (tick, rename, move or delete them, or add one),
 * notes, fields and sources. The top is read-only; the bottom bar holds the thumb actions.
 * [stepEditing] starts with that step's title being renamed (the gallery).
 */
@Composable
fun DetailPane(
    state: UiState,
    actions: Actions,
    standalone: Boolean,
    modifier: Modifier = Modifier,
    stepEditing: String? = null,
    hand: ThumbHand = ThumbHand.RIGHT,
    sheets: DetailSheets? = null,
) {
    val detail = state.detail ?: return
    val generation = state.generation
    val r = LocalDesign.current.room
    val editors = sheets ?: rememberDetailSheets(detail.id, generation, stepEditing)
    val keyboard = WindowInsets.isImeVisible
    val focus = LocalFocusManager.current
    fun open(page: DetailPage, trigger: FocusRequester?) {
        focus.clearFocus()
        editors.returnFocus = trigger
        if (page == DetailPage.Reminder) {
            editors.seenReminder = detail.reminder
            editors.reminderDue = detail.due
        }
        editors.page = page
    }
    LaunchedEffect(editors.page) {
        if (editors.page == null) {
            val trigger = editors.returnFocus
            editors.returnFocus = null
            runCatching { trigger?.requestFocus() }
        }
    }
    val taskActions = remember { FocusRequester() }
    Box(modifier.fillMaxSize()) {
    Column(Modifier.fillMaxSize().background(r.pane).then(if (standalone) Modifier.statusBarsPadding() else Modifier)
        .navigationBarsPadding().imePadding()) {
        Row(
            Modifier
                .fillMaxWidth()
                .heightIn(min = P.DETAIL_BREADCRUMB.dp)
                .hairline(r.line)
                .padding(horizontal = S.S5.dp),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(S.S2.dp),
        ) {
            detail.breadcrumb.forEachIndexed { i, c ->
                if (i > 0) Label("/", Tokens.Type.META_PHONE, r.ink4)
                if (i == 0) ListChip(c) else Label(c.label, Tokens.Type.BUTTON_SMALL, r.ink)
            }
        }
        Column(
            Modifier.weight(1f).fillMaxWidth().verticalScroll(rememberScrollState())
                .padding(start = S.S6.dp, end = S.S6.dp, top = S.S4.dp + S.S1.dp / 2),
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
            key(detail.id) { Steps(detail, actions, editors) { id, trigger -> open(DetailPage.Step(id), trigger) } }
            AddStep(detail.id, actions)
            key(detail.id) { TaskNotes(detail, actions) }
            if (detail.fields.isNotEmpty()) {
                Spacer(Modifier.height(S.S4.dp))
                Box(Modifier.fillMaxWidth().hairline(r.line, top = true))
                key(detail.id) { Fields(detail, editors.page, ::open) }
            }
            if (detail.sources.isNotEmpty()) {
                Caps("Source")
                detail.sources.forEach { s -> SourceCard(s) { actions.organize(app.todoornot.model.OrganizeEdit.RemoveLink(s.id)) } }
            }
            Choice("Add link…") { open(DetailPage.Link, null) }
            Attachments(state, actions)
            TaskActivity(detail.activity)
            Spacer(Modifier.height(S.S8.dp))
        }
        if (standalone) ToastStrip(state.toast, actions)
        AnimatedVisibility(!keyboard, enter = expandVertically(tween(Tokens.Motion.PANEL)), exit = shrinkVertically(tween(Tokens.Motion.PANEL))) {
            DetailActionBar(state, actions, standalone, hand, editors.page == DetailPage.Actions, Modifier.focusRequester(taskActions)) {
                open(DetailPage.Actions, taskActions)
            }
        }
    }
    if (sheets == null) DetailOverlays(state, actions, editors)
    }
}

@Composable
internal fun DetailActionBar(state: UiState, actions: Actions, standalone: Boolean, hand: ThumbHand,
    menuOpen: Boolean = false, menuModifier: Modifier = Modifier, openMenu: () -> Unit,
) {
    val detail = state.detail ?: return
    val r = LocalDesign.current.room
    val left = standalone && hand == ThumbHand.LEFT
    val complete: @Composable () -> Unit = {
        Box(Modifier.size(P.TOUCH.dp), contentAlignment = Alignment.Center) {
            Check(detail.done, { actions.setDone(detail.id, it, state.generation, row = false) }, "Task completion",
                modifier = Modifier.size(P.TOUCH.dp), enabled = !state.signedOut)
        }
    }
    val day: @Composable () -> Unit = { TaskToggleButton(TaskToggle.MY_DAY, detail.myDay, enabled = !state.signedOut) { actions.setMyDay(detail.id, it) } }
    val star: @Composable () -> Unit = { TaskToggleButton(TaskToggle.IMPORTANT, detail.priority == IMPORTANT, enabled = !state.signedOut) { actions.setImportant(detail.id, it) } }
    val more: @Composable () -> Unit = { IconButton("ellipsis", "Task actions", expanded = menuOpen, modifier = menuModifier, onClick = openMenu) }
    val back: @Composable () -> Unit = { IconButton("chevron-left", "Back") { actions.select(null) } }
    Row(Modifier.fillMaxWidth().height(P.BOTTOM_BAR.dp).background(r.canvas).hairline(r.line, top = true).padding(horizontal = S.S1.dp),
        verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(S.S1.dp)) {
        key(detail.id) {
            when {
                left -> { more(); star(); day(); complete(); Spacer(Modifier.weight(1f)); back() }
                standalone -> { back(); Spacer(Modifier.weight(1f)); complete(); day(); star(); more() }
                else -> { complete(); Spacer(Modifier.weight(1f)); day(); star(); more(); IconButton("x", "Close") { actions.select(null) } }
            }
        }
    }
}

@Composable
private fun TaskActivity(lines: List<TaskActivityLine>) {
    val r = LocalDesign.current.room
    Label(
        "Activity", Tokens.Type.CAPS_LABEL, r.ink3,
        Modifier.padding(top = S.S6.dp, bottom = S.S2.dp).semantics { heading() },
    )
    Column(Modifier.fillMaxWidth().hairline(r.line, top = true)) {
        lines.forEach { line ->
            Row(
                Modifier.fillMaxWidth().heightIn(min = Tokens.Metrics.Desktop.PROP_ROW.dp).padding(vertical = S.S1.dp),
                horizontalArrangement = Arrangement.spacedBy(S.S3.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                val decoration = Modifier.clearAndSetSemantics { }
                val agent = line.agent
                val icon = line.icon
                if (agent != null) {
                    AgentBadge(agent, decoration)
                } else {
                    Box(
                        decoration.size(C.AGENT_MARK.dp).background(r.raised, CircleShape)
                            .border(Tokens.Elevation.HAIRLINE.dp, r.lineStrong, CircleShape),
                        contentAlignment = Alignment.Center,
                    ) {
                        if (icon == null) Label("Y", Tokens.Type.AGENT_LETTER, r.ink2)
                        else Icon(icon, r.ink2, size = C.ICON_SMALL.dp)
                    }
                }
                Label("${line.who} ${line.action}", Tokens.Type.STEP, r.ink2, Modifier.weight(1f), maxLines = Int.MAX_VALUE)
                Label(line.relative, Tokens.Type.TAG, r.ink3)
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
private fun Steps(detail: TaskDetail, actions: Actions, editors: DetailSheets, onMenu: (String, FocusRequester) -> Unit) {
    // The steps a local move swaps: only they glide (a change from elsewhere never moves rows).
    var moving by remember { mutableStateOf(emptySet<String>()) }
    LaunchedEffect(moving) {
        if (moving.isNotEmpty()) {
            delay(Tokens.Motion.ROW_ENTER * 4L)
            moving = emptySet()
        }
    }
    LaunchedEffect(editors.movingSteps) {
        if (editors.movingSteps.isNotEmpty()) {
            moving = editors.movingSteps
            editors.movingSteps = emptySet()
        }
    }
    detail.steps.forEachIndexed { i, step ->
        key(step.id) {
            StepLine(
                step,
                i,
                editing = editors.editingStep == step.id,
                onEdit = { on -> editors.editingStep = if (on) step.id else null },
                actions = actions,
                modifier = Modifier.slideOnMove(step.id in moving),
            ) {
                val trigger = remember { FocusRequester() }
                IconButton("ellipsis", "Step actions: ${step.title}", tint = LocalDesign.current.room.ink3,
                    modifier = Modifier.focusRequester(trigger)) { onMenu(step.id, trigger) }
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
private fun Fields(detail: TaskDetail, page: DetailPage?, open: (DetailPage, FocusRequester?) -> Unit) {
    if (detail.workflow.features.workflow) {
        val focus = remember { FocusRequester() }
        val stage = detail.workflow.stage
        FieldRow(DetailField(null, stage.icon, "Status", "", false, false), "Change status",
            modifier = Modifier.focusRequester(focus), valueContent = { StatusMark(stage) }) { open(DetailPage.Status, focus) }
    }
    if (detail.workflow.features.timeline) {
        val focus = remember { FocusRequester() }
        FieldRow(DetailField(null, "calendar-range", "Starts", detail.startLabel ?: "–", mono = true, empty = detail.startLabel == null),
            "Change the start date", expanded = page == DetailPage.Starts, modifier = Modifier.focusRequester(focus)) {
            open(DetailPage.Starts, focus)
        }
    }
    detail.fields.forEach { f -> key(f.label) {
        val focus = remember { FocusRequester() }
        val target = Modifier.focusRequester(focus)
        when (f.edit) {
            FieldEdit.DUE -> FieldRow(f, "Change the due date", expanded = page == DetailPage.Due, modifier = target) { open(DetailPage.Due, focus) }
            FieldEdit.PRIORITY -> FieldRow(f, "Change the priority", modifier = target) { open(DetailPage.Priority, focus) }
            FieldEdit.REMINDER -> FieldRow(f, if (f.empty) "Add a reminder" else "Change the reminder", modifier = target) { open(DetailPage.Reminder, focus) }
            FieldEdit.REPEAT -> FieldRow(f, "Change repeat", modifier = target) { open(DetailPage.Repeat, focus) }
            FieldEdit.TAGS -> FieldRow(f, "Edit tags", tags = detail.tags.filter { it.selected }, modifier = target) { open(DetailPage.Tags, focus) }
            null -> FieldRow(f)
        }
    } }
}

internal sealed interface DetailPage {
    data object Actions : DetailPage
    data object Due : DetailPage
    data object Status : DetailPage
    data object Starts : DetailPage
    data object Priority : DetailPage
    data object Reminder : DetailPage
    data object Repeat : DetailPage
    data object Tags : DetailPage
    data object Link : DetailPage
    data class Step(val id: String) : DetailPage
}

@Stable
class DetailSheets(step: String? = null) {
    internal var page by mutableStateOf<DetailPage?>(null)
    internal var editingStep by mutableStateOf(step)
    internal var movingSteps by mutableStateOf(emptySet<String>())
    internal var returnFocus: FocusRequester? = null
    internal var seenReminder: app.todoornot.core.ReminderRef? = null
    internal var reminderDue: String? = null
}

private val DetailEditorsSaver = listSaver<DetailSheets, String>(
    save = { listOf(it.editingStep.orEmpty(), when (val page = it.page) {
        DetailPage.Actions -> "actions"
        DetailPage.Due -> "due"
        DetailPage.Status -> "status"
        DetailPage.Starts -> "starts"
        DetailPage.Priority -> "priority"
        DetailPage.Reminder -> "reminder"
        DetailPage.Repeat -> "repeat"
        DetailPage.Tags -> "tags"
        DetailPage.Link -> "link"
        is DetailPage.Step -> "step:${page.id}"
        null -> ""
    }, it.seenReminder?.id.orEmpty(), it.seenReminder?.atMs?.toString().orEmpty(), it.reminderDue.orEmpty()) },
    restore = { saved -> DetailSheets(saved[0].ifEmpty { null }).apply {
        val page = saved[1]
        seenReminder = saved[2].takeIf { it.isNotEmpty() }?.let { id -> saved[3].toLongOrNull()?.let { app.todoornot.core.ReminderRef(id, it) } }
        reminderDue = saved[4].ifEmpty { null }
        this.page = when (page) {
            "actions" -> DetailPage.Actions
            "due" -> DetailPage.Due
            "status" -> DetailPage.Status
            "starts" -> DetailPage.Starts
            "priority" -> DetailPage.Priority
            "reminder" -> DetailPage.Reminder
            "repeat" -> DetailPage.Repeat
            "tags" -> DetailPage.Tags
            "link" -> DetailPage.Link
            else -> if (page.startsWith("step:")) DetailPage.Step(page.removePrefix("step:")) else null
        }
    } },
)

@Composable
internal fun rememberDetailSheets(task: String?, generation: Long, step: String? = null) =
    rememberSaveable(task, generation, saver = DetailEditorsSaver) { DetailSheets(step) }

/** Rendered at the window root, outside the scrolling detail and pane constraints. */
@Composable
internal fun DetailOverlays(state: UiState, actions: Actions, sheets: DetailSheets) {
    val detail = state.detail ?: return
    val close = { sheets.page = null }
    var asking by remember(detail.id) { mutableStateOf<ReminderNeed?>(null) }
    val access = LocalReminderAccess.current
    when (val page = sheets.page) {
        DetailPage.Actions -> TaskActionsSheet(state, actions, detail, close)
        DetailPage.Due -> DueDateSheet(detail, actions, close)
        DetailPage.Starts -> if (detail.workflow.features.timeline) StartDateSheet(detail, actions, close) else SideEffect(close)
        DetailPage.Status -> if (detail.workflow.features.workflow) BottomSheet("Status", close) { overlay ->
            PickerCard("Status", inSheet = true) {
                StatusOptions(detail.workflow.stage, includeDone = true) { stage ->
                    overlay.choose { if (stage != detail.workflow.stage) actions.setTaskStage(detail.list, detail.id, stage, state.generation) }
                }
                Button("Cancel", false, onClick = overlay::close)
            }
        } else SideEffect(close)
        DetailPage.Priority -> BottomSheet("Priority", close) { overlay ->
            ActionSheetContent(priorityMenu(detail.id, detail.priority.toInt()), "Priority") { action ->
                if (action is MenuAction.SetPriority) overlay.choose { actions.setPriority(action.task, action.marks) }
            }
        }
        DetailPage.Repeat -> RepeatSheet(detail, actions, close)
        DetailPage.Tags -> TagsSheet(detail, actions, close)
        DetailPage.Reminder -> {
            val now = remember(detail.id) { System.currentTimeMillis() }
            // A sync may change this reminder while the sheet is up. Commit what was shown.
            val seen = sheets.seenReminder
            BottomSheet("Reminder", close) { overlay ->
                ReminderCard(seen?.atMs, sheets.reminderDue, now, ZoneId.systemDefault(), dateFormat = actions.composerNow().dateFormat, onSave = { at ->
                    overlay.choose {
                        actions.setReminder(detail.id, seen, at)
                        if (at != null) asking = access.toAsk()
                    }
                }, onCancel = overlay::close, inSheet = true)
            }
        }
        DetailPage.Link -> OverlayDialog(close) { overlay ->
            RenameCard("Add a web link", "", { address ->
                overlay.choose { actions.organize(app.todoornot.model.OrganizeEdit.AddLink(detail.id, address)) }
            }, overlay::close, webAddress = true)
        }
        is DetailPage.Step -> BottomSheet("Step actions", close) { overlay ->
            ActionSheetContent(stepMenu(detail.steps, page.id), "Step actions") { action ->
                overlay.choose {
                    when (action) {
                        is MenuAction.RenameStep -> { sheets.returnFocus = null; sheets.editingStep = action.step }
                        is MenuAction.MoveStep -> {
                            val i = detail.steps.indexOfFirst { it.id == action.step }
                            val other = detail.steps.getOrNull(i + action.by)
                            if (i >= 0 && other != null) {
                                sheets.movingSteps = setOf(action.step, other.id)
                                actions.moveStep(action.step, action.by)
                            }
                        }
                        is MenuAction.DeleteStep -> actions.deleteStep(action.step)
                        else -> Unit
                    }
                }
            }
        }
        null -> Unit
    }
    asking?.let { need -> ReminderAccessDialog(need, access, onDone = { asking = null }) }
}

@Composable
internal fun Caps(text: String) {
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
private fun FieldRow(f: DetailField, editLabel: String? = null, tags: List<app.todoornot.core.TagChoice> = emptyList(), expanded: Boolean? = null,
    modifier: Modifier = Modifier, valueContent: (@Composable () -> Unit)? = null, onEdit: (() -> Unit)? = null,
) {
    val r = LocalDesign.current.room
    Row(
        modifier.fillMaxWidth()
            .heightIn(min = if (onEdit != null) P.TOUCH.dp else P.TOUCH.dp - S.S2.dp)
            .hairline(r.line)
            .then(if (expanded != null) Modifier.semantics { stateDescription = if (expanded) "Expanded" else "Collapsed" } else Modifier)
            .then(if (onEdit != null) Modifier.clickable(onClickLabel = editLabel, onClick = onEdit) else Modifier),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Box(Modifier.width(S.S6.dp + S.S1.dp)) { Icon(f.icon, r.ink3, size = C.ICON_SMALL.dp) }
        Label(f.label, Tokens.Type.META_PHONE, r.ink3, Modifier.width(S.S10.dp * 2 + S.S3.dp))
        val color = if (f.empty) r.ink4 else r.ink
        Box(Modifier.weight(1f)) {
            if (valueContent != null) valueContent()
            else if (tags.isNotEmpty()) {
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
fun NoDetail(modifier: Modifier = Modifier, selection: String? = null) {
    val r = LocalDesign.current.room
    Column(modifier.fillMaxSize().background(r.pane).padding(S.S8.dp)) {
        if (selection != null) Spacer(Modifier.weight(2f))
        Box(Modifier.fillMaxWidth().weight(1f), contentAlignment = Alignment.Center) {
            Label(selection ?: "Pick a task to see it here.", Tokens.Type.EYEBROW_DAY, if (selection == null) r.ink3 else r.accentStrong)
        }
    }
}
