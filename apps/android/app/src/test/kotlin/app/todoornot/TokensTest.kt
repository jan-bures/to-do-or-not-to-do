package app.todoornot

import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.toArgb
import app.todoornot.design.AgentColors
import app.todoornot.design.DuskColors
import app.todoornot.design.ListColors
import app.todoornot.design.RoomColors
import app.todoornot.design.Tokens
import org.json.JSONObject
import org.junit.Assert.assertEquals
import org.junit.Test
import java.io.File

/**
 * The generated Kotlin tokens match the checked-in tokens.json (the export of crates/todo-design), so a
 * token change can't reach the desktop and not the phone.
 */
class TokensTest {
    private val json = JSONObject(File(System.getProperty("todo.tokens")!!).readText())

    private fun hex(c: Color): String = "#%06x".format(c.toArgb() and 0xFFFFFF)

    private fun <T> check(theme: JSONObject, group: String, palettes: Map<String, T>, fields: Map<String, (T) -> Color>) {
        val expected = theme.getJSONObject(group)
        assertEquals("$group hues", expected.keyset(), palettes.keys)
        for (hue in expected.keyset()) {
            val e = expected.getJSONObject(hue)
            assertEquals("$group/$hue keys", e.keyset(), fields.keys)
            val p = palettes.getValue(hue)
            for ((k, get) in fields) assertEquals("$group/$hue/$k", e.getString(k), hex(get(p)))
        }
    }

    private val room: Map<String, (RoomColors) -> Color> = mapOf(
        "accent" to { it.accent }, "accent-fill" to { it.accentFill }, "accent-strong" to { it.accentStrong },
        "arc" to { it.arc }, "canvas" to { it.canvas }, "check-selected" to { it.checkSelected },
        "control" to { it.control }, "danger" to { it.danger }, "danger-bg" to { it.dangerBg },
        "danger-ink" to { it.dangerInk }, "hill-far" to { it.hillFar }, "hill-near" to { it.hillNear },
        "horizon" to { it.horizon }, "hover" to { it.hover }, "important-fill" to { it.importantFill },
        "ink" to { it.ink }, "ink-2" to { it.ink2 },
        "ink-3" to { it.ink3 }, "ink-4" to { it.ink4 }, "line" to { it.line }, "line-strong" to { it.lineStrong },
        "on-accent" to { it.onAccent }, "pane" to { it.pane }, "raised" to { it.raised },
        "selection-check" to { it.selectionCheck }, "selection-wash" to { it.selectionWash }, "sky" to { it.sky },
        "star" to { it.star }, "success" to { it.success }, "sun" to { it.sun }, "sun-ink" to { it.sunInk },
        "tint" to { it.tint }, "toast" to { it.toast }, "toast-accent" to { it.toastAccent },
        "toast-ink" to { it.toastInk }, "toast-ink-2" to { it.toastInk2 }, "wash" to { it.wash },
    )
    private val list: Map<String, (ListColors) -> Color> = mapOf(
        "dot" to { it.dot }, "glyph" to { it.glyph }, "ink" to { it.ink },
        "on-strong" to { it.onStrong }, "strong" to { it.strong }, "tint" to { it.tint },
    )
    private val agent: Map<String, (AgentColors) -> Color> = mapOf("bg" to { it.bg }, "ink" to { it.ink }, "ring" to { it.ring })
    private val dusk: Map<String, (DuskColors) -> Color> = mapOf(
        "low" to { it.low }, "silhouette-far" to { it.silhouetteFar }, "silhouette-near" to { it.silhouetteNear },
        "star" to { it.star }, "sun" to { it.sun }, "top" to { it.top },
    )

    @Test
    fun colours_match_the_export() {
        val colors = json.getJSONObject("colors")
        val light = colors.getJSONObject("light")
        val dark = colors.getJSONObject("dark")
        check(light, "rooms", Tokens.lightRooms, room)
        check(dark, "rooms", Tokens.darkRooms, room)
        check(light, "lists", Tokens.lightLists, list)
        check(dark, "lists", Tokens.darkLists, list)
        check(light, "agents", Tokens.lightAgents, agent)
        check(dark, "agents", Tokens.darkAgents, agent)
        check(colors, "dusk", Tokens.dusk, dusk)
    }

    @Test
    fun phone_metrics_type_and_motion_match_the_export() {
        val phone = json.getJSONObject("metrics").getJSONObject("phone")
        val p = Tokens.Metrics.Phone
        mapOf(
            "row" to p.ROW, "row-pad-left" to p.ROW_PAD_LEFT, "row-pad-right" to p.ROW_PAD_RIGHT,
            "row-pad-top" to p.ROW_PAD_TOP, "due-slot" to p.DUE_SLOT, "composer" to p.COMPOSER,
            "room-pill" to p.ROOM_PILL, "room-row" to p.ROOM_ROW, "inner-list" to p.INNER_LIST,
            "toast" to p.TOAST, "toast-strip" to p.TOAST_STRIP, "app-bar" to p.APP_BAR, "touch" to p.TOUCH,
        ).forEach { (k, v) -> assertEquals(k, phone.getDouble(k).toFloat(), v, 0f) }

        val type = json.getJSONObject("type")
        mapOf(
            "room-title-phone" to Tokens.Type.ROOM_TITLE_PHONE, "task-title-phone" to Tokens.Type.TASK_TITLE_PHONE,
            "detail-title-phone" to Tokens.Type.DETAIL_TITLE_PHONE, "due" to Tokens.Type.DUE,
            "composer" to Tokens.Type.COMPOSER, "room-pill" to Tokens.Type.ROOM_PILL,
        ).forEach { (k, t) ->
            val e = type.getJSONObject(k)
            assertEquals("$k size", e.getDouble("size").toFloat(), t.size, 0f)
            assertEquals("$k line height", e.getDouble("lineHeight").toFloat(), t.lineHeight, 0f)
            assertEquals("$k weight", e.getInt("weight"), t.weight)
        }

        val motion = json.getJSONObject("motion").getJSONObject("durationsMs")
        val m = Tokens.Motion
        mapOf(
            "check-fill" to m.CHECK_FILL, "check-tick" to m.CHECK_TICK, "complete-hold" to m.COMPLETE_HOLD,
            "row-collapse" to m.ROW_COLLAPSE, "sun-travel" to m.SUN_TRAVEL, "undo-window" to m.UNDO_WINDOW,
        ).forEach { (k, v) -> assertEquals(k, motion.getInt(k), v) }

        val cues = json.getJSONObject("motion").getJSONObject("cues")
        mapOf(
            "check-fill-from-scale" to m.CHECK_FILL_FROM_SCALE, "toggle-fill-from-scale" to m.TOGGLE_FILL_FROM_SCALE,
            "dusk-night-at" to m.DUSK_NIGHT_AT,
            "dusk-handover-at" to m.DUSK_HANDOVER_AT, "dusk-handover-rise" to m.DUSK_HANDOVER_RISE,
            "stagger-max-items" to m.STAGGER_MAX_ITEMS,
            "drag-touch-scale" to m.DRAG_TOUCH_SCALE,
        ).forEach { (k, v) -> assertEquals(k, cues.getDouble(k).toFloat(), v, 0f) }
        val easing = json.getJSONObject("motion").getJSONObject("easings").getJSONArray("sun")
        assertEquals(easing.getDouble(0).toFloat(), m.EASE_SUN.x1, 0f)
        assertEquals(easing.getDouble(1).toFloat(), m.EASE_SUN.y1, 0f)
    }
}

private fun JSONObject.keyset(): Set<String> = keys().asSequence().toSet()
