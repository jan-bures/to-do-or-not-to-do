package app.todoornot.core

import java.time.Instant
import java.time.ZoneId

/** Send the zone ID for historical dates and the device's actual day bounds for sidebar counts. */
fun localNow(epochMs: Long = System.currentTimeMillis(), zone: ZoneId = ZoneId.systemDefault()): LocalNow {
    val local = Instant.ofEpochMilli(epochMs).atZone(zone)
    val date = local.toLocalDate()
    val start = date.atStartOfDay(zone).toInstant().toEpochMilli()
    val end = date.plusDays(1).atStartOfDay(zone).toInstant().toEpochMilli()
    return LocalNow(epochMs, local.offset.totalSeconds, zone.id, LocalDay(start, end))
}
