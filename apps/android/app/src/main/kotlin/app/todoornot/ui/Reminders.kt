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
import app.todoornot.design.LocalDesign
import app.todoornot.design.Tokens
import app.todoornot.reminders.ReminderAccess
import app.todoornot.reminders.ReminderNeed
import java.time.Instant
import java.time.LocalDate
import java.time.LocalDateTime
import java.time.ZoneId
import java.time.temporal.ChronoUnit

private val S = Tokens.Space
private const val DAY_MS = 86_400_000L

/** Asks for what reminders need after the owner sets one ([ReminderAccess]); the gallery's asks nothing. */
val LocalReminderAccess = staticCompositionLocalOf<ReminderAccess> { ReminderAccess.None }

/**
 * Where the reminder picker opens: the reminder's own time; else, for a task due on a later day, 09:00
 * that day; else the next whole hour from [nowMs]. In [zone], the phone's.
 */
fun reminderStart(atMs: Long?, due: String?, nowMs: Long, zone: ZoneId): LocalDateTime {
    if (atMs != null) return LocalDateTime.ofInstant(Instant.ofEpochMilli(atMs), zone)
    val now = LocalDateTime.ofInstant(Instant.ofEpochMilli(nowMs), zone)
    val day = due?.let { runCatching { LocalDate.parse(it) }.getOrNull() }
    if (day != null && day.isAfter(now.toLocalDate())) return day.atTime(9, 0)
    return now.truncatedTo(ChronoUnit.HOURS).plusHours(1)
}

/** The instant of the picker's day (its UTC midnight, as the Material date picker counts) at [hour]:[minute] in [zone]. */
fun reminderMillis(dayUtcMs: Long, hour: Int, minute: Int, zone: ZoneId): Long =
    LocalDate.ofEpochDay(Math.floorDiv(dayUtcMs, DAY_MS)).atTime(hour, minute).atZone(zone).toInstant().toEpochMilli()

/** Under the picker when the time picked has passed. */
const val REMINDER_PAST = "Pick a time that hasn’t passed."

/**
 * The reminder picker: the Material date picker and time input, in our colours and type, on the overlay
 * surface, with the time in 24 hours like the rest of the app. It opens where [reminderStart] says.
 * **Save** keeps the day and time (only one still to come), **Remove** takes the reminder away, and
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
) {
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
    val picked = date.selectedDateMillis?.let { reminderMillis(it, time.hour, time.minute, zone) }
    val past = picked != null && picked <= nowMs
    val shape = RoundedCornerShape(Tokens.Radius.XL.dp)
    BoxWithConstraints(modifier) {
        val compact = maxHeight < COMPACT_BELOW
        Column(
            Modifier
                .widthIn(max = Tokens.Metrics.Phone.INNER_LIST.dp)
                .overlay(shape, d)
                .semantics { paneTitle = "Reminder" },
        ) {
            Column(Modifier.weight(1f, fill = false).verticalScroll(rememberScrollState())) {
                MaterialTheme(colorScheme = MaterialTheme.colorScheme, typography = tokenTypography()) {
                    val timeRow = @Composable {
                        Box(Modifier.fillMaxWidth().padding(horizontal = S.S6.dp, vertical = if (compact) S.S3.dp else 0.dp), contentAlignment = Alignment.Center) {
                            TimeInput(state = time, colors = timeColors)
                        }
                    }
                    // In a short window the time comes first, so it is in view without scrolling.
                    if (compact) timeRow()
                    DatePicker(
                        state = date,
                        colors = datePickerColors(r, markToday),
                        title = if (compact) null else {
                            { Label("Reminder", Tokens.Type.CAPS_LABEL, r.ink3, Modifier.padding(start = S.S6.dp, top = S.S4.dp)) }
                        },
                        headline = if (compact) null else {
                            { DatePickerDefaults.DatePickerHeadline(date.selectedDateMillis, date.displayMode, DatePickerDefaults.dateFormatter(), Modifier.padding(start = S.S6.dp, end = S.S3.dp, bottom = S.S3.dp)) }
                        },
                        showModeToggle = !compact,
                    )
                    if (!compact) timeRow()
                }
            }
            Column(
                (if (compact) Modifier.hairline(r.line, top = true) else Modifier)
                    .padding(start = S.S5.dp, end = S.S5.dp, bottom = S.S5.dp, top = if (compact) S.S3.dp else 0.dp),
                verticalArrangement = Arrangement.spacedBy(S.S3.dp),
            ) {
                if (past) Label(REMINDER_PAST, Tokens.Type.META_PHONE, r.dangerInk, maxLines = 2)
                Row(horizontalArrangement = Arrangement.spacedBy(S.S2.dp)) {
                    if (atMs != null) Button("Remove", primary = false, modifier = Modifier.weight(1f)) { onSave(null) }
                    Button("Cancel", primary = false, modifier = Modifier.weight(1f), onClick = onCancel)
                    Button("Save", primary = true, modifier = Modifier.weight(1f), enabled = picked != null && !past) {
                        picked?.let(onSave)
                    }
                }
            }
        }
    }
}

/** The reminder picker in its dialog. */
@Composable
fun ReminderDialog(atMs: Long?, due: String?, nowMs: Long, zone: ZoneId, onSave: (Long?) -> Unit, onDismiss: () -> Unit) {
    OverlayDialog(onDismiss) { overlay ->
        ReminderCard(atMs, due, nowMs, zone, onSave = { at -> overlay.choose { onSave(at) } }, onCancel = overlay::close)
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
