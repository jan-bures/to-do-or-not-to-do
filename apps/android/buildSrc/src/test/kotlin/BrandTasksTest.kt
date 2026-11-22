import org.junit.Assert.assertEquals
import org.junit.Assert.assertThrows
import org.junit.Assert.assertTrue
import org.junit.Test
import java.io.File

class BrandTasksTest {
    private fun svg(root: String, body: String = """<circle cx="54" cy="54" r="10" fill="#f5ae39"/>""") =
        """<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 108 108"$root>$body</svg>"""

    @Test
    fun a_flat_svg_converts_shape_for_shape() {
        val xml = svgToVectorDrawable(
            svg("", """<rect width="108" height="108" fill="#ffefe1"/><circle cx="54" cy="56" r="27.5" fill="#f5ae39" opacity="0.22"/>""" +
                """<path d="M42.5,56.25 L49.5,63.25" fill="none" stroke="#241c15" stroke-width="5.8" stroke-linecap="round"/>"""),
            "test.svg",
        )
        assertTrue(xml.contains("""android:viewportWidth="108""""))
        assertTrue(xml.contains("""android:pathData="M0,0H108V108H0Z""""))
        assertTrue(xml.contains("""android:fillColor="#FFF5AE39""""))
        assertTrue(xml.contains("""android:fillAlpha="0.22""""))
        assertTrue(xml.contains("""android:strokeLineCap="round""""))
        assertEquals(3, Regex("<path").findAll(xml).count())
    }

    @Test
    fun root_attributes_that_change_the_drawing_fail_the_build() {
        listOf(
            """ transform="scale(2)"""",
            """ opacity="0.5"""",
            """ style="fill:red"""",
            """ fill="#000"""",
            """ preserveAspectRatio="none"""",
            """ width="216"""",
        ).forEach { attr ->
            assertThrows(attr, RuntimeException::class.java) { svgToVectorDrawable(svg(attr), "test.svg") }
        }
    }

    @Test
    fun a_size_equal_to_the_viewbox_is_fine() {
        svgToVectorDrawable(svg(""" width="108" height="108""""), "test.svg")
    }

    @Test
    fun groups_masks_and_unknown_attributes_fail_the_build() {
        listOf(
            """<g><circle cx="1" cy="1" r="1"/></g>""",
            """<mask id="m"/>""",
            """<circle cx="1" cy="1" r="1" transform="rotate(4)"/>""",
            """<circle cx="1" cy="1" r="1" fill="url(#g)"/>""",
        ).forEach { body ->
            assertThrows(body, RuntimeException::class.java) { svgToVectorDrawable(svg("", body), "test.svg") }
        }
    }

    @Test
    fun the_brand_sources_convert() {
        val brand = File("../../../assets/brand")
        listOf("adaptive-background.svg", "adaptive-foreground.svg", "generated/adaptive-monochrome-flat.svg").forEach {
            assertTrue(svgToVectorDrawable(File(brand, it).readText(), it).contains("<path"))
        }
    }
}
