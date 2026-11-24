package app.todoornot.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicText
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.drawBehind
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.layout.layout
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.semantics.heading
import androidx.compose.ui.semantics.hideFromAccessibility
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.LinkAnnotation
import androidx.compose.ui.text.SpanStyle
import androidx.compose.ui.text.TextLinkStyles
import androidx.compose.ui.text.buildAnnotatedString
import androidx.compose.ui.text.font.FontStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.rememberTextMeasurer
import androidx.compose.ui.text.style.TextDecoration
import androidx.compose.ui.text.withStyle
import androidx.compose.ui.unit.Constraints
import androidx.compose.ui.unit.LayoutDirection
import androidx.compose.ui.unit.isSpecified
import androidx.compose.ui.unit.dp
import app.todoornot.core.NoteBlock
import app.todoornot.core.NoteCheckbox
import app.todoornot.core.NoteDocument
import app.todoornot.core.NoteSpan
import app.todoornot.design.Fonts
import app.todoornot.design.LocalDesign
import app.todoornot.design.RoomColors
import app.todoornot.design.TextToken
import app.todoornot.design.Tokens
import app.todoornot.design.style

private val S = Tokens.Space
private val T = Tokens.Type
private val C = Tokens.Metrics.Component
private val touch = Tokens.Metrics.Phone.TOUCH.dp

/** The bridge maps style flags and allowed links onto Compose ranges, not onto HTML or a WebView. */
internal fun notesText(spans: List<NoteSpan>, colors: RoomColors, onLink: (String) -> Unit) = buildAnnotatedString {
    var activeLink: String? = null
    var start = 0
    fun closeLink() {
        val url = activeLink ?: return
        val styles = TextLinkStyles(style = SpanStyle(color = colors.accentStrong))
        addLink(LinkAnnotation.Url(url, styles, linkInteractionListener = { onLink(url) }), start, length)
    }
    for (span in spans) {
        if (span.link != activeLink) {
            closeLink()
            activeLink = span.link
            start = length
        }
        withStyle(SpanStyle(
            color = if (span.code) colors.ink else Color.Unspecified,
            background = if (span.code) colors.canvas else Color.Unspecified,
            fontFamily = if (span.code) Fonts.of(T.NOTES_CODE.role) else null,
            fontWeight = if (span.bold) FontWeight(Tokens.Weight.NOTES_BOLD) else null,
            fontStyle = if (span.italic) FontStyle.Italic else null,
            textDecoration = when {
                span.strike && span.link != null -> TextDecoration.combine(listOf(TextDecoration.LineThrough, TextDecoration.Underline))
                span.strike -> TextDecoration.LineThrough
                span.link != null -> TextDecoration.Underline
                else -> null
            },
        )) { append(span.text) }
    }
    closeLink()
}

@Composable
internal fun MarkdownNotes(
    document: NoteDocument,
    editable: Boolean,
    onEdit: () -> Unit,
    onToggle: (NoteCheckbox) -> Unit,
    onLink: (String) -> Unit,
) {
    val measurer = rememberTextMeasurer()
    val density = LocalDensity.current
    val measure = remember(measurer, density) {
        val cells = "0".repeat(Tokens.Metrics.Desktop.DETAIL_TEXT_MEASURE.toInt())
        with(density) { measurer.measure(cells, T.NOTES.style(), softWrap = false).size.width.toDp() }
    }
    val currentEdit by rememberUpdatedState(onEdit)
    // Do not merge the preview's semantics into a single edit button: TalkBack can still navigate
    // headings, links and checkboxes individually. The Notes heading is the keyboard/a11y edit target.
    val tap = if (editable) Modifier.pointerInput(Unit) { detectTapGestures { currentEdit() } } else Modifier
    Box(
        Modifier.widthIn(max = measure).fillMaxWidth().heightIn(min = touch).then(tap),
        contentAlignment = Alignment.CenterStart,
    ) {
        NoteBlocks(document.blocks, editable, onToggle, onLink)
    }
}

