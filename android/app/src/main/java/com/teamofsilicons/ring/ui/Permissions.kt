package com.teamofsilicons.ring.ui

import android.Manifest
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.net.Uri
import android.os.Build
import android.provider.Settings
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.runtime.Composable
import androidx.compose.runtime.Stable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.platform.LocalContext
import androidx.core.content.ContextCompat
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.compose.LifecycleEventEffect
import com.teamofsilicons.ring.call.CallNotifications

/** Runtime permissions Ring needs for calls, re-checked whenever the screen resumes. */
@Stable
class CallPermissions(private val context: Context) {
    var microphone by mutableStateOf(false)
        private set
    var notifications by mutableStateOf(false)
        private set
    var fullScreen by mutableStateOf(true)
        private set

    internal var micLauncher: ((String) -> Unit)? = null
    internal var notificationLauncher: ((String) -> Unit)? = null
    internal var pendingMic: ((Boolean) -> Unit)? = null

    val allGranted: Boolean get() = microphone && notifications && fullScreen

    fun refresh() {
        microphone = granted(Manifest.permission.RECORD_AUDIO)
        notifications = (Build.VERSION.SDK_INT < 33 || granted(Manifest.permission.POST_NOTIFICATIONS)) &&
            androidx.core.app.NotificationManagerCompat.from(context).areNotificationsEnabled()
        fullScreen = CallNotifications.canUseFullScreenIntent(context)
    }

    /** Runs [then] with whether the microphone is available, asking first when needed. */
    fun withMicrophone(then: (Boolean) -> Unit) {
        if (microphone) return then(true)
        pendingMic = then
        micLauncher?.invoke(Manifest.permission.RECORD_AUDIO) ?: then(false)
    }

    fun requestNotifications() {
        if (Build.VERSION.SDK_INT >= 33 && !granted(Manifest.permission.POST_NOTIFICATIONS)) {
            notificationLauncher?.invoke(Manifest.permission.POST_NOTIFICATIONS)
        } else {
            openAppNotificationSettings()
        }
    }

    fun openFullScreenSettings() {
        val intent = if (Build.VERSION.SDK_INT >= 34) {
            Intent(Settings.ACTION_MANAGE_APP_USE_FULL_SCREEN_INTENT, Uri.parse("package:${context.packageName}"))
        } else {
            Intent(Settings.ACTION_APP_NOTIFICATION_SETTINGS).putExtra(Settings.EXTRA_APP_PACKAGE, context.packageName)
        }
        runCatching { context.startActivity(intent.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)) }
            .onFailure { openAppSettings() }
    }

    fun openAppNotificationSettings() {
        val intent = Intent(Settings.ACTION_APP_NOTIFICATION_SETTINGS).putExtra(Settings.EXTRA_APP_PACKAGE, context.packageName)
        runCatching { context.startActivity(intent.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)) }.onFailure { openAppSettings() }
    }

    fun openAppSettings() {
        val intent = Intent(Settings.ACTION_APPLICATION_DETAILS_SETTINGS, Uri.parse("package:${context.packageName}"))
        runCatching { context.startActivity(intent.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)) }
    }

    private fun granted(permission: String) =
        ContextCompat.checkSelfPermission(context, permission) == PackageManager.PERMISSION_GRANTED
}

@Composable
fun rememberCallPermissions(): CallPermissions {
    val context = LocalContext.current
    val permissions = remember { CallPermissions(context).also { it.refresh() } }
    val mic = rememberLauncherForActivityResult(ActivityResultContracts.RequestPermission()) { granted ->
        permissions.refresh()
        permissions.pendingMic?.invoke(granted)
        permissions.pendingMic = null
    }
    val notifications = rememberLauncherForActivityResult(ActivityResultContracts.RequestPermission()) {
        permissions.refresh()
    }
    permissions.micLauncher = { mic.launch(it) }
    permissions.notificationLauncher = { notifications.launch(it) }
    LifecycleEventEffect(Lifecycle.Event.ON_RESUME) { permissions.refresh() }
    return permissions
}
