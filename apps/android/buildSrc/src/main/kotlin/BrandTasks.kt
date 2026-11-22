import org.gradle.api.DefaultTask
import org.gradle.api.file.DirectoryProperty
import org.gradle.api.tasks.InputDirectory
import org.gradle.api.tasks.OutputDirectory
import org.gradle.api.tasks.PathSensitive
import org.gradle.api.tasks.PathSensitivity
import org.gradle.api.tasks.TaskAction
import org.w3c.dom.Element
import java.io.File
import javax.xml.parsers.DocumentBuilderFactory

/**
 * Generates the launcher icon's layers as VectorDrawables from the brand sources in `assets/brand/` (see
 * its README): `ic_launcher_background`, `ic_launcher_foreground` and `ic_launcher_monochrome` (from the
 * flattened monochrome SVG, which has no mask or clip path). The mark is never redrawn by hand here.
 */
abstract class GenerateLauncherIcon : DefaultTask() {
    @get:InputDirectory
    @get:PathSensitive(PathSensitivity.RELATIVE)
    abstract val brand: DirectoryProperty

    @get:OutputDirectory
    abstract val outputDir: DirectoryProperty

    @TaskAction
    fun generate() {
        val dir = File(outputDir.get().asFile, "drawable")
        dir.deleteRecursively()
        dir.mkdirs()
        val layers = mapOf(
            "ic_launcher_background" to "adaptive-background.svg",
            "ic_launcher_foreground" to "adaptive-foreground.svg",
            "ic_launcher_monochrome" to "generated/adaptive-monochrome-flat.svg",
        )
        for ((name, source) in layers) {
            val svg = File(brand.get().asFile, source)
            File(dir, "$name.xml").writeText(svgToVectorDrawable(svg.readText(), "assets/brand/$source"))
        }
    }
}

/**
 * Converts a flat SVG (rect, circle and path, with fill, stroke and opacity) to a VectorDrawable on the
 * same viewport, 1 unit = 1 dp. Anything else (groups, transforms, masks, gradients) fails the build
 * instead of being dropped.
 */
fun svgToVectorDrawable(svg: String, source: String): String {
    val doc = DocumentBuilderFactory.newInstance().apply {
        isNamespaceAware = false
        setFeature("http://apache.org/xml/features/disallow-doctype-decl", true)
    }.newDocumentBuilder().parse(svg.byteInputStream())
    val root = doc.documentElement
    require(root.tagName == "svg") { "$source: not an SVG" }
    // The root may only say what the canvas is: anything that would change how its children draw
    // (transform, opacity, style, fill, preserveAspectRatio, ...) fails instead of being dropped.
    (0 until root.attributes.length).map { root.attributes.item(it).nodeName }
        .firstOrNull { it !in ROOT_ATTRIBUTES }
        ?.let { error("$source: <svg> has an unsupported attribute '$it'") }
    val w0 = root.getAttribute("width")
    val h0 = root.getAttribute("height")
    val box = root.getAttribute("viewBox").trim().split(Regex("[\\s,]+")).map(String::toDouble)
    require(box.size == 4 && box[0] == 0.0 && box[1] == 0.0) { "$source: the viewBox must start at 0 0" }
    val (w, h) = box[2] to box[3]
    // A width or height other than the viewBox's would scale the drawing.
    require(w0.isEmpty() || w0.toDoubleOrNull() == w) { "$source: <svg> width differs from its viewBox" }
    require(h0.isEmpty() || h0.toDoubleOrNull() == h) { "$source: <svg> height differs from its viewBox" }

    val out = StringBuilder()
    out.appendLine("<?xml version=\"1.0\" encoding=\"utf-8\"?>")
    out.appendLine("<!-- Generated from $source by GenerateLauncherIcon. Do not edit. -->")
    out.appendLine("<vector xmlns:android=\"http://schemas.android.com/apk/res/android\"")
    out.appendLine("    android:width=\"${n(w)}dp\"")
    out.appendLine("    android:height=\"${n(h)}dp\"")
    out.appendLine("    android:viewportWidth=\"${n(w)}\"")
    out.append("    android:viewportHeight=\"${n(h)}\">")
    out.appendLine()
    val children = root.childNodes
    for (i in 0 until children.length) {
        val node = children.item(i)
        when {
            node is Element -> out.append(vectorPath(node, source))
            node.nodeType == org.w3c.dom.Node.COMMENT_NODE -> {}
            node.nodeType == org.w3c.dom.Node.TEXT_NODE && node.textContent.isBlank() -> {}
            else -> error("$source: unexpected content in <svg>")
        }
    }
    out.appendLine("</vector>")
    return out.toString()
}

