package app.todoornot.ui

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.DatePicker
import androidx.compose.material3.DatePickerDefaults
import app.todoornot.core.localNow
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.TimeInput
import androidx.compose.material3.TimeInputDefaults
import androidx.compose.material3.OutlinedTextFieldDefaults
import androidx.compose.material3.rememberDatePickerState
import androidx.compose.material3.rememberTimePickerState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.remember
import androidx.compose.runtime.staticCompositionLocalOf
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.semantics.paneTitle
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.dp
import app.todoornot.core.reminderPastNote
import app.todoornot.design.LocalDesign
import app.todoornot.design.Tokens
import app.todoornot.reminders.ReminderAccess
import app.todoornot.reminders.ReminderNeed
import app.todoornot.reminders.clockChangeNote
import app.todoornot.reminders.reminderStart
import app.todoornot.reminders.resolveReminder
import androidx.compose.material3.isInputValid
import java.time.LocalDate
import java.time.ZoneId

private val S = Tokens.Space
private const val DAY_MS = 86_400_000L

/** Asks for what reminders need after the owner sets one ([ReminderAccess]); the gallery's asks nothing. */
val LocalReminderAccess = staticCompositionLocalOf<ReminderAccess> { ReminderAccess.None }

/** Under the picker while the time typed isn't one. */
const val REMINDER_BAD_TIME = "Type an hour from 00 to 23 and a minute from 00 to 59."

/**
 * The reminder picker: the Material date picker and time input, in our colours and type, on the overlay
 * surface, with the time in 24 hours like the rest of the app. It opens where [reminderStart] says.
 * **Save** keeps the day and time (only a whole time still to come). When the clocks change over it, a
 * note says which time is saved: the one a skipped time moves to, or the first of a doubled one
 * ([resolveReminder]). **Remove** takes the reminder away, and
 * **Cancel** changes nothing. [markToday] rings today's date; the gallery turns it off, as the picker
 * reads the device's clock for it. In a short window the date picker drops its headline and mode toggle,
 * the time comes first, and everything scrolls under the buttons, which stay in view.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun ReminderCard(
    atMs: Long?,
    due: String?,
    nowMs: Long,
    zone: ZoneId,
    onSave: (Long?) -> Unit,
    onCancel: () -> Unit,
    modifier: Modifier = Modifier,
    markToday: Boolean = true,
    inSheet: Boolean = false,
    dateFormat: String? = null,
) {
    val formatter = remember(nowMs, zone, dateFormat) { datePickerFormatter(localNow(nowMs, zone, dateFormat)) }
    val d = LocalDesign.current
    val r = d.room
    val start = remember(atMs, due, zone) { reminderStart(atMs, due, nowMs, zone) }
    val startDay = start.toLocalDate().toEpochDay() * DAY_MS
    val date = rememberDatePickerState(initialSelectedDateMillis = startDay, initialDisplayedMonthMillis = startDay)
    val time = rememberTimePickerState(initialHour = start.hour, initialMinute = start.minute, is24Hour = true)
    val timeColors = TimeInputDefaults.colors(
        containerColor = r.raised,
        periodSelectorBorderColor = r.lineStrong,
        periodSelectorSelectedContainerColor = r.tint,
        periodSelectorContainerColor = r.raised,
        periodSelectorSelectedContentColor = r.ink,
        periodSelectorContentColor = r.ink2,
        timeTextFieldColors = OutlinedTextFieldDefaults.colors(
            focusedTextColor = r.accentStrong,
            unfocusedTextColor = r.ink,
            focusedContainerColor = r.tint,
            unfocusedContainerColor = r.wash,
            cursorColor = r.accent,
            focusedBorderColor = r.accent,
            unfocusedBorderColor = Color.Transparent,
            focusedSupportingTextColor = r.ink3,
            unfocusedSupportingTextColor = r.ink3,
        ),
    )
    // Only a time the fields hold whole counts: while one is half typed, the state keeps the last good one.
    val valid = time.isInputValid
    val picked = date.selectedDateMillis?.takeIf { valid }
        ?.let { resolveReminder(LocalDate.ofEpochDay(Math.floorDiv(it, DAY_MS)), time.hour, time.minute, zone) }
    val past = picked != null && picked.atMs <= nowMs
    val note = picked?.let(::clockChangeNote)
    val shape = RoundedCornerShape(Tokens.Radius.XL.dp)
    BoxWithConstraints(modifier) {
        val compact = maxHeight < COMPACT_BELOW
        Column(
            Modifier
                .then(if (inSheet) Modifier.fillMaxWidth() else Modifier.widthIn(max = Tokens.Metrics.Phone.INNER_LIST.dp).overlay(shape, d))
                .semantics { paneTitle = "Reminder" },
        ) {
            val timeRow = @Composable {
                MaterialTheme(colorScheme = MaterialTheme.colorScheme, typography = tokenTypography()) {
                    Box(Modifier.fillMaxWidth().padding(horizontal = S.S6.dp, vertical = S.S2.dp), contentAlignment = Alignment.Center) {
                        TimeInput(state = time, colors = timeColors)
                    }
                }
            }
            Column(Modifier.weight(1f, fill = false).verticalScroll(rememberScrollState())) {
                MaterialTheme(colorScheme = MaterialTheme.colorScheme, typography = tokenTypography()) {
                    // In a short window the time comes first, so it is in view without scrolling.
                    if (compact) timeRow()
                    DatePicker(
                        state = date,
                        dateFormatter = formatter,
                        colors = datePickerColors(r, markToday),
                        title = if (compact) null else {
                            { Label("Reminder", Tokens.Type.CAPS_LABEL, r.ink3, Modifier.padding(start = S.S6.dp, top = S.S4.dp)) }
                        },
                        headline = if (compact) null else {
                            { DatePickerDefaults.DatePickerHeadline(date.selectedDateMillis, date.displayMode, formatter, Modifier.padding(start = S.S6.dp, end = S.S3.dp, bottom = S.S3.dp)) }
                        },
                        showModeToggle = !compact,
                    )
                }
            }
            if (!compact) timeRow()
            Column(
                (if (compact) Modifier.hairline(r.line, top = true) else Modifier)
                    .padding(start = S.S5.dp, end = S.S5.dp, bottom = S.S5.dp, top = if (compact) S.S3.dp else 0.dp),
                verticalArrangement = Arrangement.spacedBy(S.S3.dp),
            ) {
                if (!valid) Label(REMINDER_BAD_TIME, Tokens.Type.META_PHONE, r.dangerInk, maxLines = 2)
                if (note != null) Label(note, Tokens.Type.META_PHONE, r.ink2, maxLines = 3)
                if (past) Label(remember { reminderPastNote() }, Tokens.Type.META_PHONE, r.dangerInk, maxLines = 2)
                Row(horizontalArrangement = Arrangement.spacedBy(S.S2.dp)) {
                    if (atMs != null) Button("Remove", primary = false, modifier = Modifier.weight(1f)) { onSave(null) }
                    Button("Cancel", primary = false, modifier = Modifier.weight(1f), onClick = onCancel)
                    Button("Save", primary = true, modifier = Modifier.weight(1f), enabled = picked != null && !past) {
                        picked?.let { onSave(it.atMs) }
                    }
                }
            }
        }
    }
}

/** What the rationale says before Android's question: its title, its text and the button that goes on. */
fun reminderAccessWording(need: ReminderNeed): Triple<String, String, String> = when (need) {
    ReminderNeed.NOTIFICATIONS -> Triple(
        "Show reminders?",
        "When a reminder is due, this phone shows the task in a notification, with Complete and Snooze 10 min. Android asks next whether the app may send notifications.",
        "Continue",
    )
    ReminderNeed.EXACT_ALARMS -> Triple(
        "Remind you on time?",
        "Android may deliver reminders a few minutes late unless the app may set alarms. Turn on Alarms & reminders for it in the settings that open next.",
        "Open settings",
    )
}

