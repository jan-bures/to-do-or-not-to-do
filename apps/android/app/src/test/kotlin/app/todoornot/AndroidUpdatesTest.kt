package app.todoornot

import android.app.Application
import android.content.Context
import android.content.Intent
import android.content.pm.PackageInstaller
import android.net.Uri
import app.todoornot.updates.AndroidUpdates
import app.todoornot.updates.UpdateManifest
import app.todoornot.updates.UpdateMode
import java.io.File
import kotlinx.coroutines.runBlocking
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.RuntimeEnvironment
import org.robolectric.annotation.Config

@RunWith(RobolectricTestRunner::class)
@Config(application = Application::class, sdk = [35])
class AndroidUpdatesTest {
    @Test fun rejected_install_discards_the_apk_and_remembers_it_across_restart() = runBlocking {
        val app = RuntimeEnvironment.getApplication()
        val feed = "https://todo.example.com/owner/repo"
        val raw = """{"versionCode":999999,"versionName":"0.1.0-build.999999","size":3,
            "fileName":"todo-android-0.1.0-build.999999.apk","minSdk":29,"sha256":"${"a".repeat(64)}",
            "signingCertSha256":["${"b".repeat(64)}"]}"""
        val release = UpdateManifest.parse(raw)
        val prefs = app.getSharedPreferences("app_updates", Context.MODE_PRIVATE)
        val apk = File(File(app.noBackupFilesDir, "app-updates"), "update.apk")
        apk.parentFile?.mkdirs()
        for (status in listOf(PackageInstaller.STATUS_FAILURE_INCOMPATIBLE, PackageInstaller.STATUS_FAILURE_CONFLICT,
            PackageInstaller.STATUS_FAILURE_INVALID)) {
            prefs.edit().clear().putString("feed", feed).putString("manifest", raw).putInt("session", 42)
                .putString("sessionFingerprint", release.fingerprint).apply()
            apk.writeBytes(byteArrayOf(1, 2, 3))
            val beforeRestart = AndroidUpdates(app, feed)
            beforeRestart.activate()
            assertTrue(beforeRestart.state.value.ready)
            beforeRestart.installationResult(Intent().setData(Uri.parse("todo-update://install/42"))
                .putExtra(PackageInstaller.EXTRA_STATUS, status))
            assertFalse(apk.exists())
            assertFalse(beforeRestart.state.value.ready)
            assertEquals(release.fingerprint, prefs.getString("failed", null))

            val restarted = AndroidUpdates(app, feed)
            restarted.activate()
            assertTrue(restarted.state.value.downloadBlocked)
            assertFalse(restarted.run(UpdateMode.WIFI, null, retrying = false, mayRetry = true))
            assertTrue(restarted.state.value.problem.orEmpty().contains("couldn't use this update"))
        }
    }
}
