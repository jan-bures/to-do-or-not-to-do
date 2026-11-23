package app.todoornot.design

import androidx.compose.foundation.text.selection.LocalTextSelectionColors
import androidx.compose.foundation.text.selection.TextSelectionColors
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.darkColorScheme
import androidx.compose.material3.lightColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.Immutable
import androidx.compose.runtime.staticCompositionLocalOf
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.Font
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.LineHeightStyle
import androidx.compose.ui.unit.em
import androidx.compose.ui.unit.sp

/**
 * The design for one room: its palette in the current theme, plus lookups for list, agent and dusk
 * colours. Every colour on screen comes from here, and everything here comes from the generated
 * [Tokens].
 */
@Immutable
class Design(val dark: Boolean, val roomHue: String) {
    val room: RoomColors = palette(dark, roomHue)

    fun list(hue: String): ListColors =
        (if (dark) Tokens.darkLists else Tokens.lightLists)[hue]
            ?: (if (dark) Tokens.darkLists else Tokens.lightLists).getValue("lagoon")

    /** An agent's mark colours: its own palette for known agents, else a swatch's list colours. */
    fun agent(hue: String, known: Boolean): AgentColors {
        if (known) {
            (if (dark) Tokens.darkAgents else Tokens.lightAgents)[hue]?.let { return it }
        }
        val l = list(hue)
        return AgentColors(bg = l.tint, ink = l.ink, ring = l.swatch)
    }

    fun dusk(): DuskColors = Tokens.dusk[roomHue] ?: Tokens.dusk.getValue("my-day")

    /** The one shadow, for transient overlays: menus, popovers and the date picker. */
    val overlayShadow: List<ShadowLayer> get() = if (dark) Tokens.overlayShadowDark else Tokens.overlayShadowLight

    /** The modal scrim under a dialog, in the room's hue. */
    val scrim: Color
        get() = (if (dark) Tokens.scrimDark else Tokens.scrimLight).let { it[roomHue] ?: it.getValue("my-day") }

    fun withRoom(hue: String): Design = if (hue == roomHue) this else Design(dark, hue)

    companion object {
        fun palette(dark: Boolean, hue: String): RoomColors =
            (if (dark) Tokens.darkRooms else Tokens.lightRooms)[hue]
                ?: (if (dark) Tokens.darkRooms else Tokens.lightRooms).getValue("my-day")
    }
}

val LocalDesign = staticCompositionLocalOf { Design(dark = false, roomHue = "my-day") }

/** The bundled families, built from the generated font faces. */
object Fonts {
    private fun family(role: FontRole): FontFamily = FontFamily(
        Tokens.fontFaces.filter { it.role == role }.map {
            Font(it.res, FontWeight(it.weight), if (it.italic) FontStyle.Italic else FontStyle.Normal)
        },
    )

    val ui = family(FontRole.UI)
    val mono = family(FontRole.MONO)

    fun of(role: FontRole): FontFamily = when (role) {
        FontRole.UI -> ui
        FontRole.MONO -> mono
    }
}

/** A token as a Compose text style (sizes in sp, so the system font scale still applies). */
fun TextToken.style(color: Color = Color.Unspecified): TextStyle = TextStyle(
    color = color,
    fontFamily = Fonts.of(role),
    fontWeight = FontWeight(weight),
    fontStyle = if (italic) FontStyle.Italic else FontStyle.Normal,
    fontSize = size.sp,
    lineHeight = lineHeight.sp,
    letterSpacing = trackingEm.em,
    fontFeatureSettings = if (tabular) "tnum" else null,
    lineHeightStyle = LineHeightStyle(LineHeightStyle.Alignment.Center, LineHeightStyle.Trim.None),
)

/** Text for a token that is set in capitals. */
fun TextToken.text(s: String): String = if (uppercase) s.uppercase() else s

/**
 * Provides [Design] for `roomHue` and a Material theme mapped onto it, so the few Material pieces the
 * app uses (the drawer, text selection, ripples) take the room's colours too.
 */
@Composable
fun TodoTheme(dark: Boolean, roomHue: String, content: @Composable () -> Unit) {
    val design = Design(dark, roomHue)
    val r = design.room
    val scheme = if (dark) {
        darkColorScheme(
            primary = r.accent, onPrimary = r.onAccent, background = r.pane, onBackground = r.ink,
            surface = r.pane, onSurface = r.ink, surfaceVariant = r.canvas, onSurfaceVariant = r.ink2,
            outline = r.control, outlineVariant = r.line, error = r.danger, scrim = r.toast,
            surfaceContainerLow = r.canvas, surfaceContainer = r.raised, surfaceContainerHigh = r.raised,
        )
    } else {
        lightColorScheme(
            primary = r.accent, onPrimary = r.onAccent, background = r.pane, onBackground = r.ink,
            surface = r.pane, onSurface = r.ink, surfaceVariant = r.canvas, onSurfaceVariant = r.ink2,
            outline = r.control, outlineVariant = r.line, error = r.danger, scrim = r.toast,
            surfaceContainerLow = r.canvas, surfaceContainer = r.raised, surfaceContainerHigh = r.raised,
        )
    }
    MaterialTheme(colorScheme = scheme) {
        CompositionLocalProvider(
            LocalDesign provides design,
            LocalTextSelectionColors provides TextSelectionColors(r.accent, r.tint),
            content = content,
        )
    }
}
