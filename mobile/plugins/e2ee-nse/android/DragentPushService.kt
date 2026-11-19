// The Android messaging service for end-to-end encrypted push
// (docs/design/e2ee.md §8 step 5). Added to the app by the e2ee-nse config
// plugin in place of expo-notifications' own service (which it extends, so
// token refresh keeps working for expo-notifications).
//
// Every Dragent push is a high-priority data-only message {"e": "<blob>"}:
// nothing is shown by the system. This service decrypts it and posts the
// notification with tag = the notification id, or cancels by tag for a
// dismissal, or posts the constant placeholder from the app's resources on
// any failure (including a 10 s internal deadline). Data messages without
// "e" are not Dragent's and show nothing: server text is never shown.
//
// If the person force-stops the app, Android doesn't deliver data messages
// and nothing is shown (§8: stated plainly).
package dev.dragent.e2ee.push

import android.app.ActivityManager
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.Context
import android.content.Intent
import android.graphics.BitmapFactory
import android.net.Uri
import android.os.Build
import androidx.core.app.NotificationCompat
import androidx.core.app.NotificationManagerCompat
import com.google.firebase.messaging.RemoteMessage
import dev.dragent.e2ee.DragentE2EEKeyStore
import dev.dragent.e2ee.DragentE2EEPush
import dev.dragent.e2ee.DragentForegroundPush
import expo.modules.notifications.service.ExpoFirebaseMessagingService
import java.util.concurrent.Executors
import java.util.concurrent.TimeUnit

class DragentPushService : ExpoFirebaseMessagingService() {
  override fun onMessageReceived(remoteMessage: RemoteMessage) {
    val data = remoteMessage.data
    if (!data.containsKey(DragentPushHandler.BLOB_KEY)) return
    val ctx = applicationContext
    val outcome = handleWithDeadline(ctx, data)
    post(ctx, outcome)
  }

