package app.todoornot.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.widthIn
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.lerp
import androidx.compose.ui.semantics.clearAndSetSemantics
import androidx.compose.ui.semantics.selected
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.style.TextDecoration
import androidx.compose.ui.unit.dp
import app.todoornot.core.DueTone
import app.todoornot.core.TaskRow
import app.todoornot.design.Icon
import app.todoornot.design.LocalDesign
import app.todoornot.design.Tokens
import app.todoornot.model.RowLook

import app.todoornot.model.visibleStage

private val P = Tokens.Metrics.Phone
private val S = Tokens.Space

/**
 * A task row on the phone: 64 dp, columns `22 | 1fr | 20 | 24` (check, title and metadata, agent,
 * priority), then the Important touch target. Empty agent and priority columns are left out, so the title
 * uses their room. On a dated row the date sits in a 100 dp slot so list chips line up down the list; an
 * undated row's chips start flush with the title. The row is
 * at least 64 dp and grows with the font scale: its bottom inset is what's left of 64 dp under the
 * title and a chip-high metadata line, so nothing in the second line is ever clipped.
 *
 * [look] is the row's frame of its choreography (null at rest): the disc pops and the tick draws, the
 * title dims and its strike-through draws from the left, and the row fades and collapses. [checked] is
 * whether the task is done as far as a tap is concerned (a held row is done, though it's still listed
 * open). A row that has gone (or hasn't arrived) takes no taps, and neither does an [inert] one (a ghost,
 * painted while its choreography plays out after its room dropped it), which accessibility skips too.
 */
@Composable
fun TaskRowView(
    task: TaskRow,
    selected: Boolean,
    checked: Boolean,
    onCheck: (Boolean) -> Unit,
    onImportant: (Boolean) -> Unit,
    onOpen: () -> Unit,
    onActions: () -> Unit,
    modifier: Modifier = Modifier,
    look: RowLook? = null,
    inert: Boolean = false,
    selecting: Boolean = false,
    longClickLabel: String = "Select task",
    dragHandle: (@Composable () -> Unit)? = null,
) {
    val r = LocalDesign.current.room
    val bulkSelected = selected && selecting
    val quietInk = if (bulkSelected) r.ink2 else r.ink3
    val l = look ?: RowLook.rest(task.done)
    val live = !l.gone && !inert
    val titleLine = Tokens.Type.TASK_TITLE_PHONE.lineHeight.dp
    val hasMeta = (task.chip == null && task.visibleStage() != null) ||
        task.due != null || task.chip != null || task.fromMail || task.steps != null || task.tags.isNotEmpty()
    val m = modifier
        .rowMotion(look)
        .then(if (inert) Modifier.clearAndSetSemantics { } else Modifier)
        .fillMaxWidth()
        .heightIn(min = P.ROW.dp)
        .then(if (selected) Modifier.background(if (bulkSelected) r.selectionWash else r.wash).selectionBar(r.accentStrong) else Modifier)
        .semantics { this.selected = selected }
        .hairline(r.line)
        .combinedClickable(enabled = live, onClick = onOpen, onLongClickLabel = longClickLabel, onLongClick = onActions)
        .padding(start = P.ROW_PAD_LEFT.dp, end = P.ROW_PAD_RIGHT.dp, top = S.S2.dp, bottom = S.S2.dp)
    Row(m, verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(S.S3.dp)) {
        Box(Modifier.width(P.COL_CHECK.dp), contentAlignment = Alignment.Center) {
            Check(
                done = checked,
                onToggle = onCheck,
                label = task.title,
                overdue = task.due?.tone == DueTone.OVERDUE,
                enabled = live && !selecting,
                look = l,
                onWash = selected,
                onSelectionWash = bulkSelected,
            )
        }
        Column(Modifier.weight(1f)) {
            // Let the title's scaled text grow past the design line height; metadata uses the full width below.
            Row(Modifier.fillMaxWidth().heightIn(min = titleLine), horizontalArrangement = Arrangement.spacedBy(S.S3.dp)) {
                Box(Modifier.weight(1f)) { Title(task.title, lerp(r.ink, quietInk, l.dim), l.strike) }
                if (showsAgent(task)) {
                    Box(Modifier.size(P.COL_AGENT.dp, titleLine), contentAlignment = Alignment.Center) {
                        task.agent?.let { AgentBadge(it) }
                    }
                }
                if (showsPriority(task)) {
                    Box(Modifier.widthIn(min = P.COL_PRIORITY.dp).height(titleLine), contentAlignment = Alignment.Center) {
                        Priority(task.priority.toInt(), quietInk = quietInk)
                    }
                }
            }
            if (hasMeta) {
                Spacer(Modifier.height(S.S1.dp))
                MetaLine(task, quietInk)
            }
        }
        TaskToggleButton(TaskToggle.IMPORTANT, task.priority == IMPORTANT, enabled = live && !selecting,
            label = "Important: ${task.title}", alignment = Alignment.Center, onChange = onImportant)
        dragHandle?.invoke()
    }
}

/**
 * The title, struck through by [strike] (0 to 1) from the left: while the strike draws, the part of the
 * title it has reached is drawn struck and the rest plain, so the glyphs never double.
 */
@Composable
private fun Title(text: String, color: Color, strike: Float) {
    val token = Tokens.Type.TASK_TITLE_PHONE
    when {
        strike >= 1f -> Label(text, token, color, decoration = TextDecoration.LineThrough)
        strike <= 0f -> Label(text, token, color)
        else -> Box {
            Label(text, token, color, Modifier.clipShare(strike, 1f))
            Label(text, token, color, Modifier.clipShare(0f, strike), decoration = TextDecoration.LineThrough)
        }
    }
}

/** The agent column is there only for an agent's mark. */
fun showsAgent(task: TaskRow): Boolean = task.agent != null

/** The priority column is there for `!` marks, or to keep an agent's mark in its place. */
fun showsPriority(task: TaskRow): Boolean = showsAgent(task) || task.priority > 0u

/** The metadata line fits a chip or the overdue badge, whichever is taller than the due text. */
private val metaLine = maxOf(Tokens.Type.DUE.lineHeight, Tokens.Metrics.Component.CHIP).dp

@Composable
private fun MetaLine(task: TaskRow, quietInk: Color) {
    Row(
        Modifier.heightIn(min = metaLine),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(S.S2.dp),
    ) {
        // A dated row puts the date first, in a column as wide as the due slot so the chips after it line
        // up down the list. An undated row has no column: its chips start under the title.
        if (task.chip == null) task.visibleStage()?.let { StatusMark(it) }
        task.due?.let { due ->
            Box(Modifier.widthIn(min = P.DUE_SLOT.dp - S.S2.dp)) { TaskDate(task, due) }
        }
        task.chip?.let { chip ->
            ListChip(chip)
            task.visibleStage()?.let { StatusMark(it) }
        }
        if (task.fromMail) Icon("mail", quietInk, size = Tokens.Metrics.Component.ICON_SMALL.dp, label = "From an email")
        task.steps?.let { StepMeter(it, quietInk = quietInk) }
        for (tag in task.tags) TagLabel(tag.label, tag.hue)
    }
}

@Composable
internal fun TaskDate(task: TaskRow, due: app.todoornot.core.DueLabel) {
    val date = if (due.tone == DueTone.OVERDUE) null else task.dateSpan
    Due(if (date == null) due else due.copy(text = date))
}
