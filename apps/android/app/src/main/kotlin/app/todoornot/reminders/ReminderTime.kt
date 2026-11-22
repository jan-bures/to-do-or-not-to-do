package app.todoornot.reminders

import java.time.DayOfWeek
import java.time.Instant
import java.time.LocalDate
import java.time.LocalDateTime
import java.time.ZoneId
import java.time.temporal.ChronoUnit
import java.util.Locale

/**
 * The picker's day and time, as an instant in the phone's zone. [saved] is the wall-clock time it stands
 * for: the same as asked, unless the clocks went forward over it ([skipped]: the time didn't exist that day,
 * so the one the gap moves it to is saved). When the clocks went back over it, the time happens twice
 * ([twice]), and the first is saved.
 */
data class ReminderTime(val atMs: Long, val asked: LocalDateTime, val saved: LocalDateTime, val skipped: Boolean, val twice: Boolean)

/** Resolves the picker's [day] at [hour]:[minute] in [zone], saying how the clocks changed it, if they did. */
fun resolveReminder(day: LocalDate, hour: Int, minute: Int, zone: ZoneId): ReminderTime {
    val asked = day.atTime(hour, minute)
    val offsets = zone.rules.getValidOffsets(asked)
    // Java resolves a gap by moving the time on by the gap's length, and an overlap to the earlier offset,
    // which is the earlier of the two instants.
    val z = asked.atZone(zone)
    return ReminderTime(z.toInstant().toEpochMilli(), asked, z.toLocalDateTime(), skipped = offsets.isEmpty(), twice = offsets.size > 1)
}

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

private val WEEKDAYS = mapOf(
    DayOfWeek.MONDAY to "Mon", DayOfWeek.TUESDAY to "Tue", DayOfWeek.WEDNESDAY to "Wed", DayOfWeek.THURSDAY to "Thu",
    DayOfWeek.FRIDAY to "Fri", DayOfWeek.SATURDAY to "Sat", DayOfWeek.SUNDAY to "Sun",
)
private val MONTHS = listOf("Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec")

/** "Today", "Tomorrow", "Yesterday" or "Sun 29 Mar" ("Mon 4 Jan 2027" in another year), as the core words it. */
fun dayLabel(day: LocalDate, today: LocalDate): String = when (ChronoUnit.DAYS.between(today, day)) {
    0L -> "Today"
    1L -> "Tomorrow"
    -1L -> "Yesterday"
    else -> {
        val base = "${WEEKDAYS.getValue(day.dayOfWeek)} ${day.dayOfMonth} ${MONTHS[day.monthValue - 1]}"
        if (day.year == today.year) base else "$base ${day.year}"
    }
}

/** "15:30". */
fun clockLabel(t: LocalDateTime): String = String.format(Locale.ROOT, "%02d:%02d", t.hour, t.minute)

/**
 * When a reminder is, in words, in the zone that applies on its own day (so across a change of the clocks
 * it reads as set): "Today, 15:30", "Sun 29 Mar, 03:30".
 */
fun reminderLabel(atMs: Long, nowMs: Long, zone: ZoneId): String {
    val at = LocalDateTime.ofInstant(Instant.ofEpochMilli(atMs), zone)
    val today = LocalDateTime.ofInstant(Instant.ofEpochMilli(nowMs), zone).toLocalDate()
    return "${dayLabel(at.toLocalDate(), today)}, ${clockLabel(at)}"
}

/**
 * What the picker says under the time when the clocks change over it: which time is saved instead, or
 * that the first of two is. Null when nothing needs saying.
 */
fun clockChangeNote(t: ReminderTime): String? = when {
    t.skipped -> "${clockLabel(t.asked)} doesn’t exist that day: the clocks go forward. Saves ${clockLabel(t.saved)}."
    t.twice -> "${clockLabel(t.asked)} happens twice that day: the clocks go back. Saves the first."
    else -> null
}
