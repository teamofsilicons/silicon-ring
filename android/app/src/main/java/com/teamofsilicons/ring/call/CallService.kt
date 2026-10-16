package com.teamofsilicons.ring.call

import android.Manifest
import android.app.Notification
import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.content.pm.ServiceInfo
import android.os.Build
import android.os.IBinder
import android.util.Log
import androidx.core.app.NotificationManagerCompat
import androidx.core.app.ServiceCompat
import androidx.core.content.ContextCompat
import com.teamofsilicons.ring.ring

/**
 * The phone-call foreground service that keeps Ring alive while a call rings or is live, and
 * carries the CallStyle notification. Call state itself lives in [CallController].
 */
class CallService : Service() {

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onCreate() {
        super.onCreate()
        instance = this
        requested = false
    }

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        val graph = ring
        val ui = graph.calls.primary.value
        if (ui != null) {
            promote(CallNotifications.forCall(this, ui), wantsMic = ui.joined)
            // Sticky: if the process dies mid-call, the system restarts this service (see below).
            return START_STICKY
        }
        if (intent == null && graph.sessions.current != null) {
            // The system restarted this sticky service after Ring's process died with a call on
            // it. Stay in the foreground while the controller asks the server whether this device
            // still carries the call; it rejoins it, or the service stops when there is none.
            promote(CallNotifications.resuming(this), wantsMic = false)
            graph.calls.resumeAfterRestart()
            return START_STICKY
        }
        // The call ended before the service came up, but startForegroundService() still
        // obliges it to enter the foreground once; leaving immediately avoids that crash.
        runCatching {
            ServiceCompat.startForeground(this, CallNotifications.SERVICE_ID, CallNotifications.idle(this), phoneCallType())
        }
        ServiceCompat.stopForeground(this, ServiceCompat.STOP_FOREGROUND_REMOVE)
        stopSelf()
        return START_NOT_STICKY
    }

    private fun phoneCallType(): Int = if (Build.VERSION.SDK_INT >= 29) ServiceInfo.FOREGROUND_SERVICE_TYPE_PHONE_CALL else 0

    override fun onDestroy() {
        if (instance === this) instance = null
        requested = false
        lastKey = null
        super.onDestroy()
    }

    private fun promote(notification: Notification, wantsMic: Boolean) {
        val micGranted = ContextCompat.checkSelfPermission(this, Manifest.permission.RECORD_AUDIO) == PackageManager.PERMISSION_GRANTED
        val phone = phoneCallType()
        val mic = if (Build.VERSION.SDK_INT >= 30 && wantsMic && micGranted) ServiceInfo.FOREGROUND_SERVICE_TYPE_MICROPHONE else 0
        try {
            ServiceCompat.startForeground(this, CallNotifications.SERVICE_ID, notification, phone or mic)
        } catch (e: Exception) {
            // The microphone type needs the app in the foreground; Telecom's phone-call type is enough.
            Log.i(TAG, "Foreground with microphone refused; continuing as a phone call", e)
            runCatching { ServiceCompat.startForeground(this, CallNotifications.SERVICE_ID, notification, phone) }
                .onFailure { Log.w(TAG, "Could not enter the foreground", it) }
        }
    }

    companion object {
        private const val TAG = "RingCallService"

        @Volatile
        private var instance: CallService? = null
        private var lastKey: Any? = null
        // startForegroundService() was accepted and onCreate has not run yet.
        private var requested = false

        /** Starts the service, or updates its notification when something visible changed. */
        fun sync(context: Context, ui: CallUi?) {
            ui ?: return
            val key = listOf(ui.ringid, ui.phase, ui.peer, ui.silenced, ui.muted, ui.answeredAt, ui.alreadyOn, ui.endedText)
            if (key == lastKey) return
            lastKey = key
            // An incoming call's notification is posted directly as well as through startForeground:
            // on Android 14+ Telecom withdraws the app's call privileges when no CallStyle
            // notification appears within a few seconds of adding the call, and the service can be
            // slow to start or (without an FCM or Telecom exemption) not allowed to.
            if (instance != null || ui.phase == CallPhase.Incoming) notify(context, CallNotifications.forCall(context, ui))
            if (instance == null && !requested) {
                try {
                    ContextCompat.startForegroundService(context, Intent(context, CallService::class.java))
                    requested = true
                } catch (e: Exception) {
                    Log.w(TAG, "Could not start the call service; retrying on the next change", e)
                }
            }
        }

        private fun notify(context: Context, notification: Notification) {
            if (!CallNotifications.canNotify(context)) return
            runCatching {
                @Suppress("MissingPermission")
                NotificationManagerCompat.from(context).notify(CallNotifications.SERVICE_ID, notification)
            }.onFailure { Log.w(TAG, "Could not post the call notification", it) }
        }

        /** Re-enters the foreground so the microphone type applies after answering. */
        fun refresh(context: Context) {
            val running = instance ?: return
            val ui = running.ring.calls.primary.value ?: return
            running.promote(CallNotifications.forCall(context, ui), wantsMic = ui.joined)
        }

        fun stop(context: Context) {
            if (lastKey == null && instance == null) return
            lastKey = null
            requested = false
            NotificationManagerCompat.from(context).cancel(CallNotifications.SERVICE_ID)
            val running = instance ?: return
            instance = null
            ServiceCompat.stopForeground(running, ServiceCompat.STOP_FOREGROUND_REMOVE)
            running.stopSelf()
        }
    }
}
