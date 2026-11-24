package app.todoornot.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ColumnScope
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.platform.LocalUriHandler
import androidx.compose.ui.semantics.LiveRegionMode
import androidx.compose.ui.semantics.heading
import androidx.compose.ui.semantics.liveRegion
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.dp
import app.todoornot.design.LocalDesign
import app.todoornot.design.Tokens
import app.todoornot.model.AssistantModel
import app.todoornot.model.AssistantUi

private val S = Tokens.Space
private val T = Tokens.Type
private const val USAGE_URL = "https://chatgpt.com/settings/usage"
private const val VERIFY_URL = "https://auth.openai.com/codex/device"

private data class ModelPicker(val account: String?)

@Composable
internal fun HostedAssistantSettings(ui: AssistantUi, model: AssistantModel?) {
    val r = LocalDesign.current.room
    val browser = LocalUriHandler.current
    val state = ui.panel.hosted
    var picker by remember { mutableStateOf<ModelPicker?>(null) }
    val choose: (String?) -> Unit = { account ->
        picker = ModelPicker(account)
        model?.hostedModels(account)
    }
    HostedCard("ChatGPT", state?.selectedAccount != null) {
        Label("Cloud", T.META_PHONE, r.ink3)
        Label("Sign in once to use your ChatGPT plan or available credits on paired devices.", T.BODY, r.ink2, maxLines = Int.MAX_VALUE)
        AiButton("Manage usage", { browser.openUri(USAGE_URL) }, quiet = true)
        state?.accounts.orEmpty().forEach { account ->
            val selected = state?.selectedAccount == account.id
            Label(account.label, T.BODY, r.ink, maxLines = 2)
            Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(S.S2.dp)) {
                AiButton(if (selected) "Choose model" else "Use account", { choose(account.id) }, Modifier.weight(1f), enabled = !ui.busy)
                AiButton("Sign out", { picker = null; model?.signOutChatgpt(account.id) }, enabled = !ui.busy, quiet = true)
            }
        }
        if (ui.code != null) {
            Label("Open ChatGPT and enter this one-time code. Continue only if you started this sign-in.", T.BODY, r.ink2, maxLines = Int.MAX_VALUE)
            Label(ui.code.userCode, T.COUNT, r.ink)
            Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(S.S2.dp)) {
                AiButton("Open ChatGPT", { browser.openUri(VERIFY_URL) }, primary = true)
                AiButton("Cancel sign-in", { model?.cancelChatgpt() }, quiet = true)
            }
            Label("Waiting for ChatGPT…", T.META_PHONE, r.ink2, Modifier.semantics { liveRegion = LiveRegionMode.Polite })
        } else if (state?.deviceCode == true) {
            AiButton(if (state.accounts.isEmpty()) "Sign in with ChatGPT" else "Add another ChatGPT account",
                { model?.startChatgpt() }, enabled = !ui.busy)
        } else if (state?.accounts.isNullOrEmpty()) {
            Label(if (state?.chatgpt == true) "Sign in on a paired desktop to connect an account." else "Not configured on your server.",
                T.BODY, r.ink2, maxLines = Int.MAX_VALUE)
        }
        picker?.takeIf { it.account != null }?.let { HostedModelOptions(it, ui, model) { picker = null } }
    }
    HostedCard("Your private models (via your server)", state?.privateSelected == true) {
        Label(if (state?.privateModels == true) "Your To-do server connects to the private model. The key stays on your server."
            else "Not configured on your server.", T.BODY, r.ink2, maxLines = Int.MAX_VALUE)
        if (state?.privateModels == true) {
            AiButton(if (state.privateSelected) "Choose model" else "Use private models", { choose(null) }, enabled = !ui.busy)
        }
        picker?.takeIf { it.account == null }?.let { HostedModelOptions(it, ui, model) { picker = null } }
    }
}

@Composable
private fun HostedCard(title: String, selected: Boolean, content: @Composable ColumnScope.() -> Unit) {
    val r = LocalDesign.current.room
    val shape = RoundedCornerShape(Tokens.Radius.LG.dp)
    Column(Modifier.fillMaxWidth().clip(shape).background(r.raised)
        .border(Tokens.Elevation.HAIRLINE.dp, if (selected) r.accentStrong else r.lineStrong, shape).padding(S.S3.dp),
        verticalArrangement = Arrangement.spacedBy(S.S2.dp)) {
        Label(title, T.TASK_TITLE, r.ink, Modifier.semantics { heading() }, maxLines = 2)
        content()
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
    HostedCard("Share with ChatGPT?", true) {
        Label("This uses your ChatGPT plan or available credits. Hidden and Private only lists aren’t shared.", T.BODY, r.ink2, maxLines = Int.MAX_VALUE)
        Label(if (review.names.isEmpty()) "No task lists are shared." else "Lists ChatGPT can read: ${review.names.joinToString(", ")}.",
            T.BODY, r.ink, maxLines = Int.MAX_VALUE)
        Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(S.S2.dp)) {
            AiButton("Cancel", { model?.declineScope() }, Modifier.weight(1f), enabled = !ui.busy, quiet = true)
            AiButton("Allow and send", { model?.approveScope() }, Modifier.weight(1f), enabled = !ui.busy, primary = true)
        }
    }
}
