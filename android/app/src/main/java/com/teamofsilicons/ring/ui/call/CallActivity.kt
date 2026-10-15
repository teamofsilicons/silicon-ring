package com.teamofsilicons.ring.ui.call

import android.content.Intent
import android.os.Build
import android.os.Bundle
import android.os.PowerManager
import android.view.WindowManager
import androidx.activity.ComponentActivity
import androidx.activity.SystemBarStyle
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.core.telecom.CallEndpointCompat
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import com.teamofsilicons.ring.call.CallPhase
import com.teamofsilicons.ring.call.CallUi
import com.teamofsilicons.ring.ring
import com.teamofsilicons.ring.ui.rememberCallPermissions
import com.teamofsilicons.ring.ui.theme.RingTheme

/**
 * The incoming, outgoing and in-call screen. It shows over the lock screen and turns the screen
 * on, so a call can be answered and carried on without unlocking.
 */
class CallActivity : ComponentActivity() {

    private var pendingAnswer by mutableStateOf<String?>(null)
    private var proximity: PowerManager.WakeLock? = null

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        showOverLockScreen()
        val dark = SystemBarStyle.dark(android.graphics.Color.TRANSPARENT)
        enableEdgeToEdge(statusBarStyle = dark, navigationBarStyle = dark)
        val graph = ring
        setContent {
            val appearance by graph.prefs.appearance.collectAsStateWithLifecycle()
            val primary by graph.calls.primary.collectAsStateWithLifecycle()
            val permissions = rememberCallPermissions()

            LaunchedEffect(primary == null) { if (primary == null) finishAndRemoveTask() }
            LaunchedEffect(primary?.phase, primary?.route) { primary?.let(::updateProximity) ?: releaseProximity() }
            LaunchedEffect(pendingAnswer) {
                val ringid = pendingAnswer ?: return@LaunchedEffect
                pendingAnswer = null
                permissions.withMicrophone { granted ->
                    if (granted) graph.calls.accept(ringid) else graph.calls.showNotice(ringid, "Allow the microphone in Settings to answer calls.")
                }
            }

            val ui = primary ?: return@setContent
            RingTheme(appearance, forceDark = ui.phase == CallPhase.Incoming) {
                CallScreen(
                    ui = ui,
                    graph = graph,
                    onAnswer = { pendingAnswer = ui.ringid },
                    onMinimize = { moveTaskToBack(true) },
                )
            }
        }
        handle(intent)
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        setIntent(intent)
        handle(intent)
    }

    override fun onDestroy() {
        releaseProximity()
        super.onDestroy()
    }

    private fun handle(intent: Intent?) {
        if (intent?.action == ACTION_ANSWER) {
            pendingAnswer = intent.getStringExtra(EXTRA_RINGID) ?: ring.calls.primary.value?.ringid
            // The CallStyle notification goes away once the call is answered.
            intent.action = ACTION_SHOW
        }
    }

    private fun showOverLockScreen() {
        if (Build.VERSION.SDK_INT >= 27) {
            setShowWhenLocked(true)
            setTurnScreenOn(true)
        } else {
            @Suppress("DEPRECATION")
            window.addFlags(WindowManager.LayoutParams.FLAG_SHOW_WHEN_LOCKED or WindowManager.LayoutParams.FLAG_TURN_SCREEN_ON)
        }
        window.addFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
    }

    /** Turns the screen off at the ear while the call plays through the earpiece. */
    private fun updateProximity(ui: CallUi) {
        val atEar = ui.joined && ui.phase != CallPhase.Ended &&
            (ui.route?.type ?: CallEndpointCompat.TYPE_EARPIECE) == CallEndpointCompat.TYPE_EARPIECE
        if (!atEar) return releaseProximity()
        if (proximity?.isHeld == true) return
        val power = getSystemService(PowerManager::class.java)
        if (!power.isWakeLockLevelSupported(PowerManager.PROXIMITY_SCREEN_OFF_WAKE_LOCK)) return
        proximity = power.newWakeLock(PowerManager.PROXIMITY_SCREEN_OFF_WAKE_LOCK, "ring:proximity").also { it.acquire(4 * 60 * 60 * 1000L) }
    }

    private fun releaseProximity() {
        proximity?.let { if (it.isHeld) it.release(PowerManager.RELEASE_FLAG_WAIT_FOR_NO_PROXIMITY) }
        proximity = null
    }

    companion object {
        const val ACTION_SHOW = "com.teamofsilicons.ring.action.SHOW_CALL"
        const val ACTION_ANSWER = "com.teamofsilicons.ring.action.ANSWER"
        const val EXTRA_RINGID = "ringid"
    }
}
