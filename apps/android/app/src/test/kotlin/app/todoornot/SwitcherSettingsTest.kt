package app.todoornot

import android.app.Application
import android.content.Intent
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.runtime.mutableStateOf
import androidx.compose.ui.input.key.Key
import androidx.compose.ui.semantics.SemanticsActions
import androidx.compose.ui.test.ExperimentalTestApi
import androidx.compose.ui.test.assertIsFocused
import androidx.compose.ui.test.assertIsNotEnabled
import androidx.compose.ui.test.hasText
import androidx.compose.ui.test.junit4.createComposeRule
import androidx.compose.ui.test.onNodeWithContentDescription
import androidx.compose.ui.test.onNodeWithText
import androidx.compose.ui.test.performClick
import androidx.compose.ui.test.performKeyInput
import androidx.compose.ui.test.performTextInput
import androidx.compose.ui.test.performSemanticsAction
import androidx.compose.ui.test.pressKey
import app.todoornot.core.RoomKey
import app.todoornot.core.SmartList
import app.todoornot.design.Design
import app.todoornot.design.LocalDesign
import app.todoornot.model.Actions
import app.todoornot.model.DevicePairingApi
import app.todoornot.model.DevicePairingModel
import app.todoornot.model.UiState
import app.todoornot.session.ThumbHand
import app.todoornot.ui.BottomSheet
import app.todoornot.ui.SettingsButton
import app.todoornot.ui.SettingsPage
import app.todoornot.ui.LocalPairingOwner
import app.todoornot.ui.MainScreen
import kotlinx.coroutines.test.TestScope
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.RuntimeEnvironment
import org.robolectric.Shadows.shadowOf
import org.robolectric.annotation.Config

@RunWith(RobolectricTestRunner::class)
@Config(application = Application::class, sdk = [35])
@OptIn(ExperimentalTestApi::class)
class SwitcherSettingsTest {
    @Composable
    private fun SettingsHarness(server: String, syncing: Boolean, actions: Actions) {
        var open by remember { mutableStateOf(false) }
        var fromLast by remember { mutableStateOf<Boolean?>(null) }
        var hand by remember { mutableStateOf(ThumbHand.RIGHT) }
        var restoreFocus by remember { mutableStateOf(false) }
        val trigger = remember { FocusRequester() }
        Box(Modifier.fillMaxSize()) {
            SettingsButton(trigger) { fromLast = it; open = true }
            if (open) BottomSheet("Settings", { open = false }) { overlay ->
                SettingsPage(server, syncing, actions, hand, { hand = it }, fromLast) {
                    restoreFocus = true
                    overlay.close()
                }
            }
        }
        LaunchedEffect(open, restoreFocus) { if (!open && restoreFocus) trigger.requestFocus() }
    }
    // Robolectric needs the unconfined rule to establish the initial platform-window focus.
    @Suppress("DEPRECATION")
    @get:Rule val compose = createComposeRule()

    @Test
    fun keyboard_opens_devices_from_settings_and_returns_focus() {
        val owner = DevicePairingModel(object : DevicePairingApi {
            override val pollMs = 1_000L
            override suspend fun create() = error("not expected")
            override suspend fun status(id: String) = error("not expected")
            override suspend fun cancel(id: String) = error("not expected")
            override suspend fun review(code: String, approveAt: Long?) = error("not expected")
        }, TestScope())
        compose.setContent {
            CompositionLocalProvider(LocalDesign provides Design(false, "my-day"), LocalPairingOwner provides owner) {
                SettingsHarness("https://todo.example.com", syncing = false, Actions.None)
            }
        }

        val settings = compose.onNodeWithContentDescription("Settings")
        val admin = compose.onNodeWithText("Open admin pages")
        val sync = compose.onNodeWithText("Sync now")
        val devices = compose.onNodeWithText("Devices…")
        val unpair = compose.onNodeWithText("Unpair this phone")
        settings.performSemanticsAction(SemanticsActions.RequestFocus)
        settings.performKeyInput { pressKey(Key.Enter) }
        // Wait for the in-place sheet and its initial keyboard focus.
        compose.waitUntilExactlyOneExists(hasText("Sync now"), timeoutMillis = 5_000)
        admin.assertIsFocused().performKeyInput { pressKey(Key.DirectionDown) }
        sync.assertIsFocused().performKeyInput { pressKey(Key.DirectionDown) }
        devices.assertIsFocused().performKeyInput { pressKey(Key.DirectionDown) }
        unpair.assertIsFocused().performKeyInput { pressKey(Key.DirectionUp) }
        devices.assertIsFocused().performKeyInput { pressKey(Key.Escape) }
        devices.assertDoesNotExist()
        settings.assertIsFocused().performKeyInput { pressKey(Key.DirectionUp) }
        compose.waitUntilExactlyOneExists(hasText("Unpair this phone"), timeoutMillis = 5_000)
        unpair.assertIsFocused().performKeyInput { pressKey(Key.DirectionUp) }
        devices.assertIsFocused().performKeyInput { pressKey(Key.Spacebar) }
        val invite = compose.onNodeWithText("Pair a new device")
        compose.waitUntilExactlyOneExists(hasText("Pair a new device"), timeoutMillis = 5_000)
        invite.assertIsFocused().performKeyInput { pressKey(Key.Enter) }
        compose.onNodeWithText("Create invite").assertExists()
        compose.onNodeWithText("Close").assertIsFocused().performKeyInput { pressKey(Key.Escape) }
        compose.onNodeWithText("Create invite").assertDoesNotExist()
        settings.assertIsFocused()
    }

