package app.todoornot.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ColumnScope
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.selection.selectable
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.layout.layout
import androidx.compose.ui.platform.LocalUriHandler
import androidx.compose.ui.semantics.LiveRegionMode
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.heading
import androidx.compose.ui.semantics.liveRegion
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.constrainWidth
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.offset
import app.todoornot.design.Icon
import app.todoornot.design.LocalDesign
import app.todoornot.design.Tokens
import app.todoornot.model.AssistantModel
import app.todoornot.model.AssistantProvider
import app.todoornot.model.AssistantUi
import app.todoornot.model.ChatGptSignIn

private val S = Tokens.Space
private val T = Tokens.Type
private val P = Tokens.Metrics.Phone
private val C = Tokens.Metrics.Component
private const val USAGE_URL = "https://chatgpt.com/settings/usage"
private const val VERIFY_URL = "https://auth.openai.com/codex/device"

private data class ModelPicker(val account: String?)

@Composable
internal fun HostedAssistantSettings(provider: AssistantProvider, ui: AssistantUi, model: AssistantModel?) {
    val r = LocalDesign.current.room
    val browser = LocalUriHandler.current
    val state = ui.panel.hosted
    var picker by remember(provider) { mutableStateOf<ModelPicker?>(null) }
    val choose: (String?) -> Unit = { account ->
        picker = ModelPicker(account)
        model?.hostedModels(account)
    }
    if (provider == AssistantProvider.CHATGPT) HostedCard(provider.label, cloud = true) {
        Label("Sign in once to use your ChatGPT plan or available credits on paired devices.", T.BODY, r.ink2, maxLines = Int.MAX_VALUE)
        val signingIn = ui.code != null || ui.browserSignIn
        val enabled = !ui.busy && !signingIn
        val accounts = state?.accounts.orEmpty()
        if (accounts.isNotEmpty()) Column(Modifier.fillMaxWidth()) {
            accounts.forEach { account ->
                val selected = state?.selectedAccount == account.id
                Row(Modifier.fillMaxWidth().hairline(r.line, top = true), verticalAlignment = Alignment.CenterVertically,
                    horizontalArrangement = Arrangement.spacedBy(S.S2.dp)) {
                    Row(Modifier.weight(1f).heightIn(min = P.TOUCH.dp).clip(RoundedCornerShape(Tokens.Radius.MD.dp))
                        .then(if (selected) Modifier.background(r.wash) else Modifier)
                        .selectable(selected, enabled = enabled, role = Role.RadioButton, onClick = {
                            if (!selected) picker = null
                            model?.useChatgptAccount(account.id)
                        }).padding(horizontal = S.S1.dp),
                        horizontalArrangement = Arrangement.spacedBy(S.S2.dp), verticalAlignment = Alignment.CenterVertically) {
                        Box(Modifier.size(C.ICON_SMALL.dp)) {
                            if (selected) Icon("check", r.accentStrong, size = C.ICON_SMALL.dp)
                        }
                        Label(account.label, T.BODY, r.ink, Modifier.weight(1f))
                    }
                    HostedLink("Sign out", "Sign out of ${account.label}", enabled = enabled, alignEnd = true) {
                        picker = null
                        model?.signOutChatgpt(account.id)
                    }
                }
            }
        }
        ui.signedIn?.let { account ->
            Label("Connected ${account.label}.", T.META_PHONE, r.ink2,
                Modifier.semantics { liveRegion = LiveRegionMode.Polite })
        }
        if (provider.available(state)) HostedLink("Manage usage", icon = "external-link") {
            model?.clearSignInStatus()
            browser.openUri(USAGE_URL)
        }
        if (ui.code != null) {
            Label("Open ChatGPT and enter this one-time code. Continue only if you started this sign-in.", T.BODY, r.ink2, maxLines = Int.MAX_VALUE)
            Label(ui.code.userCode, T.COUNT, r.ink)
            Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(S.S2.dp)) {
                AiButton("Open ChatGPT", { browser.openUri(VERIFY_URL) }, primary = true)
                AiButton("Cancel sign-in", { model?.cancelChatgpt() }, quiet = true)
            }
            Label("Waiting for ChatGPT…", T.META_PHONE, r.ink2, Modifier.semantics { liveRegion = LiveRegionMode.Polite })
        } else if (ui.browserSignIn) {
            Label("Waiting for ChatGPT…", T.META_PHONE, r.ink2, Modifier.semantics { liveRegion = LiveRegionMode.Polite })
            HostedLink("Cancel sign-in") { model?.cancelChatgpt() }
        } else if (provider.available(state)) {
            FlowRow(horizontalArrangement = Arrangement.spacedBy(S.S2.dp)) {
                ChatGptSignIn.actions(state).forEach { action ->
                    if (action == ChatGptSignIn.CODE) HostedLink(action.label, enabled = enabled) { model?.startChatgpt() }
                    else AiButton(action.label, { model?.startChatgptBrowser(browser::openUri) },
                        enabled = enabled, primary = true, maxLines = 1)
                }
            }
        } else Label(state?.disabledReason ?: "Not configured on your server.", T.META_PHONE, r.ink2, maxLines = Int.MAX_VALUE)
        val active = accounts.firstOrNull { it.id == state?.selectedAccount }
        active?.let { account ->
            HostedModelRow(state?.model.orEmpty(), enabled) { choose(account.id) }
            picker?.takeIf { it.account == account.id }?.let { HostedModelOptions(it, ui, model) { picker = null } }
        }
    } else HostedCard(provider.label) {
        Label(if (provider.available(state)) "Your To-do server connects to the private model. The key stays on your server."
            else "Not configured on your server.", T.BODY, r.ink2, maxLines = Int.MAX_VALUE)
        if (provider.available(state)) {
            HostedModelRow(state?.model.orEmpty().takeIf { state?.privateSelected == true }.orEmpty(), !ui.busy) { choose(null) }
            picker?.let { HostedModelOptions(it, ui, model) { picker = null } }
        }
    }
}

