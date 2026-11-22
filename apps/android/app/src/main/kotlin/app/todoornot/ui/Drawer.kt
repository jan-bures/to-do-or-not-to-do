package app.todoornot.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.semantics.selected
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.dp
import app.todoornot.core.Home
import app.todoornot.core.NavItem
import app.todoornot.core.RoomKey
import app.todoornot.core.SyncState
import app.todoornot.core.SyncStatus
import app.todoornot.design.Glyph
import app.todoornot.design.Icon
import app.todoornot.design.LocalDesign
import app.todoornot.design.Tokens

private val S = Tokens.Space
private val P = Tokens.Metrics.Phone

/** The rooms: smart lists, then each collection with its lists. Glyphs in each list's hue. */
@Composable
fun RoomDrawer(home: Home, current: RoomKey, status: SyncStatus, server: String?, onOpen: (RoomKey) -> Unit) {
    val r = LocalDesign.current.room
    Column(
        Modifier
            .fillMaxHeight()
            // The desktop sidebar's width, plus room for touch-sized rows.
            .width(Tokens.Metrics.Desktop.SIDEBAR.dp + S.S10.dp)
            .background(r.canvas)
            .statusBarsPadding()
            .navigationBarsPadding(),
    ) {
        Column(Modifier.weight(1f).verticalScroll(rememberScrollState()).padding(horizontal = S.S3.dp, vertical = S.S3.dp)) {
            home.smart.forEach { NavRow(it, it.key == current, indent = false, onOpen) }
            home.groups.forEach { g ->
                Spacer(Modifier.height(S.S4.dp))
                g.collection?.let { NavRow(it, it.key == current, indent = false, onOpen, heading = true) }
                g.lists.forEach { NavRow(it, it.key == current, indent = g.collection != null, onOpen) }
            }
        }
        Row(
            Modifier.fillMaxWidth().hairline(r.line, top = true).padding(horizontal = S.S5.dp, vertical = S.S3.dp),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(S.S2.dp),
        ) {
            val color = when (status.state) {
                SyncState.ONLINE -> r.success
                SyncState.SYNCING -> r.accent
                SyncState.OFFLINE -> r.danger
                SyncState.PAUSED -> r.ink4
            }
            Box(Modifier.size(Tokens.Metrics.Component.CHIP_DOT.dp).background(color, CircleShape))
            Label(server?.removePrefix("https://") ?: "Not paired", Tokens.Type.COUNT, r.ink3, Modifier.weight(1f))
            if (status.pending > 0u) Label("${status.pending} waiting", Tokens.Type.COUNT, r.ink3)
        }
    }
}

@Composable
internal fun NavRow(item: NavItem, on: Boolean, indent: Boolean, onOpen: (RoomKey) -> Unit, heading: Boolean = false) {
    val d = LocalDesign.current
    val r = d.room
    val l = d.list(item.hue)
    TouchTarget({ onOpen(item.key) }, Modifier.fillMaxWidth().semantics { selected = on }) { press ->
        Row(
            Modifier
                .fillMaxWidth()
                .height(P.NAV_ROW.dp)
                .clip(RoundedCornerShape(Tokens.Radius.MD.dp))
                .background(if (on) r.raised else r.canvas)
                .then(press)
                .padding(start = if (indent) S.S8.dp else S.S2.dp, end = S.S3.dp),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(S.S3.dp),
        ) {
            when {
                item.glyph != null -> Glyph(item.glyph!!, l.glyph)
                item.icon != null -> Icon(item.icon!!, l.glyph)
                else -> Box(Modifier.size(Tokens.Metrics.Component.GLYPH.dp))
            }
            val token = when {
                heading -> Tokens.Type.NAV_COLLECTION
                indent -> Tokens.Type.NAV_SUB
                else -> Tokens.Type.NAV
            }
            Label(item.name, token, r.ink, Modifier.weight(1f))
            if (item.open > 0u) Label("${item.open}", Tokens.Type.COUNT, r.ink3)
        }
    }
}
