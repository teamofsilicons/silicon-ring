package com.teamofsilicons.ring.call

import android.Manifest
import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.graphics.Bitmap
import android.graphics.Canvas
import android.graphics.Paint
import android.graphics.Typeface
import android.media.AudioAttributes
import android.media.RingtoneManager
import android.util.Log
import androidx.core.app.NotificationCompat
import androidx.core.app.NotificationManagerCompat
import androidx.core.app.Person
import androidx.core.content.ContextCompat
import androidx.core.content.res.ResourcesCompat
import androidx.core.graphics.createBitmap
import androidx.core.graphics.drawable.IconCompat
import androidx.core.graphics.toColorInt
import com.teamofsilicons.ring.R
import com.teamofsilicons.ring.data.Account
import com.teamofsilicons.ring.ui.MainActivity
import com.teamofsilicons.ring.ui.call.CallActivity

/** Notification channels and the call, missed-call and voicemail notifications. */
object CallNotifications {
    private const val TAG = "RingNotifications"

    const val CHANNEL_INCOMING = "calls_incoming"
    const val CHANNEL_ONGOING = "calls_ongoing"
    const val CHANNEL_MISSED = "calls_missed"
    const val CHANNEL_VOICEMAIL = "voicemail"

    const val SERVICE_ID = 7100

    private val accent = "#1F5CB1".toColorInt()

    fun createChannels(context: Context) {
        val manager = context.getSystemService(NotificationManager::class.java)
        val alertSound = RingtoneManager.getDefaultUri(RingtoneManager.TYPE_NOTIFICATION)
        val alertAttributes = AudioAttributes.Builder()
            .setUsage(AudioAttributes.USAGE_NOTIFICATION)
            .setContentType(AudioAttributes.CONTENT_TYPE_SONIFICATION)
            .build()
        manager.createNotificationChannels(
            listOf(
                // Ring plays the ringtone itself so it can stop it on every device at once.
                NotificationChannel(CHANNEL_INCOMING, "Incoming calls", NotificationManager.IMPORTANCE_HIGH).apply {
                    description = "Rings when someone calls you on Ring"
                    setSound(null, null)
                    enableVibration(false)
                    lockscreenVisibility = Notification.VISIBILITY_PUBLIC
                },
                NotificationChannel(CHANNEL_ONGOING, "Ongoing calls", NotificationManager.IMPORTANCE_DEFAULT).apply {
                    description = "The call you are on"
                    setSound(null, null)
                    enableVibration(false)
                    lockscreenVisibility = Notification.VISIBILITY_PUBLIC
                },
                NotificationChannel(CHANNEL_MISSED, "Missed calls", NotificationManager.IMPORTANCE_HIGH).apply {
                    description = "Calls you did not answer"
                    setSound(alertSound, alertAttributes)
                },
                NotificationChannel(CHANNEL_VOICEMAIL, "Voicemail", NotificationManager.IMPORTANCE_HIGH).apply {
                    description = "New voicemail messages"
                    setSound(alertSound, alertAttributes)
                },
            ),
        )
    }

    /** The service notification for [ui]: an incoming CallStyle, or an ongoing one. */
    fun forCall(context: Context, ui: CallUi): Notification =
        if (ui.phase == CallPhase.Incoming) incoming(context, ui) else ongoing(context, ui)

    fun postIncoming(context: Context, ui: CallUi) {
        if (!canNotify(context)) return
        runCatching {
            @Suppress("MissingPermission")
            NotificationManagerCompat.from(context).notify(incomingId(ui.ringid), incoming(context, ui))
        }.onFailure { Log.w(TAG, "Could not post the incoming call notification", it) }
    }

    fun cancelIncoming(context: Context, ringid: String) {
        NotificationManagerCompat.from(context).cancel(incomingId(ringid))
    }