@Composable
private fun HostedCard(title: String, cloud: Boolean = false, content: @Composable ColumnScope.() -> Unit) {
    val r = LocalDesign.current.room
    val shape = RoundedCornerShape(Tokens.Radius.LG.dp)
    Column(Modifier.fillMaxWidth().clip(shape).background(r.raised)
        .border(Tokens.Elevation.HAIRLINE.dp, r.accentStrong, shape).padding(S.S3.dp),
        verticalArrangement = Arrangement.spacedBy(S.S2.dp)) {
        Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) {
            Label(title, T.TASK_TITLE, r.ink, Modifier.weight(1f).semantics { heading() })
            if (cloud) Label("Cloud", T.META_PHONE, r.ink3)
        }
        content()
    }
}

@Composable
private fun HostedLink(
    text: String,
    description: String = text,
    icon: String? = null,
    enabled: Boolean = true,
    alignEnd: Boolean = false,
    onClick: () -> Unit,
) {
    val r = LocalDesign.current.room
    val ink = if (enabled) r.accentStrong else r.ink3
    // Let the padded touch target overflow at the aligned edge without moving the label.
    Row(Modifier.layout { measurable, constraints ->
        val inset = S.S1.dp.roundToPx()
        val placeable = measurable.measure(constraints.offset(horizontal = inset))
        layout(constraints.constrainWidth(placeable.width - inset), placeable.height) {
            placeable.placeRelative(if (alignEnd) 0 else -inset, 0)
        }
    }.heightIn(min = P.TOUCH.dp).clip(RoundedCornerShape(Tokens.Radius.MD.dp))
        .clickable(enabled = enabled, role = Role.Button, onClickLabel = description, onClick = onClick)
        .padding(horizontal = S.S1.dp), horizontalArrangement = Arrangement.spacedBy(S.S1.dp),
        verticalAlignment = Alignment.CenterVertically) {
        Label(text, T.BUTTON_SMALL, ink)
        icon?.let { Icon(it, ink, size = C.ICON_SMALL.dp) }
    }
}

@Composable
private fun HostedModelRow(model: String, enabled: Boolean, choose: () -> Unit) {
    val r = LocalDesign.current.room
    Row(Modifier.fillMaxWidth().hairline(r.line, top = true).padding(top = S.S2.dp),
        horizontalArrangement = Arrangement.spacedBy(S.S2.dp), verticalAlignment = Alignment.CenterVertically) {
        Label("Model", T.META_PHONE, r.ink2)
        Label(model.ifBlank { "No model selected" }, T.COUNT, r.ink, Modifier.weight(1f))
        AiButton("Choose model", choose, enabled = enabled, maxLines = 1)
    }
}

@Composable
private fun HostedModelOptions(picker: ModelPicker, ui: AssistantUi, model: AssistantModel?, close: () -> Unit) {
    val r = LocalDesign.current.room
    val choices = ui.hostedModels
    when {
        choices == null && ui.problem == null -> Label("Loading models…", T.META_PHONE, r.ink2)
        choices.isNullOrEmpty() -> Label("No models are available.", T.META_PHONE, r.ink2)
        else -> choices.forEach { item ->
            AiButton(item.label, { model?.useHosted(picker.account, item.id); close() }, Modifier.fillMaxWidth(),
                icon = if (ui.panel.hosted?.model == item.id) "check" else null, enabled = !ui.busy, quiet = true)
        }
    }
}

@Composable
internal fun CloudScopeReview(ui: AssistantUi, model: AssistantModel?) {
    val review = ui.scopeReview ?: return
    val r = LocalDesign.current.room
    HostedCard("Share with ChatGPT?") {
        Label("This uses your ChatGPT plan or available credits. Hidden and Private only lists aren’t shared.", T.BODY, r.ink2, maxLines = Int.MAX_VALUE)
        Label(if (review.names.isEmpty()) "No task lists are shared." else "Lists ChatGPT can read: ${review.names.joinToString(", ")}.",
            T.BODY, r.ink, maxLines = Int.MAX_VALUE)
        Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(S.S2.dp)) {
            AiButton("Cancel", { model?.declineScope() }, Modifier.weight(1f), enabled = !ui.busy, quiet = true)
            AiButton("Allow and send", { model?.approveScope() }, Modifier.weight(1f), enabled = !ui.busy, primary = true)
        }
    }
}
