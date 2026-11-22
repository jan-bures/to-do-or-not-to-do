package app.todoornot.ui

import androidx.compose.foundation.border
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material3.DatePicker
import androidx.compose.material3.DatePickerDefaults
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Typography
import androidx.compose.material3.rememberDatePickerState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.paneTitle
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.TextRange
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.KeyboardCapitalization
import androidx.compose.ui.text.input.TextFieldValue
import androidx.compose.ui.unit.dp
import app.todoornot.design.LocalDesign
import app.todoornot.design.Tokens
import app.todoornot.design.style
import java.time.LocalDate

private val S = Tokens.Space
private const val DAY_MS = 86_400_000L

/** The UTC midnight of an ISO date, as the Material date picker counts days. */
fun isoToUtcMillis(iso: String): Long = LocalDate.parse(iso).toEpochDay() * DAY_MS

/** The ISO date of the Material date picker's UTC midnight. */
fun utcMillisToIso(ms: Long): String = LocalDate.ofEpochDay(Math.floorDiv(ms, DAY_MS)).toString()

/**
 * The type the Material pieces use, from our tokens: day numbers and weekdays in the UI face, the headline
 * in the detail title's style, the month and the buttons as our buttons.
 */
private fun tokenTypography(): Typography {
    val body = Tokens.Type.BODY.style()
    val label = Tokens.Type.BUTTON_SMALL.style()
    val headline = Tokens.Type.DETAIL_TITLE_PHONE.style()
    val title = Tokens.Type.BUTTON.style()
    return Typography(
        displayLarge = headline, displayMedium = headline, displaySmall = headline,
        headlineLarge = headline, headlineMedium = headline, headlineSmall = headline,
        titleLarge = title, titleMedium = title, titleSmall = title,
        bodyLarge = body, bodyMedium = body, bodySmall = Tokens.Type.META_PHONE.style(),
        labelLarge = label, labelMedium = Tokens.Type.CAPS_LABEL.style(), labelSmall = Tokens.Type.COUNT.style(),
    )
}

/**
 * The due date picker: the Material date picker in our colours and type, on the overlay surface. It opens
 * on the task's date (or today's month), takes a tap on a day (or a typed date), and saves with **Save**.
 * **Clear** takes the date away, and with it a repeat, which the note says. [markToday] rings today's
 * date; the gallery turns it off, as the picker reads the device's clock for it.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun DueDateCard(
    due: String?,
    today: String,
    repeats: Boolean,
    onSave: (String?) -> Unit,
    onCancel: () -> Unit,
    modifier: Modifier = Modifier,
    markToday: Boolean = true,
) {
    val d = LocalDesign.current
    val r = d.room
    val opening = remember(due, today) { isoToUtcMillis(due ?: today.ifEmpty { LocalDate.now().toString() }) }
    val state = rememberDatePickerState(initialSelectedDateMillis = due?.let(::isoToUtcMillis), initialDisplayedMonthMillis = opening)
    val colors = DatePickerDefaults.colors(
        containerColor = r.raised,
        titleContentColor = r.ink3,
        headlineContentColor = r.ink,
        weekdayContentColor = r.ink3,
        subheadContentColor = r.ink2,
        navigationContentColor = r.ink2,
        yearContentColor = r.ink,
        disabledYearContentColor = r.ink4,
        currentYearContentColor = r.accentStrong,
        selectedYearContentColor = r.onAccent,
        selectedYearContainerColor = r.accentFill,
        dayContentColor = r.ink,
        disabledDayContentColor = r.ink4,
        selectedDayContentColor = r.onAccent,
        selectedDayContainerColor = r.accentFill,
        todayContentColor = if (markToday) r.accentStrong else r.ink,
        todayDateBorderColor = if (markToday) r.accent else Color.Transparent,
        dayInSelectionRangeContentColor = r.ink,
        dayInSelectionRangeContainerColor = r.tint,
        dividerColor = r.line,
    )
    val shape = RoundedCornerShape(Tokens.Radius.XL.dp)
    Column(
        modifier
            .widthIn(max = Tokens.Metrics.Phone.INNER_LIST.dp)
            .overlay(shape, d)
            .semantics { paneTitle = "Due date" },
    ) {
        MaterialTheme(colorScheme = MaterialTheme.colorScheme, typography = tokenTypography()) {
            DatePicker(
                state = state,
                colors = colors,
                title = { Label("Due", Tokens.Type.CAPS_LABEL, r.ink3, Modifier.padding(start = S.S6.dp, top = S.S4.dp)) },
                showModeToggle = true,
            )
        }
        Column(Modifier.padding(start = S.S5.dp, end = S.S5.dp, bottom = S.S5.dp), verticalArrangement = Arrangement.spacedBy(S.S3.dp)) {
            if (repeats && due != null) Label(REPEAT_NOTE, Tokens.Type.META_PHONE, r.ink3, maxLines = 2)
            Row(horizontalArrangement = Arrangement.spacedBy(S.S2.dp)) {
                if (due != null) Button("Clear", primary = false, modifier = Modifier.weight(1f)) { onSave(null) }
                Button("Cancel", primary = false, modifier = Modifier.weight(1f), onClick = onCancel)
                val picked = state.selectedDateMillis
                Button("Save", primary = true, modifier = Modifier.weight(1f), enabled = picked != null) {
                    picked?.let { onSave(utcMillisToIso(it)) }
                }
            }
        }
    }
}

/** Under the picker when the task repeats: what Clear also does. */
const val REPEAT_NOTE = "This task repeats. Clearing the date also stops the repeat."

