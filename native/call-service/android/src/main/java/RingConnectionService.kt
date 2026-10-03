package com.teamofsilicons.callservice

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.pm.ServiceInfo
import android.net.Uri
import android.os.Build
import android.os.IBinder
import android.telecom.Connection
import android.telecom.ConnectionRequest
import android.telecom.ConnectionService
import android.telecom.DisconnectCause
import android.telecom.PhoneAccountHandle
import android.telecom.TelecomManager
import androidx.core.app.NotificationCompat
import androidx.core.content.ContextCompat
import com.google.firebase.messaging.FirebaseMessagingService
import com.google.firebase.messaging.RemoteMessage

class RingConnectionService : ConnectionService() {
    companion object {
        internal val connections = mutableMapOf<String, RingConnection>()
        internal val incoming = mutableMapOf<String, Pair<String, String>>()
        fun end(ringid: String) { connections.remove(ringid)?.let { it.setDisconnected(DisconnectCause(DisconnectCause.REMOTE)); it.destroy() }; incoming.remove(ringid) }
    }
    override fun onCreateIncomingConnection(manager: PhoneAccountHandle?, request: ConnectionRequest): Connection {
        val ring = request.extras?.getString("ringid") ?: return Connection.createFailedConnection(DisconnectCause(DisconnectCause.ERROR))
        val info = incoming[ring] ?: Pair(request.extras?.getString("caller") ?: "Ring", request.extras?.getString("display_name") ?: "Incoming Ring call")
        val connection = RingConnection(applicationContext, ring, info.second)
        connection.connectionProperties = Connection.PROPERTY_SELF_MANAGED
        connection.connectionCapabilities = Connection.CAPABILITY_MUTE
        connection.setAddress(Uri.fromParts("sip", info.first, null), TelecomManager.PRESENTATION_ALLOWED)
        connection.setCallerDisplayName(info.second, TelecomManager.PRESENTATION_ALLOWED)
        connection.setInitializing(); connection.setRinging(); connections[ring] = connection
        return connection
    }
    override fun onCreateIncomingConnectionFailed(manager: PhoneAccountHandle?, request: ConnectionRequest) {
        request.extras?.getString("ringid")?.let { ring -> RingRuntime.get(this).end(ring, true) }
    }
}

class RingConnection(private val context: Context, private val ringid: String, private val name: String) : Connection() {
    override fun onShowIncomingCallUi() { RingCallService.show(context, ringid, name, true) }
    override fun onAnswer() { RingRuntime.get(context).answer(ringid) { _, error -> if (error == null) setActive() else { setDisconnected(DisconnectCause(DisconnectCause.ERROR, error)); destroy() } } }
    override fun onAnswer(videoState: Int) = onAnswer()
    override fun onReject() { RingRuntime.get(context).end(ringid, true) }
    override fun onDisconnect() { RingRuntime.get(context).end(ringid, false) }
    override fun onAbort() { RingRuntime.get(context).end(ringid, true) }
    override fun onCallAudioStateChanged(state: android.telecom.CallAudioState) { RingRuntime.get(context).mute(state.isMuted) { _, _ -> } }
}

class RingCallService : Service() {
    private var ringtone: android.media.Ringtone? = null
    override fun onDestroy() { ringtone?.stop(); ringtone = null; super.onDestroy() }
    companion object {
        private const val CHANNEL = "ring-calls"
        fun show(context: Context, ringid: String, title: String, incoming: Boolean) {
            val intent = Intent(context, RingCallService::class.java).putExtra("ringid", ringid).putExtra("title", title).putExtra("incoming", incoming)
            try { ContextCompat.startForegroundService(context, intent) } catch (_: IllegalStateException) { /* Android may reject non-FCM background starts; next foreground launch restores calls. */ } catch (_: SecurityException) { /* Missing call/notification permissions are requested on the next foreground launch. */ }
        }
    }
    override fun onBind(intent: Intent?): IBinder? = null
    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        val ring = intent?.getStringExtra("ringid") ?: return START_NOT_STICKY
        val runtime = RingRuntime.get(this)
        when (intent.action) {
            "answer" -> { runtime.answer(ring); return START_NOT_STICKY }
            "decline" -> { runtime.end(ring, true); return START_NOT_STICKY }
            "end" -> { runtime.end(ring, false); return START_NOT_STICKY }
        }
        val incoming = intent.getBooleanExtra("incoming", false)
        val title = intent.getStringExtra("title") ?: "Silicon Ring"
        if (incoming && ringtone == null) { ringtone = android.media.RingtoneManager.getRingtone(this, android.media.RingtoneManager.getDefaultUri(android.media.RingtoneManager.TYPE_RINGTONE)); if (Build.VERSION.SDK_INT >= 28) ringtone?.isLooping = true; ringtone?.play() }
        else if (!incoming) { ringtone?.stop(); ringtone = null }
        getSystemService(NotificationManager::class.java).createNotificationChannel(NotificationChannel(CHANNEL, "Ring calls", NotificationManager.IMPORTANCE_HIGH).apply { description = "Incoming and ongoing Ring calls"; setSound(null, null) })
        val launch = packageManager.getLaunchIntentForPackage(packageName) ?: Intent()
        launch.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_SINGLE_TOP).putExtra("ringid", ring)
        val open = PendingIntent.getActivity(this, ring.hashCode(), launch, PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE)
        fun action(name: String) = PendingIntent.getService(this, (ring + name).hashCode(), Intent(this, RingCallService::class.java).setAction(name).putExtra("ringid", ring), PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE)
        val notification = NotificationCompat.Builder(this, CHANNEL).setSmallIcon(android.R.drawable.sym_action_call).setContentTitle(title)
            .setContentText(if (incoming) "Incoming Ring call" else "Tap to return to your call")
            .setCategory(NotificationCompat.CATEGORY_CALL).setPriority(NotificationCompat.PRIORITY_MAX).setOngoing(true).setVisibility(NotificationCompat.VISIBILITY_PRIVATE).setContentIntent(open)
        if (incoming) notification.setFullScreenIntent(open, true).addAction(android.R.drawable.sym_action_call, "Answer", action("answer")).addAction(android.R.drawable.ic_menu_close_clear_cancel, "Decline", action("decline"))
        else notification.addAction(android.R.drawable.ic_menu_close_clear_cancel, "End call", action("end"))
        if (Build.VERSION.SDK_INT >= 29) {
            var types = ServiceInfo.FOREGROUND_SERVICE_TYPE_PHONE_CALL
            if (!incoming && ContextCompat.checkSelfPermission(this, android.Manifest.permission.RECORD_AUDIO) == android.content.pm.PackageManager.PERMISSION_GRANTED) types = types or ServiceInfo.FOREGROUND_SERVICE_TYPE_MICROPHONE
            startForeground(7100, notification.build(), types)
        } else startForeground(7100, notification.build())
        return START_NOT_STICKY
    }
}

class RingMessagingService : FirebaseMessagingService() {
    override fun onNewToken(token: String) { RingRuntime.get(this).registerPush(token) }
    override fun onMessageReceived(message: RemoteMessage) {
        val data = message.data
        val ring = data["ringid"] ?: return
        RingRuntime.get(this).incoming(ring, data["caller"] ?: "Ring", data["display_name"] ?: "Incoming Ring call")
    }
}
