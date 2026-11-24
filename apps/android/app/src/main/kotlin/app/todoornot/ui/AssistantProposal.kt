package app.todoornot.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.sizeIn
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.selection.toggleable
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.heading
import androidx.compose.ui.semantics.paneTitle
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.style.TextDecoration
import androidx.compose.ui.unit.dp
import app.todoornot.core.AssistantEffect
import app.todoornot.core.AssistantEffectKind
import app.todoornot.core.AssistantProposal
import app.todoornot.core.AssistantProposalStatus
import app.todoornot.core.Chip
import app.todoornot.design.Icon
import app.todoornot.design.LocalDesign
import app.todoornot.design.Tokens
import app.todoornot.model.AssistantModel

private val S = Tokens.Space
private val T = Tokens.Type
private val P = Tokens.Metrics.Phone

@Composable
internal fun AssistantProposalCard(proposal: AssistantProposal, left: Boolean, enabled: Boolean, model: AssistantModel?, onTask: (String) -> Unit) {
    val r = LocalDesign.current.room
    val shape = RoundedCornerShape(Tokens.Radius.LG.dp)
    val status = proposal.status
    val pending = status == AssistantProposalStatus.PENDING
    val stale = status == AssistantProposalStatus.STALE
    val selected = proposal.items.filter { it.selected }
    val batch = proposal.items.size > 1
    val effects = selected.flatMap { it.effects }
    val highest = effects.maxByOrNull { it.kind.ordinal }?.kind ?: AssistantEffectKind.EDIT
    val color = when (highest) {
        AssistantEffectKind.CREATE -> r.accentStrong
        AssistantEffectKind.EDIT, AssistantEffectKind.COMPLETE -> r.sunInk
        AssistantEffectKind.DELETE -> r.dangerInk
    }
    val icon = when (highest) {
        AssistantEffectKind.CREATE -> "plus"
        AssistantEffectKind.EDIT -> "pencil"
        AssistantEffectKind.COMPLETE -> "circle-check"
        AssistantEffectKind.DELETE -> "trash-2"
    }
    val heading = when {
        batch && effects.all { it.kind == AssistantEffectKind.CREATE } -> "${proposal.items.size} new tasks"
        batch -> "${proposal.items.size} proposed changes"
        highest == AssistantEffectKind.CREATE -> "New task"
        highest == AssistantEffectKind.COMPLETE -> "Change completion"
        highest == AssistantEffectKind.DELETE -> "Delete task"
        else -> "Edit task"
    }
    if (!pending && !stale) {
        val label = when (status) {
            AssistantProposalStatus.REJECTED -> "Rejected"
            AssistantProposalStatus.SUPERSEDED -> "Superseded by a reply"
            AssistantProposalStatus.SAVING, AssistantProposalStatus.QUEUED -> "Approved · waiting to sync"
            AssistantProposalStatus.APPLIED -> "Applied"
            AssistantProposalStatus.FAILED -> "Not applied"
            AssistantProposalStatus.UNDO_REQUESTED -> "Undo requested"
            AssistantProposalStatus.PENDING, AssistantProposalStatus.STALE -> return
        }
        Column(Modifier.background(r.canvas, shape).border(Tokens.Elevation.HAIRLINE.dp, r.line, shape)) {
            Row(Modifier.fillMaxWidth().padding(horizontal = S.S3.dp).heightIn(min = P.TOUCH.dp),
                verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(S.S2.dp)) {
                AssistantMark()
                Label("$label · ${effects.singleOrNull()?.title ?: heading}", T.META_PHONE, r.ink2, Modifier.weight(1f), maxLines = 2)
                if (status == AssistantProposalStatus.APPLIED) AiButton("Undo", { model?.undo(proposal) }, enabled = enabled, quiet = true)
            }
            proposal.problem?.let { AssistantProblem(it) }
        }
        return
    }
    Column(Modifier.fillMaxWidth().clip(shape).background(r.raised)
        .border(Tokens.Elevation.HAIRLINE.dp, r.lineStrong, shape).selectionBar(color)
        .semantics { paneTitle = heading }) {
        Row(Modifier.fillMaxWidth().padding(horizontal = S.S2.dp).hairline(r.line)
            .padding(horizontal = S.S2.dp, vertical = S.S2.dp)
            .heightIn(min = S.S6.dp), horizontalArrangement = Arrangement.spacedBy(S.S2.dp), verticalAlignment = Alignment.CenterVertically) {
            Icon(icon, color, size = Tokens.Metrics.Component.ICON_SMALL.dp)
            Label(heading, T.CAPS_LABEL, color, Modifier.weight(1f).semantics { heading() }, maxLines = 2)
        }
        Column(Modifier.fillMaxWidth().padding(horizontal = S.S4.dp, vertical = S.S2.dp),
            verticalArrangement = Arrangement.spacedBy(S.S2.dp)) {
            if (batch) {
                proposal.items.forEach { item ->
                    var expanded by rememberSaveable(item.index) { mutableStateOf(false) }
                    val title = item.effects.firstOrNull()?.title ?: "Change ${item.index + 1u}"
                    Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) {
                        val canToggle = pending && enabled && (!item.selected || selected.size > 1)
                        Box(Modifier.sizeIn(minWidth = P.TOUCH.dp, minHeight = P.TOUCH.dp)
                            .toggleable(item.selected, enabled = canToggle, role = Role.Checkbox, onValueChange = { on ->
                                val indices = selected.map { it.index }.filter { it != item.index } + if (on) listOf(item.index) else emptyList()
                                model?.select(proposal, indices)
                            }).semantics { contentDescription = "Include $title" }.padding(horizontal = S.S3.dp),
                            contentAlignment = Alignment.Center) {
                            AccessCheck(item.selected)
                        }
                        Row(Modifier.weight(1f).heightIn(min = P.TOUCH.dp)
                            .clickable(role = Role.Button, onClickLabel = if (expanded) "Hide change details" else "Review change details") { expanded = !expanded },
                            verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(S.S1.dp)) {
                            Column(Modifier.weight(1f).padding(vertical = S.S1.dp)) {
                                Label(title, T.TASK_TITLE, if (item.selected) r.ink else r.ink3, maxLines = 2)
                                if (!expanded) item.effects.firstOrNull()?.let { ListChip(Chip(it.list, it.listHue)) }
                            }
                            Icon(if (expanded) "chevron-down" else "chevron-right", r.ink3)
                        }
                    }
                    if (expanded) item.effects.forEach { EffectDetails(it, onTask) }
                }
            } else effects.forEach { EffectDetails(it, onTask) }
            if (stale) AssistantProblem("Tasks changed since this proposal. Reply to request a new version.")
        }
        Row(Modifier.fillMaxWidth().padding(horizontal = S.S2.dp).hairline(r.line, top = true)
            .padding(horizontal = S.S1.dp, vertical = S.S3.dp),
            horizontalArrangement = Arrangement.spacedBy(S.S2.dp), verticalAlignment = Alignment.CenterVertically) {
            val reject: @Composable () -> Unit = {
                AiButton(if (batch) "Reject all" else "Reject", { model?.reject(proposal) }, Modifier.weight(1f),
                    icon = "x", enabled = enabled)
            }
            val approve: @Composable () -> Unit = {
                AiButton(if (batch) "Approve ${selected.size}" else if (highest == AssistantEffectKind.CREATE) "Approve" else "Approve edit",
                    { model?.approve(proposal) }, Modifier.weight(1f), icon = "check", enabled = enabled && pending && highest != AssistantEffectKind.DELETE, primary = true)
            }
            if (left && pending) { approve(); reject() } else { reject(); if (pending) approve() }
        }
    }
}