private val ROOT_ATTRIBUTES = setOf("xmlns", "viewBox", "width", "height")

private val SUPPORTED = setOf(
    "d", "cx", "cy", "r", "x", "y", "width", "height",
    "fill", "fill-rule", "opacity", "fill-opacity", "stroke", "stroke-width", "stroke-linecap", "stroke-linejoin", "stroke-opacity",
)

private fun vectorPath(e: Element, source: String): String {
    val attrs = (0 until e.attributes.length).map { e.attributes.item(it).nodeName }
    attrs.firstOrNull { it !in SUPPORTED }?.let { error("$source: <${e.tagName}> has an unsupported attribute '$it'") }
    fun a(name: String) = e.getAttribute(name).takeIf { it.isNotEmpty() }
    fun d(name: String) = a(name)?.toDouble() ?: 0.0
    val data = when (e.tagName) {
        "path" -> a("d") ?: error("$source: a path without d")
        "rect" -> {
            val (x, y, rw, rh) = listOf(d("x"), d("y"), d("width"), d("height"))
            "M${n(x)},${n(y)}H${n(x + rw)}V${n(y + rh)}H${n(x)}Z"
        }
        "circle" -> {
            val (cx, cy, r) = listOf(d("cx"), d("cy"), d("r"))
            "M${n(cx - r)},${n(cy)}A${n(r)},${n(r)} 0 1 1 ${n(cx + r)},${n(cy)}A${n(r)},${n(r)} 0 1 1 ${n(cx - r)},${n(cy)}Z"
        }
        else -> error("$source: <${e.tagName}> isn't supported; flatten it first")
    }
    val opacity = a("opacity")?.toDouble() ?: 1.0
    val lines = mutableListOf<String>()
    val fill = a("fill") ?: "#000000"
    if (fill != "none") {
        lines += "android:fillColor=\"${color(fill, source)}\""
        val alpha = opacity * (a("fill-opacity")?.toDouble() ?: 1.0)
        if (alpha != 1.0) lines += "android:fillAlpha=\"${n(alpha)}\""
        if (a("fill-rule") == "evenodd") lines += "android:fillType=\"evenOdd\""
    }
    a("stroke")?.takeIf { it != "none" }?.let { stroke ->
        lines += "android:strokeColor=\"${color(stroke, source)}\""
        lines += "android:strokeWidth=\"${n(a("stroke-width")?.toDouble() ?: 1.0)}\""
        val alpha = opacity * (a("stroke-opacity")?.toDouble() ?: 1.0)
        if (alpha != 1.0) lines += "android:strokeAlpha=\"${n(alpha)}\""
        a("stroke-linecap")?.let { lines += "android:strokeLineCap=\"$it\"" }
        a("stroke-linejoin")?.let { lines += "android:strokeLineJoin=\"$it\"" }
    }
    lines += "android:pathData=\"$data\""
    return "    <path\n" + lines.joinToString("\n") { "        $it" } + " />\n"
}

/** `#fff` and `#ffefe1` as `#FFFFFFFF` and `#FFFFEFE1`. */
private fun color(c: String, source: String): String {
    val hex = c.removePrefix("#")
    require(c.startsWith("#") && hex.all { it.isDigit() || it.lowercaseChar() in 'a'..'f' }) { "$source: colour '$c' isn't hex" }
    val six = when (hex.length) {
        3 -> hex.map { "$it$it" }.joinToString("")
        6 -> hex
        else -> error("$source: colour '$c' isn't #rgb or #rrggbb")
    }
    return "#FF" + six.uppercase()
}

private fun n(d: Double): String =
    if (d == Math.floor(d) && !d.isInfinite()) d.toLong().toString() else "%.4f".format(java.util.Locale.ROOT, d).trimEnd('0')