    private fun incoming(context: Context, ui: CallUi): Notification {
        val screen = callScreenIntent(context, ui.ringid, null, 1)
        val answer = callScreenIntent(context, ui.ringid, CallActivity.ACTION_ANSWER, 2)
        val decline = CallActionReceiver.intent(context, CallActionReceiver.ACTION_DECLINE, ui.ringid)
        val text = when {
            ui.alreadyOn.isNotEmpty() -> "With " + ui.alreadyOn.joinToString(", ") { it.label }
            ui.peer.isSilicon -> "Silicon · ${ui.peer.id}"
            else -> ui.peer.id
        }
        return NotificationCompat.Builder(context, CHANNEL_INCOMING)
            .setSmallIcon(R.drawable.ic_stat_ring)
            .setColor(accent)
            .setContentTitle(ui.peer.label)
            .setContentText(if (ui.silenced) "Silenced · $text" else text)
            .setStyle(NotificationCompat.CallStyle.forIncomingCall(person(context, ui.peer), decline, answer))
            .setFullScreenIntent(screen, true)
            .setContentIntent(screen)
            .setCategory(NotificationCompat.CATEGORY_CALL)
            .setPriority(NotificationCompat.PRIORITY_MAX)
            .setVisibility(NotificationCompat.VISIBILITY_PUBLIC)
            .setOngoing(true)
            // Not setSilent(): that files the notification in a group whose alerts are suppressed,
            // and SystemUI then skips both the heads-up and the full-screen intent. The channel
            // itself is silent because Ring plays the ringtone.
            .setOnlyAlertOnce(true)
            .setWhen(System.currentTimeMillis())
            .build()
    }

    private fun ongoing(context: Context, ui: CallUi): Notification {
        val screen = callScreenIntent(context, ui.ringid, null, 3)
        val hangUp = CallActionReceiver.intent(context, CallActionReceiver.ACTION_HANG_UP, ui.ringid)
        val status = when (ui.phase) {
            CallPhase.Calling -> "Calling…"
            CallPhase.Ringing -> "Ringing"
            CallPhase.Connecting -> "Connecting…"
            CallPhase.Voicemail -> "Leaving a message"
            CallPhase.Ended -> ui.endedText?.lowercase()?.replaceFirstChar { it.uppercase() } ?: "Call ended"
            else -> if (ui.muted) "On call · muted" else "On call"
        }
        val builder = NotificationCompat.Builder(context, CHANNEL_ONGOING)
            .setSmallIcon(R.drawable.ic_stat_ring)
            .setColor(accent)
            .setContentTitle(ui.peer.label)
            .setContentText(status)
            .setStyle(NotificationCompat.CallStyle.forOngoingCall(person(context, ui.peer), hangUp))
            .setContentIntent(screen)
            .setCategory(NotificationCompat.CATEGORY_CALL)
            .setVisibility(NotificationCompat.VISIBILITY_PUBLIC)
            .setOngoing(true)
            .setOnlyAlertOnce(true)
            .setSilent(true)
        ui.answeredAt?.takeIf { ui.phase == CallPhase.Active }?.let {
            builder.setUsesChronometer(true).setShowWhen(true).setWhen(it.toEpochMilli())
        }
        return builder.build()
    }

    /** The call service's notification while it rejoins a call after Ring's process restarted. */
    fun resuming(context: Context): Notification {
        val open = PendingIntent.getActivity(
            context, 0,
            Intent(context, MainActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK),
            PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
        )
        return NotificationCompat.Builder(context, CHANNEL_ONGOING)
            .setSmallIcon(R.drawable.ic_stat_ring)
            .setColor(accent)
            .setContentTitle("Ring")
            .setContentText("Reconnecting your call…")
            .setContentIntent(open)
            .setCategory(NotificationCompat.CATEGORY_CALL)
            .setOngoing(true)
            .setSilent(true)
            .build()
    }

    /** A placeholder for the call service when the call it was started for has already ended. */
    fun idle(context: Context): Notification =
        NotificationCompat.Builder(context, CHANNEL_ONGOING)
            .setSmallIcon(R.drawable.ic_stat_ring)
            .setColor(accent)
            .setContentTitle("Ring")
            .setContentText("Call ended")
            .setCategory(NotificationCompat.CATEGORY_CALL)
            .setSilent(true)
            .build()

    fun missed(context: Context, ringid: String, from: Account) {
        if (!canNotify(context)) return
        val open = PendingIntent.getActivity(
            context, ringid.hashCode(),
            MainActivity.intentForCall(context, ringid),
            PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
        )
        val callBack = PendingIntent.getActivity(
            context, ringid.hashCode() + 1,
            MainActivity.intentForCallBack(context, from.id),
            PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
        )
        val notification = NotificationCompat.Builder(context, CHANNEL_MISSED)
            .setSmallIcon(R.drawable.ic_stat_ring)
            .setColor(accent)
            .setContentTitle("Missed call")
            .setContentText(from.label)
            .setSubText(from.id)
            .setContentIntent(open)
            .addAction(R.drawable.ms_call, "Call back", callBack)
            .setCategory(NotificationCompat.CATEGORY_MISSED_CALL)
            .setAutoCancel(true)
            .setOnlyAlertOnce(true)
            .build()
        runCatching {
            @Suppress("MissingPermission")
            NotificationManagerCompat.from(context).notify("missed:$ringid".hashCode(), notification)
        }.onFailure { Log.w(TAG, "Could not post the missed call notification", it) }
    }

