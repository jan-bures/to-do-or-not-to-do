package app.todoornot

import android.app.Application
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.ui.input.key.Key
import androidx.compose.ui.semantics.SemanticsActions
import androidx.compose.ui.test.ExperimentalTestApi
import androidx.compose.ui.test.assertIsFocused
import androidx.compose.ui.test.hasText
import androidx.compose.ui.test.junit4.createComposeRule
import androidx.compose.ui.test.onNodeWithContentDescription
import androidx.compose.ui.test.onNodeWithText
import androidx.compose.ui.test.performKeyInput
import androidx.compose.ui.test.performSemanticsAction
import androidx.compose.ui.test.pressKey
import app.todoornot.design.Design
import app.todoornot.design.LocalDesign
import app.todoornot.model.Actions
import app.todoornot.model.DevicePairingApi
import app.todoornot.model.DevicePairingModel
import app.todoornot.ui.DrawerSettings
import app.todoornot.ui.LocalPairingOwner
import kotlinx.coroutines.test.TestScope
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config

@RunWith(RobolectricTestRunner::class)
@Config(application = Application::class, sdk = [35])
@OptIn(ExperimentalTestApi::class)
class DrawerDeviceSettingsTest {
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
                DrawerSettings("https://todo.example.com", Actions.None)
            }
        }

        val settings = compose.onNodeWithContentDescription("Settings")
        val sync = compose.onNodeWithText("Sync now")
        val devices = compose.onNodeWithText("Devices…")
        val unpair = compose.onNodeWithText("Unpair this phone")
        settings.performSemanticsAction(SemanticsActions.RequestFocus)
        settings.performKeyInput { pressKey(Key.Enter) }
        // A popup has a separate platform window; wait for its Compose root before sending keys.
        compose.waitUntilExactlyOneExists(hasText("Sync now"), timeoutMillis = 5_000)
        sync.assertIsFocused().performKeyInput { pressKey(Key.DirectionDown) }
        devices.assertIsFocused().performKeyInput { pressKey(Key.DirectionDown) }
        unpair.assertIsFocused().performKeyInput { pressKey(Key.DirectionUp) }
        devices.assertIsFocused().performKeyInput { pressKey(Key.Escape) }
        devices.assertDoesNotExist()
        settings.assertIsFocused().performKeyInput { pressKey(Key.DirectionUp) }
        compose.waitUntilExactlyOneExists(hasText("Unpair this phone"), timeoutMillis = 5_000)
        unpair.assertIsFocused().performKeyInput { pressKey(Key.DirectionUp) }
        devices.assertIsFocused().performKeyInput { pressKey(Key.Spacebar) }
        devices.assertDoesNotExist()
        val invite = compose.onNodeWithText("Pair a new device")
        compose.waitUntilExactlyOneExists(hasText("Pair a new device"), timeoutMillis = 5_000)
        invite.assertIsFocused().performKeyInput { pressKey(Key.Enter) }
        compose.onNodeWithText("Create invite").assertExists()
        compose.onNodeWithText("Close").assertIsFocused().performKeyInput { pressKey(Key.Escape) }
        compose.onNodeWithText("Create invite").assertDoesNotExist()
        settings.assertIsFocused()
    }
}
