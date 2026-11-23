import org.gradle.api.DefaultTask
import org.gradle.api.file.DirectoryProperty
import org.gradle.api.file.RegularFileProperty
import org.gradle.api.tasks.InputDirectory
import org.gradle.api.tasks.InputFile
import org.gradle.api.tasks.OutputDirectory
import org.gradle.api.tasks.PathSensitive
import org.gradle.api.tasks.PathSensitivity
import org.gradle.api.tasks.TaskAction
import java.io.File

/** `GeistMono-400.ttf` → `geist_mono_400` (an Android resource name). */
fun fontResName(file: String): String =
    file.substringAfterLast('/').substringBeforeLast('.')
        .replace(Regex("([a-z])([A-Z])"), "$1_$2")
        .replace('-', '_')
        .lowercase()

private fun camel(key: String): String {
    val parts = key.split('-', '_')
    return parts.first() + parts.drop(1).joinToString("") { it.replaceFirstChar(Char::uppercase) }
}

private fun constName(key: String): String =
    key.replace(Regex("([a-z])([A-Z])"), "$1_$2").replace('-', '_').uppercase().let { if (it.first().isDigit()) "S$it" else it }

private fun pascal(key: String): String = camel(key).replaceFirstChar(Char::uppercase)

private fun f(v: Any?): String {
    val d = (v as Number).toDouble()
    return "${d}f"
}

private fun kstr(s: String): String = "\"" + s.replace("\\", "\\\\").replace("\"", "\\\"") + "\""

private fun color(hex: Any?): String {
    val h = (hex as String).removePrefix("#")
    require(h.length == 6) { "not #rrggbb: $hex" }
    return "Color(0xFF${h.uppercase()})"
}

@Suppress("UNCHECKED_CAST")
private fun Any?.obj(): Map<String, Any?> = this as Map<String, Any?>

/**
 * Generates `Tokens.kt` from `crates/todo-design/generated/tokens.json`: every colour of every named hue
 * in both themes, the type scale, spacing, radii, metrics, motion and the landscape constants. The app
 * never hard-codes a design value; it reads them from here.
 */
abstract class GenerateDesignTokens : DefaultTask() {
    @get:InputFile
    @get:PathSensitive(PathSensitivity.NONE)
    abstract val tokens: RegularFileProperty

    @get:OutputDirectory
    abstract val outputDir: DirectoryProperty