/** The due date picker in its dialog. */
@Composable
fun DueDateDialog(due: String?, today: String, repeats: Boolean, onSave: (String?) -> Unit, onDismiss: () -> Unit) {
    OverlayDialog(onDismiss) { close ->
        DueDateCard(due, today, repeats, onSave = { onSave(it); close() }, onCancel = close)
    }
}

/**
 * Renames a list or a collection: the name, selected, with the keyboard up. Done or **Save** keeps it;
 * a blank name can't be saved.
 */
@Composable
fun RenameCard(title: String, name: String, onSave: (String) -> Unit, onCancel: () -> Unit, focus: Boolean = true) {
    val r = LocalDesign.current.room
    var text by rememberSaveable(name, stateSaver = TextFieldValue.Saver) {
        mutableStateOf(TextFieldValue(name, TextRange(0, name.length)))
    }
    val field = remember { FocusRequester() }
    if (focus) LaunchedEffect(Unit) { runCatching { field.requestFocus() } }
    val ok = text.text.isNotBlank()
    fun save() {
        if (ok) onSave(text.text.trim())
    }
    DialogCard(title) {
        Label(title, Tokens.Type.DETAIL_TITLE_PHONE, r.ink)
        BasicTextField(
            value = text,
            onValueChange = { text = it.copy(text = it.text.replace("\n", "")) },
            modifier = Modifier
                .fillMaxWidth()
                .heightIn(min = Tokens.Metrics.Phone.TOUCH.dp)
                .border(Tokens.Elevation.HAIRLINE.dp, r.accent, RoundedCornerShape(Tokens.Radius.MD.dp))
                .padding(horizontal = S.S3.dp, vertical = S.S3.dp)
                .focusRequester(field)
                .semantics { contentDescription = "Name" },
            textStyle = Tokens.Type.NAV.style(r.ink),
            singleLine = true,
            cursorBrush = SolidColor(r.accent),
            keyboardOptions = KeyboardOptions(capitalization = KeyboardCapitalization.Sentences, imeAction = ImeAction.Done),
            keyboardActions = KeyboardActions(onDone = { save() }),
        )
        Row(horizontalArrangement = Arrangement.spacedBy(S.S2.dp)) {
            Button("Cancel", primary = false, modifier = Modifier.weight(1f), onClick = onCancel)
            Button("Save", primary = true, modifier = Modifier.weight(1f), enabled = ok) { save() }
        }
    }
}

/**
 * Asks before deleting a list, saying in full which list and how many tasks go with it, and that Undo
 * brings them back. Cancel is the default: first, filled and focused; Back or a tap outside cancels too.
 */
@Composable
fun DeleteListCard(question: String, onDelete: () -> Unit, onCancel: () -> Unit, focusCancel: Boolean = true) {
    val r = LocalDesign.current.room
    val cancel = remember { FocusRequester() }
    if (focusCancel) LaunchedEffect(Unit) { runCatching { cancel.requestFocus() } }
    DialogCard("Delete list?") {
        Label("Delete list?", Tokens.Type.DETAIL_TITLE_PHONE, r.ink)
        Label(question, Tokens.Type.NOTES, r.ink2, maxLines = 8)
        Button("Cancel", primary = true, modifier = Modifier.focusRequester(cancel), onClick = onCancel)
        Button("Delete list", primary = false, danger = true, onClick = onDelete)
    }
}