@Composable
private fun EffectDetails(effect: AssistantEffect, onTask: (String) -> Unit) {
    val r = LocalDesign.current.room
    Column(Modifier.fillMaxWidth(), verticalArrangement = Arrangement.spacedBy(S.S1.dp)) {
        // Never truncate a field on a surface that grants authority for that exact change.
        val task = effect.target
        if (task == null) Label(effect.title, T.TASK_TITLE, r.ink, maxLines = Int.MAX_VALUE)
        else AiButton(effect.title, { onTask(task) }, Modifier.fillMaxWidth(), icon = "arrow-up-right", maxLines = Int.MAX_VALUE)
        // Keep the entire target list label available, even on a narrow pane at large text scale.
        val listColors = LocalDesign.current.list(effect.listHue)
        Label(effect.list, T.CHIP, listColors.ink, Modifier.background(listColors.tint, RoundedCornerShape(Tokens.Radius.CHIP.dp))
            .padding(horizontal = Tokens.Metrics.Component.CHIP_PAD_X.dp), maxLines = Int.MAX_VALUE)
        effect.fields.forEach { field ->
            Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(S.S2.dp)) {
                Label(field.label, T.META_PHONE, r.ink3, Modifier.weight(1f), maxLines = Int.MAX_VALUE)
                Column(Modifier.weight(3f)) {
                    val token = if (field.label == "Due" || field.label == "Priority") T.COUNT else T.BODY
                    field.before?.let { Label(it, token, r.ink3, maxLines = Int.MAX_VALUE, decoration = TextDecoration.LineThrough) }
                    Label(if (field.before != null) "→ ${field.after}" else field.after, token, r.ink,
                        maxLines = Int.MAX_VALUE, weight = T.BUTTON.weight)
                }
            }
        }
    }
}
