package io.github.hi9841.tdt.bubble

import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.Context
import android.content.Intent
import androidx.core.app.NotificationCompat
import androidx.core.app.NotificationManagerCompat
import io.github.hi9841.tdt.R

/**
 * While the bubble is hidden, a quiet notification offers to bring it back. It times out with the 10-minute
 * hide, so it never outlives the state it describes. Without the notification permission the bubble still
 * returns after 10 minutes. [BubbleService] handles [ACTION_SHOW].
 */
object HiddenNotice {
    private const val CHANNEL = "bubble"
    private const val ID = 1
    const val ACTION_SHOW = "io.github.hi9841.tdt.action.SHOW_BUBBLE"

    fun post(context: Context, hiddenMillis: Long) {
        if (!allowed(context)) return
        val manager = context.getSystemService(NotificationManager::class.java)
        manager.createNotificationChannel(
            NotificationChannel(CHANNEL, "Hidden bubble", NotificationManager.IMPORTANCE_LOW),
        )
        // The bubble service receives this, so it can also close the notification shade.
        val restore = PendingIntent.getBroadcast(
            context, 0, Intent(ACTION_SHOW).setPackage(context.packageName), PendingIntent.FLAG_IMMUTABLE,
        )
        val notification = NotificationCompat.Builder(context, CHANNEL)
            .setSmallIcon(R.drawable.ic_bubble)
            .setContentTitle("TDT bubble hidden")
            .setContentText("It comes back in 10 minutes.")
            .setContentIntent(restore)
            .addAction(0, "Show bubble", restore)
            .setTimeoutAfter(hiddenMillis)
            .setAutoCancel(true)
            .setSilent(true)
            .build()
        manager.notify(ID, notification)
    }

    /** Correct on every Android version; the notification permission exists only from Android 13. */
    fun allowed(context: Context) = NotificationManagerCompat.from(context).areNotificationsEnabled()

    fun cancel(context: Context) = context.getSystemService(NotificationManager::class.java).cancel(ID)
}