    @TaskAction
    fun generate() {
        val root = Json.parse(tokens.get().asFile.readText()).obj()
        val out = StringBuilder()
        out.appendLine("// Generated from crates/todo-design/generated/tokens.json by GenerateDesignTokens (buildSrc).")
        out.appendLine("// Do not edit: change the tokens in crates/todo-design and rebuild.")
        out.appendLine("@file:Suppress(\"MagicNumber\", \"LargeClass\")")
        out.appendLine()
        out.appendLine("package app.todoornot.design")
        out.appendLine()
        out.appendLine("import androidx.compose.ui.graphics.Color")
        out.appendLine("import app.todoornot.R")
        out.appendLine()

        val colors = root["colors"].obj()
        val light = colors["light"].obj()
        val dark = colors["dark"].obj()
        fun dataClass(name: String, sample: Map<String, Any?>) {
            out.appendLine("data class $name(")
            sample.keys.forEach { out.appendLine("    val ${camel(it)}: Color,") }
            out.appendLine(")")
            out.appendLine()
        }
        val roomSample = light["rooms"].obj().values.first().obj()
        dataClass("RoomColors", roomSample)
        dataClass("ListColors", light["lists"].obj().values.first().obj())
        dataClass("AgentColors", light["agents"].obj().values.first().obj())
        dataClass("DuskColors", colors["dusk"].obj().values.first().obj())

        out.appendLine("enum class FontRole { UI, MONO }")
        out.appendLine()
        out.appendLine("data class TextToken(")
        out.appendLine("    val role: FontRole,")
        out.appendLine("    val weight: Int,")
        out.appendLine("    val size: Float,")
        out.appendLine("    val lineHeight: Float,")
        out.appendLine("    val trackingEm: Float,")
        out.appendLine("    val italic: Boolean,")
        out.appendLine("    val uppercase: Boolean,")
        out.appendLine("    val tabular: Boolean,")
        out.appendLine(")")
        out.appendLine()
        out.appendLine("data class FontFaceToken(val role: FontRole, val res: Int, val weight: Int, val italic: Boolean)")
        out.appendLine()
        out.appendLine("data class Easing(val x1: Float, val y1: Float, val x2: Float, val y2: Float)")
        out.appendLine()
        out.appendLine("/** One layer of the overlay shadow, in dp: offset down, blur, spread, and its colour with alpha. */")
        out.appendLine("data class ShadowLayer(val y: Float, val blur: Float, val spread: Float, val color: Color)")
        out.appendLine()

        fun role(family: String) = when (family) {
            "Geist" -> "FontRole.UI"
            "Geist Mono" -> "FontRole.MONO"
            else -> error("unknown family $family")
        }

        out.appendLine("object Tokens {")
        val swatches = root["hues"].obj()["swatches"].obj().entries.sortedBy { (it.value as Number).toDouble() }
        out.appendLine("    val swatches = listOf(${swatches.joinToString { "\"${it.key}\"" }})")
        fun numbers(name: String, m: Map<String, Any?>, indent: String = "    ") {
            out.appendLine("${indent}object $name {")
            m.forEach { (k, v) -> if (v is Number) out.appendLine("$indent    const val ${constName(k)} = ${f(v)}") }
            out.appendLine("$indent}")
        }
        numbers("Space", root["space"].obj())
        numbers("Radius", root["radius"].obj())
        out.appendLine("    object Metrics {")
        root["metrics"].obj().forEach { (group, m) -> numbers(pascal(group), m.obj(), "        ") }
        out.appendLine("    }")
        numbers("Landscape", root["landscape"].obj())
        val elevation = root["elevation"].obj()
        numbers("Elevation", elevation)
        // The one shadow (menus, popovers, the date picker) and the modal scrim, per theme.
        fun withAlpha(c: Map<String, Any?>): String {
            val h = (c["color"] as String).removePrefix("#")
            require(h.length == 6) { "not #rrggbb: ${c["color"]}" }
            val a = Math.round((c["alpha"] as Number).toDouble() * 255).toInt().coerceIn(0, 255)
            return "Color(0x%02X%s)".format(a, h.uppercase())
        }
        val shadow = elevation["overlayShadow"].obj()
        for (theme in listOf("light", "dark")) {
            out.appendLine("    val overlayShadow${pascal(theme)}: List<ShadowLayer> = listOf(")
            (shadow[theme] as List<*>).forEach {
                val l = it.obj()
                out.appendLine("        ShadowLayer(${f(l["y"])}, ${f(l["blur"])}, ${f(l["spread"])}, ${withAlpha(l["color"].obj())}),")
            }
            out.appendLine("    )")
        }
        val scrim = elevation["scrim"].obj()
        for (theme in listOf("light", "dark")) {
            out.appendLine("    val scrim${pascal(theme)}: Map<String, Color> = mapOf(")
            scrim[theme].obj().forEach { (k, v) -> out.appendLine("        ${kstr(k)} to ${withAlpha(v.obj())},") }
            out.appendLine("    )")
        }

        val motion = root["motion"].obj()
        out.appendLine("    object Motion {")
        motion["durationsMs"].obj().forEach { (k, v) ->
            out.appendLine("        const val ${constName(k)} = ${(v as Number).toInt()}")
        }
        motion["easings"].obj().forEach { (k, v) ->
            val e = (v as List<*>).map { f(it) }
            out.appendLine("        val EASE_${constName(k)} = Easing(${e.joinToString()})")
        }
        // Cues that aren't durations: a scale, shares of DUSK, a distance in dp and an item count.
        motion["cues"].obj().forEach { (k, v) ->
            out.appendLine("        const val ${constName(k)} = ${f(v)}")
        }
        out.appendLine("    }")

        out.appendLine("    object Type {")
        root["type"].obj().forEach { (k, v) ->
            val t = v.obj()
            out.appendLine(
                "        val ${constName(k)} = TextToken(${role(t["family"] as String)}, ${(t["weight"] as Number).toInt()}, " +
                    "${f(t["size"])}, ${f(t["lineHeight"])}, ${f(t["trackingEm"])}, ${t["italic"]}, ${t["uppercase"]}, ${t["tabular"]})",
            )
        }
        out.appendLine("    }")

        out.appendLine("    val fontFaces: List<FontFaceToken> = listOf(")
        (root["fonts"].obj()["faces"] as List<*>).forEach {
            val face = it.obj()
            out.appendLine(
                "        FontFaceToken(${role(face["family"] as String)}, R.font.${fontResName(face["file"] as String)}, " +
                    "${(face["weight"] as Number).toInt()}, ${face["italic"]}),",
            )
        }
        out.appendLine("    )")

        val hues = root["hues"].obj()
        out.appendLine("    object Hues {")
        hues.forEach { (group, m) ->
            out.appendLine("        val ${camel(group)}: Map<String, Float> = mapOf(")
            m.obj().forEach { (k, v) -> out.appendLine("            ${kstr(k)} to ${f(v)},") }
            out.appendLine("        )")
        }
        out.appendLine("    }")

        // One function per palette keeps every method well under the JVM's size limit.
        fun palettes(prop: String, cls: String, theme: Map<String, Any?>, group: String) {
            val entries = theme[group].obj()
            out.appendLine("    val $prop: Map<String, $cls> by lazy {")
            out.appendLine("        mapOf(")
            entries.keys.forEach { out.appendLine("            ${kstr(it)} to ${prop}_${camel(it)}(),") }
            out.appendLine("        )")
            out.appendLine("    }")
            entries.forEach { (k, v) ->
                out.appendLine("    private fun ${prop}_${camel(k)}() = $cls(")
                v.obj().forEach { (ck, cv) -> out.appendLine("        ${camel(ck)} = ${color(cv)},") }
                out.appendLine("    )")
            }
        }
        palettes("lightRooms", "RoomColors", light, "rooms")
        palettes("darkRooms", "RoomColors", dark, "rooms")
        palettes("lightLists", "ListColors", light, "lists")
        palettes("darkLists", "ListColors", dark, "lists")
        palettes("lightAgents", "AgentColors", light, "agents")
        palettes("darkAgents", "AgentColors", dark, "agents")
        palettes("dusk", "DuskColors", colors, "dusk")
        out.appendLine("}")

        val dir = File(outputDir.get().asFile, "app/todoornot/design")
        dir.mkdirs()
        File(dir, "Tokens.kt").writeText(out.toString())
    }
}