@Composable
private fun NoteBlocks(
    blocks: List<NoteBlock>,
    editable: Boolean,
    onToggle: (NoteCheckbox) -> Unit,
    onLink: (String) -> Unit,
    modifier: Modifier = Modifier,
) {
    val r = LocalDesign.current.room
    Column(modifier.fillMaxWidth(), verticalArrangement = Arrangement.spacedBy(S.S2.dp)) {
        for (block in blocks) when (block) {
            is NoteBlock.Paragraph -> NoteText(block.spans, T.NOTES, r.ink2, onLink)
            is NoteBlock.Heading -> {
                val type = when (block.level.toInt()) {
                    1 -> T.NOTES_H1
                    2 -> T.NOTES_H2
                    else -> T.NOTES_H3
                }
                NoteText(block.spans, type, r.ink, onLink, Modifier.semantics { heading() })
            }
            is NoteBlock.Quote -> NoteBlocks(
                block.blocks, editable, onToggle, onLink,
                Modifier.drawBehind {
                    val stroke = Tokens.Elevation.HAIRLINE.dp.toPx()
                    val x = if (layoutDirection == LayoutDirection.Ltr) stroke / 2 else size.width - stroke / 2
                    drawLine(r.lineStrong, Offset(x, 0f), Offset(x, size.height), stroke)
                }.padding(start = S.S2.dp),
            )
            is NoteBlock.Code -> Box(
                Modifier.fillMaxWidth().clip(RoundedCornerShape(Tokens.Radius.BUTTON.dp)).background(r.canvas)
                    .horizontalScroll(rememberScrollState()).padding(S.S2.dp),
            ) {
                BasicText(block.text, style = T.NOTES_CODE.style(r.ink), softWrap = false)
            }
            is NoteBlock.Rule -> Box(Modifier.fillMaxWidth().height(Tokens.Elevation.HAIRLINE.dp).background(r.lineStrong))
            is NoteBlock.ListBlock -> Column(verticalArrangement = Arrangement.spacedBy(S.S1.dp)) {
                val start = block.start
                block.items.forEachIndexed { i, item ->
                    val checkbox = item.checkbox
                    Row(horizontalArrangement = Arrangement.spacedBy(S.S2.dp)) {
                        if (checkbox == null) {
                            val marker = if (start == null) "•" else "${start + i.toULong().coerceAtMost(ULong.MAX_VALUE - start)}."
                            Box(Modifier.widthIn(min = C.ICON_SMALL.dp), contentAlignment = Alignment.TopEnd) {
                                Label(marker, T.NOTES, r.ink2, Modifier.semantics { hideFromAccessibility() })
                            }
                            NoteBlocks(item.blocks, editable, onToggle, onLink, Modifier.weight(1f))
                        } else {
                            // Occupy the marker column and the first line like the desktop and web; the
                            // 48 dp touch target overflows invisibly instead of pushing the text aside.
                            val line = T.NOTES.style(r.ink2).lineHeight
                            Check(checkbox.checked, { onToggle(checkbox) }, checkbox.label,
                                modifier = Modifier.layout { measurable, _ ->
                                    val target = touch.roundToPx()
                                    val width = C.ICON_SMALL.dp.roundToPx()
                                    val height = if (line.isSpecified) line.roundToPx() else width
                                    val placeable = measurable.measure(Constraints.fixed(target, target))
                                    layout(width, height) { placeable.place((width - target) / 2, (height - target) / 2) }
                                },
                                size = C.ICON_SMALL.dp, enabled = editable)
                            NoteBlocks(item.blocks, editable, onToggle, onLink, Modifier.weight(1f))
                        }
                    }
                }
            }
        }
    }
}

@Composable
private fun NoteText(spans: List<NoteSpan>, type: TextToken, color: Color, onLink: (String) -> Unit, modifier: Modifier = Modifier) {
    val r = LocalDesign.current.room
    val currentLink by rememberUpdatedState(onLink)
    val text = remember(spans, r) { notesText(spans, r) { currentLink(it) } }
    BasicText(text, modifier, style = type.style(color))
}
