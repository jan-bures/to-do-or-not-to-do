package app.todoornot.widget

import app.todoornot.core.Chip
import app.todoornot.core.DueLabel
import app.todoornot.core.Home
import app.todoornot.core.LocalNow
import app.todoornot.core.NavItem
import app.todoornot.core.RoomKey
import app.todoornot.core.SmartList
import app.todoornot.core.TodoEngine
import app.todoornot.session.EngineHost
import app.todoornot.session.Host

internal val defaultWidgetRoom = RoomKey.Smart(SmartList.MY_DAY)

private val widgetSmartLists = setOf(SmartList.MY_DAY, SmartList.IMPORTANT, SmartList.PLANNED, SmartList.ALL)
// The launcher receives the whole collection, even though it lays out only the visible rows.
internal const val WIDGET_ROW_LIMIT = 100

/** Stable keys for preferences and deep links. Never serialize a title, session, or server address. */
internal fun RoomKey.widgetKey(): String = when (this) {
    is RoomKey.Smart -> "smart:${list.name}"
    is RoomKey.List -> "list:$id"
    is RoomKey.Collection -> "collection:$id"
    RoomKey.Trash -> defaultWidgetRoom.widgetKey()
}

internal fun widgetRoom(key: String?): RoomKey? {
    val value = key?.substringAfter(':', "")?.takeIf { it.isNotEmpty() } ?: return null
    return when (key.substringBefore(':')) {
        "smart" -> widgetSmartLists.firstOrNull { it.name == value }?.let { RoomKey.Smart(it) }
        "list" -> RoomKey.List(value)
        "collection" -> RoomKey.Collection(value)
        else -> null
    }
}

internal data class WidgetChoice(val item: NavItem, val indented: Boolean = false)

/** A projection of the drawer: the same items and order, except rooms without open-task widgets. */
internal fun widgetChoices(home: Home): List<WidgetChoice> = buildList {
    home.smart.filter { (it.key as? RoomKey.Smart)?.list in widgetSmartLists }.forEach { add(WidgetChoice(it)) }
    home.groups.forEach { group ->
        group.collection?.let { add(WidgetChoice(it)) }
        group.lists.forEach { add(WidgetChoice(it, indented = group.collection != null)) }
    }
}

internal data class WidgetRow(val id: String, val title: String, val list: Chip?, val due: DueLabel?, val important: Boolean)

internal sealed interface WidgetData {
    data object SignedOut : WidgetData
    data object Unavailable : WidgetData
    data object Unconfigured : WidgetData
    data class Missing(val collection: Boolean) : WidgetData
    data class Room(val item: NavItem, val rows: List<WidgetRow>, val empty: String) : WidgetData
}

internal data class WidgetSnapshot(val data: WidgetData, val pairingId: String? = null)

/** Keep the rows and the identity used by their actions from the same pairing. */
internal fun widgetSnapshot(host: EngineHost, choice: RoomKey?, now: LocalNow): WidgetSnapshot {
    val signedOut = WidgetSnapshot(WidgetData.SignedOut)
    return when (val ready = host.state.value) {
        Host.SignedOut -> signedOut
        is Host.CantOpen -> WidgetSnapshot(WidgetData.Unavailable)
        is Host.Ready -> try {
            if (!host.isAuthorized(ready)) return signedOut
            val data = widgetData(ready.engine, choice, now)
            if (host.isAuthorized(ready)) WidgetSnapshot(data, ready.session.pairingId) else signedOut
        } catch (_: IllegalStateException) {
            signedOut // Engine closed during unpair.
        }
    }
}

internal fun widgetData(engine: TodoEngine, choice: RoomKey?, now: LocalNow): WidgetData {
    if (choice == null) return WidgetData.Unconfigured
    val missing = WidgetData.Missing(choice is RoomKey.Collection)
    val item = widgetChoices(engine.home(now)).firstOrNull { it.item.key == choice }?.item ?: return missing
    val room = engine.room(choice, now)
    // A deletion between reads can make the engine fall back to My Day. Don't show the wrong room.
    if (room.key != choice) return missing
    val rows = room.tasks.asSequence().filterNot { it.done }.take(WIDGET_ROW_LIMIT).map {
        WidgetRow(it.id, it.title, it.chip, it.due, it.priority == 3.toUByte())
    }.toList()
    return WidgetData.Room(item, rows, room.emptyLine)
}

/** Explicit desired states make repeated delivery of the same click harmless. */
internal sealed interface WidgetEdit {
    val task: String
    data class Complete(override val task: String) : WidgetEdit
    data class Important(override val task: String, val on: Boolean) : WidgetEdit

    fun apply(engine: TodoEngine, now: LocalNow) {
        if (engine.task(task, now)?.done != false) return
        when (this) {
            is Complete -> engine.setDone(task, true)
            is Important -> engine.setImportant(task, on)
        }
    }
}
