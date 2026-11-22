package app.todoornot.core

import java.time.Instant
import java.util.TimeZone

/** The current instant, its UTC offset, and the actual bounds of its local calendar day. */
fun localNow(epochMs: Long = System.currentTimeMillis(), zone: TimeZone = TimeZone.getDefault()): LocalNow {
    val zoneId = zone.toZoneId()
    val date = Instant.ofEpochMilli(epochMs).atZone(zoneId).toLocalDate()
    val start = date.atStartOfDay(zoneId).toInstant().toEpochMilli()
    val end = date.plusDays(1).atStartOfDay(zoneId).toInstant().toEpochMilli()
    return LocalNow(epochMs, zone.getOffset(epochMs) / 1000, LocalDay(start, end))
}