/**
 * Generates `IconPaths.kt`: the Lucide subset and the hand-drawn glyphs as SVG path data (circles, lines
 * and rectangles converted to paths), preserving which shapes are filled with `currentColor`.
 */
abstract class GenerateIcons : DefaultTask() {
    @get:InputDirectory
    @get:PathSensitive(PathSensitivity.RELATIVE)
    abstract val icons: DirectoryProperty

    @get:OutputDirectory
    abstract val outputDir: DirectoryProperty

    private fun attr(tag: String, name: String): String? =
        Regex("""(?:^|\s)$name="([^"]*)"""").find(tag)?.groupValues?.get(1)

    private fun num(tag: String, name: String): Double = attr(tag, name)?.toDouble() ?: 0.0

    private fun fmt(d: Double): String = if (d == Math.floor(d)) d.toLong().toString() else d.toString()

    private fun paths(svg: String): List<Pair<String, Boolean>> =
        Regex("""<(path|circle|line|rect|polyline|polygon)\b([^>]*)/?>""", RegexOption.DOT_MATCHES_ALL).findAll(svg).map { m ->
            val tag = " " + m.groupValues[2].replace(Regex("\\s+"), " ")
            val path = when (m.groupValues[1]) {
                "path" -> attr(tag, "d") ?: error("path without d")
                "circle" -> {
                    val (cx, cy, r) = listOf(num(tag, "cx"), num(tag, "cy"), num(tag, "r"))
                    "M${fmt(cx - r)} ${fmt(cy)}a${fmt(r)} ${fmt(r)} 0 1 0 ${fmt(2 * r)} 0a${fmt(r)} ${fmt(r)} 0 1 0 ${fmt(-2 * r)} 0"
                }
                "line" -> "M${fmt(num(tag, "x1"))} ${fmt(num(tag, "y1"))}L${fmt(num(tag, "x2"))} ${fmt(num(tag, "y2"))}"
                "rect" -> {
                    val (x, y, w, h) = listOf(num(tag, "x"), num(tag, "y"), num(tag, "width"), num(tag, "height"))
                    val r = minOf(num(tag, "rx").takeIf { it > 0 } ?: num(tag, "ry"), w / 2, h / 2)
                    if (r <= 0) {
                        "M${fmt(x)} ${fmt(y)}h${fmt(w)}v${fmt(h)}h${fmt(-w)}Z"
                    } else {
                        "M${fmt(x + r)} ${fmt(y)}h${fmt(w - 2 * r)}a${fmt(r)} ${fmt(r)} 0 0 1 ${fmt(r)} ${fmt(r)}" +
                            "v${fmt(h - 2 * r)}a${fmt(r)} ${fmt(r)} 0 0 1 ${fmt(-r)} ${fmt(r)}" +
                            "h${fmt(-(w - 2 * r))}a${fmt(r)} ${fmt(r)} 0 0 1 ${fmt(-r)} ${fmt(-r)}" +
                            "v${fmt(-(h - 2 * r))}a${fmt(r)} ${fmt(r)} 0 0 1 ${fmt(r)} ${fmt(-r)}Z"
                    }
                }
                else -> {
                    val pts = (attr(tag, "points") ?: "").trim().split(Regex("[\\s,]+")).chunked(2)
                    "M" + pts.joinToString("L") { "${it[0]} ${it[1]}" } + if (m.groupValues[1] == "polygon") "Z" else ""
                }
            }
            path to (attr(tag, "fill") == "currentColor")
        }.toList()

    @TaskAction
    fun generate() {
        val out = StringBuilder()
        out.appendLine("// Generated from assets/icons by GenerateIcons (buildSrc). Do not edit.")
        out.appendLine()
        out.appendLine("package app.todoornot.design")
        out.appendLine()
        out.appendLine("/** A single-colour icon on a 24-unit grid: SVG path data and the stroke width. */")
        out.appendLine("class IconPath(val d: String, val filled: Boolean)")
        out.appendLine("class IconData(val strokeWidth: Float, val paths: List<IconPath>)")
        out.appendLine()
        out.appendLine("object IconPaths {")
        for ((prop, sub) in listOf("lucide" to "lucide", "glyphs" to "glyphs")) {
            val files = File(icons.get().asFile, sub).listFiles { f -> f.extension == "svg" }!!.sortedBy { it.name }
            out.appendLine("    val $prop: Map<String, IconData> by lazy {")
            out.appendLine("        mapOf(")
            files.forEach { file ->
                val svg = file.readText()
                val root = Regex("<svg\\b([^>]*)>", RegexOption.DOT_MATCHES_ALL).find(svg)!!.groupValues[1].replace(Regex("\\s+"), " ")
                val stroke = attr(" $root", "stroke-width")?.toDouble() ?: 2.0
                val ps = paths(svg).joinToString(", ") { (d, fill) -> "IconPath(${kstr(d)}, $fill)" }
                out.appendLine("            ${kstr(file.nameWithoutExtension)} to IconData(${stroke}f, listOf($ps)),")
            }
            out.appendLine("        )")
            out.appendLine("    }")
        }
        out.appendLine("}")
        val dir = File(outputDir.get().asFile, "app/todoornot/design")
        dir.mkdirs()
        File(dir, "IconPaths.kt").writeText(out.toString())
    }
}

/** Copies the bundled fonts into a generated `res/font` with Android resource names. */
abstract class PrepareFonts : DefaultTask() {
    @get:InputDirectory
    @get:PathSensitive(PathSensitivity.RELATIVE)
    abstract val fonts: DirectoryProperty

