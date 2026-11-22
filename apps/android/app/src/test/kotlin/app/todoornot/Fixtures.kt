package app.todoornot

import app.todoornot.core.LocalNow
import app.todoornot.core.RoomKey
import app.todoornot.core.TodoEngine
import java.time.Instant

/** Saturday 21 November 2026, 10:35 in a UTC+1 zone: the moment the fixtures and mockups are drawn at. */
val FIXTURE_NOW = LocalNow(Instant.parse("2026-11-21T09:35:00Z").toEpochMilli(), 3600)

/** An offline engine over the invented fixture data ("world", "finished" or "blank"). */
fun fixtureEngine(name: String = "world"): TodoEngine = TodoEngine.fixture(name, FIXTURE_NOW)

fun TodoEngine.collection(name: String): RoomKey =
    home(FIXTURE_NOW).groups.mapNotNull { it.collection }.first { it.name == name }.key

fun TodoEngine.list(name: String): RoomKey =
    home(FIXTURE_NOW).groups.flatMap { it.lists }.first { it.name == name }.key

fun TodoEngine.taskId(room: RoomKey, title: String): String =
    room(room, FIXTURE_NOW).tasks.first { it.title == title }.id