/**
 * Says why reminders need [need] before Android asks: after the owner sets their first reminder, never
 * at launch. **Not now** is first; Back or a tap outside counts as it, and it isn't asked again.
 */
@Composable
fun ReminderAccessCard(need: ReminderNeed, onAllow: () -> Unit, onNotNow: () -> Unit) {
    val r = LocalDesign.current.room
    val (title, body, allow) = reminderAccessWording(need)
    DialogCard(title) {
        Label(title, Tokens.Type.DETAIL_TITLE_PHONE, r.ink, maxLines = 2)
        Label(body, Tokens.Type.NOTES, r.ink2, maxLines = 8)
        Row(horizontalArrangement = Arrangement.spacedBy(S.S2.dp)) {
            Button("Not now", primary = false, modifier = Modifier.weight(1f), onClick = onNotNow)
            Button(allow, primary = true, modifier = Modifier.weight(1f), onClick = onAllow)
        }
    }
}

/** The rationale in its dialog. Dismissing it is a "Not now". */
@Composable
fun ReminderAccessDialog(need: ReminderNeed, access: ReminderAccess, onDone: () -> Unit) {
    val answered = remember { booleanArrayOf(false) }
    OverlayDialog({
        if (!answered[0]) access.decline(need)
        onDone()
    }) { overlay ->
        ReminderAccessCard(
            need,
            onAllow = { overlay.choose { answered[0] = true; access.allow(need) } },
            onNotNow = { overlay.choose { answered[0] = true; access.decline(need) } },
        )
    }
}