  companion object {
    /** Pops up (heads-up) when the app isn't open. */
    const val CHANNEL = "dragent-alerts"
    /** Into the shade only: the app showed its own banner. */
    const val QUIET_CHANNEL = "dragent-quiet"
    /** The first channel, normal importance: replaced by the two above. */
    private const val OLD_CHANNEL = "dragent-push"
    const val DEADLINE_MS = 10_000L

    fun open(ctx: Context, sealed: ByteArray, nowMs: Long): DragentOpenedPush {
      val n = DragentE2EEPush.open(DragentE2EEKeyStore(ctx), sealed, nowMs)
      return DragentOpenedPush(
        desktopId = n.desktopId,
        id = n.id,
        issuedAtMs = n.issuedAtMs.toLong(),
        dismiss = n.dismiss,
        title = n.title,
        body = n.body,
        workspaceId = n.workspaceId,
        dismissIds = n.dismissIds,
        kind = n.kind.toInt(),
        tabId = n.tabId,
      )
    }

    fun handleWithDeadline(ctx: Context, data: Map<String, String>): DragentPushOutcome {
      val pool = Executors.newSingleThreadExecutor()
      return try {
        pool.submit<DragentPushOutcome> {
          DragentPushHandler.handle(data, System.currentTimeMillis(), DragentSeenIds(ctx)) { s, t -> open(ctx, s, t) }
        }.get(DEADLINE_MS, TimeUnit.MILLISECONDS)
      } catch (_: Exception) {
        DragentPushOutcome.Placeholder("timeout")
      } finally {
        pool.shutdownNow()
      }
    }

    private fun string(ctx: Context, name: String, fallback: String): String {
      val id = ctx.resources.getIdentifier(name, "string", ctx.packageName)
      return if (id != 0) ctx.getString(id) else fallback
    }

    /** The app is on screen (its process is in the foreground). */
    private fun inForeground(ctx: Context): Boolean {
      val info = ActivityManager.RunningAppProcessInfo()
      ActivityManager.getMyMemoryState(info)
      return info.importance == ActivityManager.RunningAppProcessInfo.IMPORTANCE_FOREGROUND
    }

    private fun icon(ctx: Context): Int {
      val id = ctx.resources.getIdentifier("notification_icon", "drawable", ctx.packageName)
      return if (id != 0) id else ctx.applicationInfo.icon
    }

    fun post(ctx: Context, outcome: DragentPushOutcome) {
      val manager = NotificationManagerCompat.from(ctx)
      if (outcome is DragentPushOutcome.Dismiss) {
        for (tag in outcome.tags) manager.cancel(tag, 0)
        return
      }
      // Open in front: the app shows its own banner, and the notification
      // goes quietly into the shade. Otherwise it pops up.
      val inApp = outcome is DragentPushOutcome.Show && inForeground(ctx) &&
        DragentForegroundPush.deliver(outcome.title, outcome.body, outcome.desktop)
      val channel = if (inApp) QUIET_CHANNEL else CHANNEL
      if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
        val system = ctx.getSystemService(NotificationManager::class.java)
        if (system.getNotificationChannel(OLD_CHANNEL) != null) system.deleteNotificationChannel(OLD_CHANNEL)
        if (system.getNotificationChannel(CHANNEL) == null) {
          system.createNotificationChannel(
            NotificationChannel(CHANNEL, string(ctx, "dragent_push_channel", "Agents"), NotificationManager.IMPORTANCE_HIGH).apply {
              description = "When an agent finishes or needs you"
            },
          )
        }
        if (system.getNotificationChannel(QUIET_CHANNEL) == null) {
          system.createNotificationChannel(
            NotificationChannel(QUIET_CHANNEL, "While Dragent is open", NotificationManager.IMPORTANCE_LOW).apply {
              description = "Kept in the shade; the app showed it already"
            },
          )
        }
      }
      val launch = ctx.packageManager.getLaunchIntentForPackage(ctx.packageName)
      val builder = NotificationCompat.Builder(ctx, channel)
        .setSmallIcon(icon(ctx))
        .setAutoCancel(true)
        .setCategory(NotificationCompat.CATEGORY_MESSAGE)
        .setPriority(if (inApp) NotificationCompat.PRIORITY_LOW else NotificationCompat.PRIORITY_HIGH)
      val tag: String
      when (outcome) {
        is DragentPushOutcome.Show -> {
          builder.setContentTitle(outcome.title).setContentText(outcome.body)
            .setStyle(NotificationCompat.BigTextStyle().bigText(outcome.body))
          // The agent's icon beside the text.
          DragentPushHandler.agentIcon(outcome.title)?.let { key ->
            val id = ctx.resources.getIdentifier("agent_$key", "drawable", ctx.packageName)
            if (id != 0) BitmapFactory.decodeResource(ctx.resources, id)?.let { builder.setLargeIcon(it) }
          }
          outcome.group?.let { builder.setGroup(it) }
          launch?.putExtra("dragent_desktop", outcome.desktop)
          tag = outcome.tag
          // Allow / Deny: each opens the app's answer screen, which asks the
          // desktop to answer (it checks the agent is still waiting).
          outcome.pane?.let { pane ->
            for ((index, choice) in listOf("allow" to "Allow", "deny" to "Deny").withIndex()) {
              val uri = Uri.parse("dragent://machine/${Uri.encode(outcome.desktop)}/answer?pane=${Uri.encode(pane)}&choice=${choice.first}")
              val intent = Intent(Intent.ACTION_VIEW, uri).setPackage(ctx.packageName)
              val flags = PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE
              builder.addAction(0, choice.second, PendingIntent.getActivity(ctx, (tag + choice.first).hashCode() + index, intent, flags))
            }
          }
        }
        else -> {
          builder.setContentTitle(string(ctx, "dragent_push_placeholder_title", "Dragent"))
            .setContentText(string(ctx, "dragent_push_placeholder_body", "An agent needs you"))
          tag = "dragent-placeholder"
        }
      }
      if (launch != null) {
        val flags = PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE
        builder.setContentIntent(PendingIntent.getActivity(ctx, tag.hashCode(), launch, flags))
      }
      try {
        manager.notify(tag, 0, builder.build())
      } catch (_: SecurityException) {
        // POST_NOTIFICATIONS not granted: nothing can be shown.
      }
    }
  }
}
