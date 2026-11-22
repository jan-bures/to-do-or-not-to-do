package app.todoornot.core

import java.util.TimeZone

/** The wall clock as the Rust core wants it: an instant plus this zone's offset at that instant. */
fun localNow(epochMs: Long = System.currentTimeMillis(), zone: TimeZone = TimeZone.getDefault()): LocalNow =
    LocalNow(epochMs, zone.getOffset(epochMs) / 1000)
