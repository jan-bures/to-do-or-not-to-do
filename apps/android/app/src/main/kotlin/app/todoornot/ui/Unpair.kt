package app.todoornot.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.remember
import androidx.compose.ui.Modifier
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.semantics.paneTitle
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.dp
import androidx.compose.ui.window.Dialog
import app.todoornot.design.LocalDesign
import app.todoornot.design.Tokens

private val S = Tokens.Space

/**
 * Asks before unpairing, saying what would be lost. Cancel is the default: it is first, filled and
 * focused, and Back or a tap outside cancels too. With unsent changes the other button reads "Unpair
 * and discard" in the danger colours. [unsent] is null when it can't be counted (the cache won't open).
 */
@Composable
fun UnpairDialog(unsent: Int?, onCancel: () -> Unit, onConfirm: () -> Unit, changed: Boolean = false) {
    Dialog(onDismissRequest = onCancel) {
        UnpairCard(unsent, onCancel, onConfirm, changed = changed)
    }
}

/** The dialog's content, on its own so the screenshot gallery can draw it. */
@Composable
fun UnpairCard(
    unsent: Int?,
    onCancel: () -> Unit,
    onConfirm: () -> Unit,
    focusCancel: Boolean = true,
    changed: Boolean = false,
) {
    val r = LocalDesign.current.room
    val cancel = remember { FocusRequester() }
    if (focusCancel) LaunchedEffect(Unit) { runCatching { cancel.requestFocus() } }
    val (body, confirm) = unpairWording(unsent)
    Column(
        Modifier
            .widthIn(max = Tokens.Metrics.Phone.INNER_LIST.dp)
            .background(r.raised, RoundedCornerShape(Tokens.Radius.LG.dp))
            .padding(S.S5.dp)
            .semantics { paneTitle = "Unpair this phone?" },
        verticalArrangement = Arrangement.spacedBy(S.S4.dp),
    ) {
        Label("Unpair this phone?", Tokens.Type.DETAIL_TITLE_PHONE, r.ink, maxLines = 2)
        Label(body, Tokens.Type.NOTES, r.ink2, maxLines = 8)
        if (changed) Label(CHANGED, Tokens.Type.META_PHONE, r.dangerInk, maxLines = 3)
        Button("Cancel", primary = true, modifier = Modifier.focusRequester(cancel), onClick = onCancel)
        Button(confirm, primary = false, danger = unsent != 0, onClick = onConfirm)
    }
}

/** The question's text and the confirm button's label, by how many changes would be lost. */
fun unpairWording(unsent: Int?): Pair<String, String> = when {
    unsent == null ->
        "This phone’s copy of your tasks can’t be opened, so it can’t tell whether some changes haven’t " +
            "reached the server. Unpairing deletes the copy, and any such changes are lost." to "Unpair and discard"
    unsent == 0 ->
        "This phone forgets the server and deletes its copy of your tasks. Everything on it has reached the " +
            "server, so nothing is lost." to "Unpair"
    unsent == 1 ->
        "1 change on this phone hasn’t reached the server. Unpairing deletes this phone’s copy of your " +
            "tasks, and that change is lost." to "Unpair and discard"
    else ->
        "$unsent changes on this phone haven’t reached the server. Unpairing deletes this phone’s copy of " +
            "your tasks, and those changes are lost." to "Unpair and discard"
}

/** Shown when the count changed between showing the question and confirming it. */
const val CHANGED = "The number changed while the question was open. Check it, then confirm again."
