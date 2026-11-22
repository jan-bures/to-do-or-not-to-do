package app.todoornot

import app.todoornot.design.IconPaths
import app.todoornot.design.Tokens
import app.todoornot.ui.toggleScale
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class TaskToggleTest {
    @Test
    fun toggle_icons_fill_only_the_star_and_sun_disc() {
        for (key in listOf("star", "sun")) {
            val outline = IconPaths.lucide.getValue(key).paths
            val filled = IconPaths.lucide.getValue("$key-filled").paths
            assertEquals(outline.map { it.d }, filled.map { it.d })
            assertTrue(outline.none { it.filled })
            assertTrue(filled.first().filled)
            assertFalse(filled.drop(1).any { it.filled })
        }
    }

    @Test
    fun scale_pops_only_when_turning_on_with_full_motion() {
        assertEquals(Tokens.Motion.TOGGLE_FILL_FROM_SCALE, toggleScale(0f, true, false), 0f)
        assertTrue(toggleScale(0.6f, true, false) > 1f)
        assertEquals(1f, toggleScale(1f, true, false), 0f)
        assertEquals(1f, toggleScale(0.6f, true, true), 0f)
        assertEquals(1f, toggleScale(0.6f, false, false), 0f)
    }
}
