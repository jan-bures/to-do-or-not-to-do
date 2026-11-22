package app.todoornot.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
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
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.focus.onFocusChanged
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.platform.LocalFocusManager
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.style.TextDecoration
import androidx.compose.ui.unit.dp
import app.todoornot.core.DetailField
import app.todoornot.core.SourceLink
import app.todoornot.core.StepCount
import app.todoornot.core.TaskDetail
import app.todoornot.design.Icon
import app.todoornot.design.LocalDesign
import app.todoornot.design.Tokens
import app.todoornot.design.style
import app.todoornot.model.Actions

private val P = Tokens.Metrics.Phone
private val S = Tokens.Space
private val C = Tokens.Metrics.Component

/**
 * A task's detail: breadcrumb, title (editable), My Day, steps, notes, fields and sources. Beside the
 * list on the unfolded phone; on its own, with a back arrow, on the cover screen.
 */
@Composable
fun DetailPane(detail: TaskDetail, actions: Actions, standalone: Boolean, modifier: Modifier = Modifier) {
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
            if (!standalone) IconButton("x", "Close") { actions.select(null) }
        }
        Column(
            Modifier
                .weight(1f)
                .verticalScroll(rememberScrollState())
                .padding(start = S.S6.dp, end = S.S6.dp, top = S.S4.dp + S.S1.dp / 2)
                .navigationBarsPadding(),
        ) {
            Row(verticalAlignment = Alignment.Top, horizontalArrangement = Arrangement.spacedBy(S.S3.dp)) {
                Box(Modifier.size(P.COL_CHECK.dp, Tokens.Type.DETAIL_TITLE_PHONE.lineHeight.dp), contentAlignment = Alignment.Center) {
                    Check(detail.done, { actions.setDone(detail.id, it) }, detail.title)
                }
                TitleField(detail, actions, Modifier.weight(1f))
            }
            Label(
                detail.subtitle,
                Tokens.Type.COUNT,
                r.ink3,
                Modifier.padding(start = P.COL_CHECK.dp + S.S3.dp, top = S.S1.dp, bottom = S.S4.dp),
            )
            MyDayButton(detail.myDay) { actions.setMyDay(detail.id, !detail.myDay) }
            if (detail.steps.isNotEmpty()) {
                val done = detail.steps.count { it.done }
                Row(
                    Modifier.fillMaxWidth().padding(top = S.S5.dp).height(S.S6.dp).hairline(r.line),
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    Label("Steps", Tokens.Type.CAPS_LABEL, r.ink3, Modifier.weight(1f))
                    StepMeter(StepCount(done.toUInt(), detail.steps.size.toUInt()))
                }
                detail.steps.forEachIndexed { i, step ->
                    Row(
                        Modifier.fillMaxWidth().height(P.TOUCH.dp - S.S2.dp).hairline(r.line),
                        verticalAlignment = Alignment.CenterVertically,
                        horizontalArrangement = Arrangement.spacedBy(S.S3.dp),
                    ) {
                        StepMark(step.done)
                        Label(
                            step.title,
                            Tokens.Type.STEP,
                            if (step.done) r.ink3 else r.ink,
                            Modifier.weight(1f),
                            decoration = if (step.done) TextDecoration.LineThrough else null,
                        )
                        Label("%02d".format(i + 1), Tokens.Type.COUNT, r.ink3)
                    }
                }
            }
            if (detail.notes.isNotBlank()) {
                Caps("Notes")
                Label(detail.notes, Tokens.Type.NOTES, r.ink2, maxLines = Int.MAX_VALUE)
            }
            if (detail.fields.isNotEmpty()) {
                Spacer(Modifier.height(S.S4.dp))
                Box(Modifier.fillMaxWidth().hairline(r.line, top = true))
                detail.fields.forEach { FieldRow(it) }
            }
            if (detail.sources.isNotEmpty()) {
                Caps("Source")
                detail.sources.forEach { SourceCard(it) }
            }
            Spacer(Modifier.height(S.S8.dp))
        }
    }
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
private fun MyDayButton(on: Boolean, onClick: () -> Unit) {
    val r = LocalDesign.current.room
    val shape = RoundedCornerShape(Tokens.Radius.MD.dp)
    Row(
        Modifier
            .height(C.BUTTON_SMALL.dp)
            .clip(shape)
            .background(if (on) r.tint else r.raised)
            .border(Tokens.Elevation.HAIRLINE.dp, if (on) r.tint else r.lineStrong, shape)
            .clickable(onClick = onClick)
            .padding(horizontal = S.S2.dp + S.S1.dp / 2),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(S.S2.dp),
    ) {
        Icon("sun", r.accentStrong, size = C.ICON_SMALL.dp)
        Label(if (on) "In My Day" else "Add to My Day", Tokens.Type.BUTTON_SMALL, if (on) r.accentStrong else r.ink2)
    }
}

/** A step's mark (steps are read-only on the phone for now). */
@Composable
private fun StepMark(done: Boolean) {
    Box(Modifier.width(P.COL_CHECK.dp), contentAlignment = Alignment.Center) {
        Check(done, {}, "Step", size = C.CHECKBOX.dp - S.S1.dp / 2, enabled = false)
    }
}

@Composable
private fun FieldRow(f: DetailField) {
    val r = LocalDesign.current.room
    Row(
        Modifier.fillMaxWidth().height(P.TOUCH.dp - S.S2.dp).hairline(r.line),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Box(Modifier.width(S.S6.dp + S.S1.dp)) { Icon(f.icon, r.ink3, size = C.ICON_SMALL.dp) }
        Label(f.label, Tokens.Type.META_PHONE, r.ink3, Modifier.width(S.S10.dp * 2 + S.S3.dp))
        val color = if (f.empty) r.ink4 else r.ink
        if (f.mono) {
            Label(f.value, Tokens.Type.DUE, color, weight = if (f.empty) null else Tokens.Type.OVERDUE_BADGE.weight)
        } else {
            Label(f.value, Tokens.Type.NAV, color)
        }
    }
}

@Composable
private fun SourceCard(s: SourceLink) {
    val r = LocalDesign.current.room
    val shape = RoundedCornerShape(Tokens.Radius.LG.dp)
    Row(
        Modifier
            .fillMaxWidth()
            .padding(bottom = S.S2.dp)
            .clip(shape)
            .background(r.raised)
            .border(Tokens.Elevation.HAIRLINE.dp, r.lineStrong, shape)
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
        Icon("arrow-up-right", r.ink3, size = C.ICON_SMALL.dp)
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