    fun voicemail(context: Context, voicemailId: String, from: Account, transcript: String?) {
        if (!canNotify(context)) return
        val open = PendingIntent.getActivity(
            context, voicemailId.hashCode(),
            MainActivity.intentForVoicemail(context, voicemailId),
            PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
        )
        val text = transcript?.takeIf { it.isNotBlank() } ?: "New voicemail"
        val notification = NotificationCompat.Builder(context, CHANNEL_VOICEMAIL)
            .setSmallIcon(R.drawable.ic_stat_ring)
            .setColor(accent)
            .setContentTitle("Voicemail from ${from.label}")
            .setContentText(text)
            .setStyle(NotificationCompat.BigTextStyle().bigText(text))
            .setContentIntent(open)
            .setCategory(NotificationCompat.CATEGORY_MESSAGE)
            .setAutoCancel(true)
            .setOnlyAlertOnce(true)
            .build()
        runCatching {
            @Suppress("MissingPermission")
            NotificationManagerCompat.from(context).notify("voicemail:$voicemailId".hashCode(), notification)
        }.onFailure { Log.w(TAG, "Could not post the voicemail notification", it) }
    }

    fun cancelVoicemail(context: Context, voicemailId: String) {
        NotificationManagerCompat.from(context).cancel("voicemail:$voicemailId".hashCode())
    }

    fun canNotify(context: Context): Boolean =
        NotificationManagerCompat.from(context).areNotificationsEnabled() &&
            (android.os.Build.VERSION.SDK_INT < 33 ||
                ContextCompat.checkSelfPermission(context, Manifest.permission.POST_NOTIFICATIONS) == PackageManager.PERMISSION_GRANTED)

    /** Whether incoming calls can take over the screen (Android 14+ can revoke this). */
    fun canUseFullScreenIntent(context: Context): Boolean =
        android.os.Build.VERSION.SDK_INT < 34 || context.getSystemService(NotificationManager::class.java).canUseFullScreenIntent()

    private fun incomingId(ringid: String) = "incoming:$ringid".hashCode()

    private fun callScreenIntent(context: Context, ringid: String, action: String?, request: Int): PendingIntent {
        val intent = Intent(context, CallActivity::class.java)
            .setAction(action ?: CallActivity.ACTION_SHOW)
            .putExtra(CallActivity.EXTRA_RINGID, ringid)
            .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_NO_USER_ACTION)
        return PendingIntent.getActivity(
            context, ringid.hashCode() * 31 + request, intent,
            PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
        )
    }

    private fun person(context: Context, account: Account): Person =
        Person.Builder()
            .setName(account.label)
            .setKey(account.uuid)
            .setImportant(true)
            .setIcon(IconCompat.createWithBitmap(initialAvatar(context, account.label)))
            .build()

    /** The Ring initial avatar: the first letter in the serif face, accent on beige. */
    private fun initialAvatar(context: Context, label: String): Bitmap {
        val size = 192
        val bitmap = createBitmap(size, size)
        val canvas = Canvas(bitmap)
        val fill = Paint(Paint.ANTI_ALIAS_FLAG).apply { color = "#EDE8E0".toColorInt() }
        canvas.drawCircle(size / 2f, size / 2f, size / 2f, fill)
        val typeface = runCatching { ResourcesCompat.getFont(context, R.font.hedvig_letters_serif) }.getOrNull() ?: Typeface.SERIF
        val text = Paint(Paint.ANTI_ALIAS_FLAG).apply {
            color = accent
            textSize = size * 0.46f
            textAlign = Paint.Align.CENTER
            this.typeface = typeface
        }
        val letter = label.trim().removePrefix("si:").removePrefix("c:").firstOrNull()?.uppercase() ?: "?"
        val y = size / 2f - (text.descent() + text.ascent()) / 2f
        canvas.drawText(letter, size / 2f, y, text)
        return bitmap
    }
}
