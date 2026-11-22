package app.todoornot

import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test
import org.w3c.dom.Element
import java.io.File
import javax.xml.parsers.DocumentBuilderFactory
import kotlin.math.hypot

/**
 * The launcher icon's key art (halo, sun and tick) stays inside the adaptive icon's 66 dp safe circle, so
 * no launcher mask (circle, squircle, rounded square, teardrop) can cut it. Only the hills bleed to the
 * edges, by design.
 */
class LauncherIconTest {
    private val brand = File(System.getProperty("todo.brand") ?: "../../../assets/brand")

    private fun elements(svg: String): List<Element> {
        val root = DocumentBuilderFactory.newInstance().newDocumentBuilder().parse(File(brand, svg)).documentElement
        assertEquals("0 0 108 108", root.getAttribute("viewBox"))
        return (0 until root.childNodes.length).map { root.childNodes.item(it) }.filterIsInstance<Element>()
    }

    private fun Element.d(name: String) = getAttribute(name).toDouble()

    /** How far a shape reaches from the canvas centre, at most. */
    private fun reach(x: Double, y: Double, extra: Double) = hypot(x - CENTRE, y - CENTRE) + extra

    @Test
    fun the_sun_and_the_tick_stay_inside_the_safe_zone() {
        val shapes = elements("adaptive-foreground.svg")
        val circles = shapes.filter { it.tagName == "circle" }
        assertEquals("the halo and the sun", 2, circles.size)
        circles.forEach { c ->
            val r = reach(c.d("cx"), c.d("cy"), c.d("r"))
            assertTrue("a circle reaches $r dp from the centre", r <= SAFE_RADIUS)
        }
        val tick = shapes.single { it.tagName == "path" && it.getAttribute("fill") == "none" }
        val half = tick.d("stroke-width") / 2
        val points = Regex("""(-?\d+(?:\.\d+)?),(-?\d+(?:\.\d+)?)""").findAll(tick.getAttribute("d"))
            .map { it.groupValues[1].toDouble() to it.groupValues[2].toDouble() }.toList()
        assertEquals(3, points.size)
        points.forEach { (x, y) ->
            val r = reach(x, y, half)
            assertTrue("the tick reaches $r dp from the centre", r <= SAFE_RADIUS)
        }
    }

    @Test
    fun the_background_fills_the_whole_canvas() {
        val tile = elements("adaptive-background.svg").first()
        assertEquals("rect", tile.tagName)
        assertEquals(108.0, tile.d("width"), 0.0)
        assertEquals(108.0, tile.d("height"), 0.0)
    }

    private companion object {
        const val CENTRE = 54.0
        const val SAFE_RADIUS = 33.0
    }
}
