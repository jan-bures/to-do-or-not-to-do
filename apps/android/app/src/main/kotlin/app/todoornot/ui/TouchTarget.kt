package app.todoornot.ui

import androidx.compose.foundation.LocalIndication
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.indication
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.heightIn
import androidx.compose.runtime.Composable
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import app.todoornot.design.Tokens

/**
 * A touch target at least the touch token tall around a smaller visual (a drawer row's highlight, a room
 * pill). A tap anywhere in the target counts; the press ripple stays on the visual. [visual] gets the
 * modifier to end its chain with, after its clip and background. [onLongClick] (a list's menu, say) is
 * also offered to accessibility services, as [longClickLabel].
 */
@Composable
fun TouchTarget(
    onClick: () -> Unit,
    modifier: Modifier = Modifier,
    onLongClick: (() -> Unit)? = null,
    longClickLabel: String? = null,
    visual: @Composable (press: Modifier) -> Unit,
) {
    val press = remember { MutableInteractionSource() }
    Box(
        modifier
            .heightIn(min = Tokens.Metrics.Phone.TOUCH.dp)
            .combinedClickable(
                interactionSource = press,
                indication = null,
                onLongClickLabel = longClickLabel,
                onLongClick = onLongClick,
                onClick = onClick,
            ),
        contentAlignment = Alignment.CenterStart,
    ) {
        visual(Modifier.indication(press, LocalIndication.current))
    }
}
