package app.todoornot.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
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
import androidx.compose.foundation.layout.widthIn
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.style.TextDecoration
import androidx.compose.ui.unit.dp
import app.todoornot.core.DueTone
import app.todoornot.core.TaskRow
import app.todoornot.design.Icon
import app.todoornot.design.LocalDesign
import app.todoornot.design.Tokens

private val P = Tokens.Metrics.Phone
private val S = Tokens.Space

/**
 * A task row on the phone: 64 dp, columns `22 | 1fr | 20 | 24` (check, title and metadata, agent,
 * priority). On a dated row the date sits in a 100 dp slot so list chips line up down the list; an
 * undated row's chips start flush with the title. The row is
 * at least 64 dp and grows with the font scale: its bottom inset is what's left of 64 dp under the
 * title and a chip-high metadata line, so nothing in the second line is ever clipped.
 */
@Composable
fun TaskRowView(
    task: TaskRow,
    selected: Boolean,
    checked: Boolean,
    onCheck: (Boolean) -> Unit,
    onOpen: () -> Unit,
    modifier: Modifier = Modifier,
) {
    val r = LocalDesign.current.room
    val titleLine = Tokens.Type.TASK_TITLE_PHONE.lineHeight.dp
    val bottom = (P.ROW.dp - P.ROW_PAD_TOP.dp - titleLine - S.S1.dp - metaLine).coerceAtLeast(0.dp)
    val m = modifier
        .fillMaxWidth()
        .heightIn(min = P.ROW.dp)
        .then(if (selected) Modifier.background(r.wash).selectionBar(r.accent) else Modifier)
        .hairline(r.line)
        .clickable(onClick = onOpen)
        .padding(start = P.ROW_PAD_LEFT.dp, end = P.ROW_PAD_RIGHT.dp, top = P.ROW_PAD_TOP.dp, bottom = bottom)
    Row(m, horizontalArrangement = Arrangement.spacedBy(S.S3.dp)) {
        Box(Modifier.size(P.COL_CHECK.dp, titleLine), contentAlignment = Alignment.Center) {
            Check(
                done = checked,
                onToggle = onCheck,
                label = task.title,
                overdue = task.due?.tone == DueTone.OVERDUE,
            )
        }
        Column(Modifier.weight(1f)) {
            Label(
                task.title,
                Tokens.Type.TASK_TITLE_PHONE,
                if (task.done) r.ink3 else r.ink,
                decoration = if (task.done) TextDecoration.LineThrough else null,
            )
            Spacer(Modifier.height(S.S1.dp))
            MetaLine(task)
        }
        Box(Modifier.size(P.COL_AGENT.dp, titleLine), contentAlignment = Alignment.Center) {
            task.agent?.let { AgentBadge(it) }
        }
        Box(Modifier.widthIn(min = P.COL_PRIORITY.dp).height(titleLine), contentAlignment = Alignment.Center) {
            Priority(task.priority.toInt())
        }
    }
}

/** The metadata line fits a chip or the overdue badge, whichever is taller than the due text. */
private val metaLine = maxOf(Tokens.Type.DUE.lineHeight, Tokens.Metrics.Component.CHIP).dp

@Composable
private fun MetaLine(task: TaskRow) {
    val r = LocalDesign.current.room
    Row(
        Modifier.heightIn(min = metaLine),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(S.S2.dp),
    ) {
        // A dated row puts the date first, in a column as wide as the due slot so the chips after it line
        // up down the list. An undated row has no column: its chips start under the title.
        task.due?.let { due ->
            Box(Modifier.widthIn(min = P.DUE_SLOT.dp - S.S2.dp)) { Due(due) }
        }
        task.chip?.let { ListChip(it) }
        if (task.fromMail) Icon("mail", r.ink3, size = Tokens.Metrics.Component.ICON_SMALL.dp, label = "From an email")
        task.steps?.let { StepMeter(it) }
        for (tag in task.tags) Label(tag, Tokens.Type.TAG, r.ink3)
    }
}
