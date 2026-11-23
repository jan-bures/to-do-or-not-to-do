package app.todoornot.updates

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.net.Uri
import android.os.Bundle
import androidx.activity.ComponentActivity
import app.todoornot.MainActivity
import app.todoornot.widget.todoGraph

internal const val SHOW_UPDATES = "app.todoornot.SHOW_UPDATES"
private const val VERSION = "version"

internal object UpdateAlerts {
    private const val CHANNEL = "app-updates"
    private const val ID = 7914

    fun ready(context: Context, offer: UpdateManifest) {
        val manager = context.getSystemService(NotificationManager::class.java) ?: return
        manager.createNotificationChannel(NotificationChannel(CHANNEL, "App updates", NotificationManager.IMPORTANCE_LOW))
        if (!manager.areNotificationsEnabled()) return
        val flags = PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT
        val show = PendingIntent.getActivity(context, 0, updatesIntent(context), flags)
        val install = PendingIntent.getActivity(context, 0, Intent(context, UpdateInstallActivity::class.java)
            .setData(Uri.parse("todo-update://ready/${offer.versionCode}"))
            .putExtra(VERSION, offer.versionCode), flags)
        val icon = android.R.drawable.stat_sys_download_done
        val notification = Notification.Builder(context, CHANNEL)
            .setSmallIcon(icon)
            .setContentTitle("Update ready")
            .setContentText(offer.versionName)
            .setContentIntent(show)
            .setOnlyAlertOnce(true)
            .setAutoCancel(true)
            .addAction(Notification.Action.Builder(android.graphics.drawable.Icon.createWithResource("android", icon), "Install", install).build())
            .build()
        // Permission can be revoked between checking and posting.
        runCatching { manager.notify(ID, notification) }
    }

    fun clear(context: Context) { context.getSystemService(NotificationManager::class.java)?.cancel(ID) }
}

private fun updatesIntent(context: Context) = Intent(context, MainActivity::class.java)
    .setAction(SHOW_UPDATES).addFlags(Intent.FLAG_ACTIVITY_CLEAR_TOP or Intent.FLAG_ACTIVITY_SINGLE_TOP)

/** A private Activity lets the notification's explicit Install tap authorize the same flow as Settings.
 * MainActivity is exported but its SHOW_UPDATES action only opens a sheet, never installs. */
class UpdateInstallActivity : ComponentActivity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        if (savedInstanceState == null) applicationContext.todoGraph.installUpdate(intent.getLongExtra(VERSION, -1))
        startActivity(updatesIntent(this))
        finish()
    }
}

/** PackageInstaller fills in status extras on an explicit, app-private PendingIntent. */
class UpdateInstallReceiver : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        val pending = goAsync()
        context.todoGraph.updateInstallResult(intent, pending::finish)
    }
}
