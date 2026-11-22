package app.todoornot.ui

import android.provider.Settings
import androidx.compose.foundation.border
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.selection.toggleable
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.runtime.withFrameNanos
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.focus.onFocusChanged
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.dp
import app.todoornot.design.Icon
import app.todoornot.design.LocalDesign
import app.todoornot.design.RoomColors
import app.todoornot.design.Tokens
import app.todoornot.model.Pace
import app.todoornot.model.ease

internal const val IMPORTANT: UByte = 3u

internal enum class TaskToggle(val icon: String, val label: String) {
    MY_DAY("sun", "My Day"), IMPORTANT("star", "Important"),
}

/** The active ink shared by task toggles and their menu equivalents. */
internal fun RoomColors.taskToggleFill(icon: String): Color? = when (icon) {
    "sun-filled" -> sunInk
    "star-filled" -> importantFill
    else -> null
}

internal fun toggleScale(progress: Float, on: Boolean, reduced: Boolean): Float =
    if (!on || reduced) 1f else Tokens.Motion.TOGGLE_FILL_FROM_SCALE +
        (1f - Tokens.Motion.TOGGLE_FILL_FROM_SCALE) * Tokens.Motion.EASE_POP.ease(progress)

/** Keep the hit target still, and expose the checked state independently of the glyph or focus. */
@Composable
internal fun TaskToggleButton(
    toggle: TaskToggle,
    on: Boolean,
    enabled: Boolean = true,
    label: String = toggle.label,
    alignment: Alignment = Alignment.Center,
    onChange: (Boolean) -> Unit,
) {
    val r = LocalDesign.current.room
    val context = LocalContext.current
    val target = if (on) 1f else 0f
    var fill by remember(toggle) { mutableFloatStateOf(target) }
    var scale by remember(toggle) { mutableFloatStateOf(1f) }
    var focused by remember { mutableStateOf(false) }
    LaunchedEffect(toggle, on) {
        val from = fill
        if (from == target) return@LaunchedEffect
        val pace = Pace(Settings.Global.getFloat(context.contentResolver, Settings.Global.ANIMATOR_DURATION_SCALE, 1f))
        val duration = pace.fade(Tokens.Motion.CHECK_FILL) * 1_000_000L
        val start = withFrameNanos { it }
        var progress: Float
        do {
            progress = if (duration <= 0) 1f else withFrameNanos { ((it - start).toFloat() / duration).coerceIn(0f, 1f) }
            fill = from + (target - from) * Tokens.Motion.EASE_STANDARD.ease(progress)
            scale = toggleScale(progress, on, pace.reduced)
        } while (progress < 1f)
    }
    Box(
        Modifier.size(Tokens.Metrics.Phone.TOUCH.dp)
            .clip(CircleShape)
            .onFocusChanged { focused = it.isFocused }
            .border(Tokens.Elevation.HAIRLINE.dp, if (focused) r.accentStrong else Color.Transparent, CircleShape)
            .semantics { contentDescription = label }
            .toggleable(value = on, enabled = enabled, role = Role.Checkbox, onValueChange = onChange),
        contentAlignment = alignment,
    ) {
        Icon(toggle.icon, r.ink3, Modifier.graphicsLayer { alpha = 1f - fill })
        val filled = "${toggle.icon}-filled"
        Icon(filled, r.taskToggleFill(filled) ?: r.ink3,
            Modifier.graphicsLayer { alpha = fill; scaleX = scale; scaleY = scale })
    }
}
