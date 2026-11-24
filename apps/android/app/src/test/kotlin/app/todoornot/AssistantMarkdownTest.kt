package app.todoornot

import app.todoornot.core.AssistantMarkdownBlock
import app.todoornot.core.AssistantMarkdownItem
import app.todoornot.core.AssistantMarkdownSpan
import app.todoornot.core.assistantVisibleTaskLinks
import app.todoornot.core.parseAssistantMarkdown
import org.junit.Assert.assertEquals
import org.junit.Test

class AssistantMarkdownTest {
    @Test
    fun `the bridge preserves reply styles markers and text`() {
        val source = "# Weekend plan\n\n**First**, *then* `plan`.\n\n5. Return books\n9. [Reference](https://example.com)"
        assertEquals(listOf(
            AssistantMarkdownBlock.Heading(1u.toUByte(), listOf(AssistantMarkdownSpan.Text("Weekend plan"))),
            AssistantMarkdownBlock.Paragraph(listOf(
                AssistantMarkdownSpan.Bold("First"),
                AssistantMarkdownSpan.Text(", "),
                AssistantMarkdownSpan.Italic("then"),
                AssistantMarkdownSpan.Text(" "),
                AssistantMarkdownSpan.Code("plan"),
                AssistantMarkdownSpan.Text("."),
            )),
            AssistantMarkdownBlock.OrderedList(listOf(
                AssistantMarkdownItem(5u, listOf(AssistantMarkdownSpan.Text("Return books"))),
                AssistantMarkdownItem(9u, listOf(AssistantMarkdownSpan.Text("[Reference](https://example.com)"))),
            )),
        ), parseAssistantMarkdown(source))
        assertEquals(5u, assistantVisibleTaskLinks())
    }
}
