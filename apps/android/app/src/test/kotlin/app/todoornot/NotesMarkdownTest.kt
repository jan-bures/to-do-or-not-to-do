package app.todoornot

import androidx.compose.ui.text.LinkAnnotation
import androidx.compose.ui.text.font.FontStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextDecoration
import app.todoornot.core.NoteBlock
import app.todoornot.core.parseNote
import app.todoornot.design.Design
import app.todoornot.design.Fonts
import app.todoornot.design.Tokens
import app.todoornot.ui.notesText
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Test

class NotesMarkdownTest {
    @Test
    fun `the bridge preserves nested blocks and checkbox byte offsets`() {
        val source = "# Příručka\n\n5. Setup\n   - [x] **Ready** now\n\n> Leave room.\n\n---\n\n```text\n  station = north\n\n```"
        val (heading, list, quote, rule, code) = parseNote(source).blocks
        assertEquals(1u.toUByte(), (heading as NoteBlock.Heading).level)
        assertEquals("Příručka", heading.spans.single().text)
        assertEquals(5uL, (list as NoteBlock.ListBlock).start)
        val nested = list.items.single().blocks[1] as NoteBlock.ListBlock
        assertNull(nested.start)
        val checkbox = requireNotNull(nested.items.single().checkbox)
        assertEquals("Ready now", checkbox.label)
        assertEquals(true, checkbox.checked)
        assertEquals(source.substringBefore("[x]").toByteArray(Charsets.UTF_8).size.toUInt(), checkbox.offset)
        assertEquals("Leave room.", ((quote as NoteBlock.Quote).blocks.single() as NoteBlock.Paragraph).spans.single().text)
        assertEquals(NoteBlock.Rule, rule)
        assertEquals("  station = north\n", (code as NoteBlock.Code).text)
    }

    @Test
    fun `Compose styles source as text and makes only approved destinations actionable`() {
        val source = "🔑 **one** *two* ~~old~~ `code` [**floor** plan](https://example.com/plan) " +
            "[email](mailto:you@example.com) [no](javascript:alert%281%29) <kbd>Enter</kbd> ![map](map.png)"
        val spans = (parseNote(source).blocks.single() as NoteBlock.Paragraph).spans
        val colors = Design.palette(dark = false, hue = "my-day")
        val opened = mutableListOf<String>()
        val text = notesText(spans, colors, opened::add)
        fun styleOf(word: String) = text.spanStyles.single { text.text.substring(it.start, it.end) == word }.item
        assertEquals(FontWeight(Tokens.Weight.NOTES_BOLD), styleOf("one").fontWeight)
        assertEquals(FontStyle.Italic, styleOf("two").fontStyle)
        assertEquals(TextDecoration.LineThrough, styleOf("old").textDecoration)
        assertEquals(Fonts.mono, styleOf("code").fontFamily)
        assertEquals(colors.canvas, styleOf("code").background)
        assertEquals(TextDecoration.Underline, styleOf("floor").textDecoration)
        assertEquals("🔑 one two old code floor plan email no <kbd>Enter</kbd> ![map](map.png)", text.text)
        val links = text.getLinkAnnotations(0, text.length)
        assertEquals(listOf("floor plan", "email"), links.map { text.text.substring(it.start, it.end) })
        for ((link) in links) {
            link as LinkAnnotation.Url
            assertEquals(colors.accentStrong, link.styles?.style?.color)
            link.linkInteractionListener?.onClick(link)
        }
        assertEquals(listOf("https://example.com/plan", "mailto:you@example.com"), opened)
        assertFalse(spans.any { it.link?.startsWith("javascript:") == true })
    }
}