    @get:OutputDirectory
    abstract val outputDir: DirectoryProperty

    @TaskAction
    fun copy() {
        val dir = File(outputDir.get().asFile, "font")
        dir.deleteRecursively()
        dir.mkdirs()
        fonts.get().asFile.listFiles { f -> f.extension == "ttf" }!!.forEach {
            it.copyTo(File(dir, fontResName(it.name) + ".ttf"), overwrite = true)
        }
    }
}

/**
 * Generates the colour Android needs before Compose runs: the window background (so a cold start doesn't
 * flash white in the dark theme), from the My Day room palette in tokens.json. The launcher icon comes
 * from assets/brand (GenerateLauncherIcon).
 */
abstract class GenerateDesignResources : DefaultTask() {
    @get:InputFile
    @get:PathSensitive(PathSensitivity.NONE)
    abstract val tokens: RegularFileProperty

    @get:OutputDirectory
    abstract val outputDir: DirectoryProperty

    @TaskAction
    fun generate() {
        val root = Json.parse(tokens.get().asFile.readText()).obj()
        val colors = root["colors"].obj()
        fun room(theme: String) = colors[theme].obj()["rooms"].obj()["my-day"].obj()
        fun write(dir: String, theme: String) {
            val r = room(theme)
            val out = StringBuilder()
            out.appendLine("<?xml version=\"1.0\" encoding=\"utf-8\"?>")
            out.appendLine("<!-- Generated from crates/todo-design/generated/tokens.json by GenerateDesignResources. -->")
            out.appendLine("<resources>")
            out.appendLine("    <color name=\"window_background\">${r["pane"]}</color>")
            if (theme == "light") {
                val touch = (root["metrics"].obj()["phone"].obj()["touch"] as Number).toDouble()
                val widgetInset = (root["space"].obj()["3"] as Number).toDouble()
                out.appendLine("    <dimen name=\"todo_widget_radius\">${root["radius"].obj()["lg"]}dp</dimen>")
                out.appendLine("    <dimen name=\"todo_widget_min_width\">${4 * touch}dp</dimen>")
                // WidgetCard: a header and one full row, plus the outer padding and header gap.
                out.appendLine("    <dimen name=\"todo_widget_min_height\">${2 * touch + 3 * widgetInset}dp</dimen>")
            }
            out.appendLine("</resources>")
            val d = File(outputDir.get().asFile, dir)
            d.mkdirs()
            File(d, "design_colors.xml").writeText(out.toString())
        }
        write("values", "light")
        write("values-night", "dark")
    }
}
