package app.todoornot.updates

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import app.todoornot.MainActivity
import app.todoornot.widget.todoGraph

internal const val SHOW_UPDATES = "app.todoornot.SHOW_UPDATES"

internal object UpdateAlerts {
    private const val CHANNEL = "app-updates"
    private const val ID = 7914

    fun ready(context: Context, offer: UpdateManifest) {
        val manager = context.getSystemService(NotificationManager::class.java) ?: return
        manager.createNotificationChannel(NotificationChannel(CHANNEL, "App updates", NotificationManager.IMPORTANCE_LOW))
        if (!manager.areNotificationsEnabled()) return
        val flags = PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT
        val show = PendingIntent.getActivity(context, 0, updatesIntent(context), flags)
        val icon = android.R.drawable.stat_sys_download_done
        val notification = Notification.Builder(context, CHANNEL)
            .setSmallIcon(icon)
            .setContentTitle("Update ready")
            .setContentText(offer.versionName)
            .setContentIntent(show)
            .setOnlyAlertOnce(true)
            .setAutoCancel(true)
            // The notification only opens the sheet. Installation takes a second, deliberate tap.
            .addAction(Notification.Action.Builder(android.graphics.drawable.Icon.createWithResource("android", icon), "Install", show).build())
            .build()
        // Permission can be revoked between checking and posting.
        runCatching { manager.notify(ID, notification) }
    }

    fun clear(context: Context) { context.getSystemService(NotificationManager::class.java)?.cancel(ID) }
}

private fun updatesIntent(context: Context) = Intent(context, MainActivity::class.java)
    .setAction(SHOW_UPDATES).addFlags(Intent.FLAG_ACTIVITY_CLEAR_TOP or Intent.FLAG_ACTIVITY_SINGLE_TOP)

/** PackageInstaller fills in status extras on an explicit, app-private PendingIntent. */
class UpdateInstallReceiver : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        val pending = goAsync()
        context.todoGraph.updateInstallResult(intent, pending::finish)
    }
}