    @Test
    fun the_mirrored_dock_opens_a_searchable_switcher_and_routes_a_result() {
        val engine = fixtureEngine()
        val current = RoomKey.Smart(SmartList.MY_DAY)
        val state = UiState(engine.home(FIXTURE_NOW), engine.room(current, FIXTURE_NOW), null, engine.status(), emptyList(), null)
        var opened: RoomKey? = null
        val actions = object : Actions by Actions.None {
            override fun open(room: RoomKey) { opened = room }
        }
        compose.setContent { MainScreen(state, actions, false, false, hand = ThumbHand.LEFT) }
        val rooms = compose.onNodeWithContentDescription("Rooms")
        val filter = compose.onNodeWithContentDescription("Filter")
        assertTrue(rooms.fetchSemanticsNode().boundsInRoot.left > filter.fetchSemanticsNode().boundsInRoot.left)
        rooms.performClick()
        compose.onNodeWithContentDescription("Find a room").performTextInput("ERRANDS")
        compose.onNodeWithText("Errands").performClick()
        compose.runOnIdle { assertEquals(engine.list("Errands"), opened) }
        compose.onNodeWithContentDescription("Find a room").assertDoesNotExist()
    }

    @Test
    fun admin_opens_the_paired_server_in_the_browser() {
        compose.setContent {
            CompositionLocalProvider(LocalDesign provides Design(false, "my-day")) {
                SettingsHarness("https://todo.example.com:8443/", syncing = false, Actions.None)
            }
        }
        val settings = compose.onNodeWithContentDescription("Settings")
        settings.performSemanticsAction(SemanticsActions.RequestFocus)
        settings.performKeyInput { pressKey(Key.Enter) }
        compose.waitUntilExactlyOneExists(hasText("Open admin pages"), timeoutMillis = 5_000)
        compose.onNodeWithText("Open admin pages").assertIsFocused().performKeyInput { pressKey(Key.Enter) }
        val intent = requireNotNull(shadowOf(RuntimeEnvironment.getApplication()).nextStartedActivity)
        assertEquals(Intent.ACTION_VIEW, intent.action)
        assertEquals("https://todo.example.com:8443/admin", intent.dataString)
        assertTrue(intent.hasCategory(Intent.CATEGORY_BROWSABLE))
        settings.assertIsFocused()
    }

    @Test
    fun sync_is_disabled_while_syncing_and_nudges_when_idle() {
        val syncing = mutableStateOf(true)
        var nudges = 0
        val actions = object : Actions by Actions.None {
            override fun nudge() { nudges++ }
        }
        compose.setContent {
            CompositionLocalProvider(LocalDesign provides Design(false, "my-day")) {
                SettingsHarness("https://todo.example.com", syncing.value, actions)
            }
        }
        val settings = compose.onNodeWithContentDescription("Settings")
        val admin = compose.onNodeWithText("Open admin pages")
        val unpair = compose.onNodeWithText("Unpair this phone")
        settings.performSemanticsAction(SemanticsActions.RequestFocus)
        settings.performKeyInput { pressKey(Key.Enter) }
        compose.waitUntilExactlyOneExists(hasText("Syncing…"), timeoutMillis = 5_000)
        compose.onNodeWithText("Syncing…").assertIsNotEnabled()
        admin.assertIsFocused().performKeyInput { pressKey(Key.DirectionDown) }
        unpair.assertIsFocused().performKeyInput { pressKey(Key.DirectionUp) }
        admin.assertIsFocused()
        assertEquals(0, nudges)

        compose.runOnIdle { syncing.value = false }
        admin.performKeyInput { pressKey(Key.DirectionDown) }
        compose.onNodeWithText("Sync now").assertIsFocused().performKeyInput { pressKey(Key.Spacebar) }
        assertEquals(1, nudges)
        settings.assertIsFocused()
    }
}
