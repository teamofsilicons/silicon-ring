package com.teamofsilicons.ring.debug

import android.os.Bundle
import android.util.Log
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.runtime.mutableStateOf
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.lifecycle.lifecycleScope
import com.teamofsilicons.ring.call.CallMedia
import com.teamofsilicons.ring.call.CallPhase
import com.teamofsilicons.ring.call.CallUi
import com.teamofsilicons.ring.call.TranscriptLine
import com.teamofsilicons.ring.data.Account
import com.teamofsilicons.ring.data.Appearance
import com.teamofsilicons.ring.data.Call
import com.teamofsilicons.ring.data.Invitation
import com.teamofsilicons.ring.data.MediaGrant
import com.teamofsilicons.ring.data.Participant
import com.teamofsilicons.ring.data.Voicemail
import com.teamofsilicons.ring.ring
import com.teamofsilicons.ring.ui.Navigator
import com.teamofsilicons.ring.ui.call.ActiveCallScreen
import com.teamofsilicons.ring.ui.call.IncomingCallScreen
import com.teamofsilicons.ring.ui.components.PixelLabel
import com.teamofsilicons.ring.ui.home.HomeScreen
import com.teamofsilicons.ring.ui.theme.Ring
import com.teamofsilicons.ring.ui.theme.RingTheme
import java.time.Instant
import java.time.temporal.ChronoUnit
import kotlinx.coroutines.launch

/**
 * Debug-only design catalog. Renders one screen with sample data so it can be reviewed on a
 * device or emulator without a live call:
 *
 *     adb shell am start -n com.teamofsilicons.ring/.debug.DesignCatalogActivity \
 *         --es screen incoming --ez dark false
 *
 * Screens: incoming, incoming-reasons, calling, ringing, active, voicemail, ended, recents, voicemails.
 * `simulate-incoming` instead rings a local call through Telecom and the call service.
 * `media-test` (with `--es url ws://… --es token <LiveKit JWT>`) joins a LiveKit room with the
 * app's call media stack and publishes the microphone, to check media against a dev SFU.
 */
class DesignCatalogActivity : ComponentActivity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        enableEdgeToEdge()
        val screen = intent.getStringExtra("screen") ?: "incoming"
        val dark = intent.getBooleanExtra("dark", false)
        val graph = ring
        if (screen == "simulate-incoming") {
            // Rings through the real path (Telecom, call service, notification) without a server.
            graph.calls.simulateIncomingForDebug(scout)
            finish()
            return
        }
        if (screen == "media-test") {
            mediaTest(intent.getStringExtra("url").orEmpty(), intent.getStringExtra("token").orEmpty())
            return
        }
        if (screen == "recents" || screen == "voicemails") seedLists()
        setContent {
            RingTheme(if (dark) Appearance.Dark else Appearance.Light, forceDark = screen.startsWith("incoming")) {
                Box(Modifier.fillMaxSize().background(Ring.colors.background)) {
                    when (screen) {
                        "incoming" -> IncomingCallScreen(sample(CallPhase.Incoming), {}, {}, {})
                        "incoming-reasons" -> IncomingCallScreen(sample(CallPhase.Incoming), {}, {}, {}, startWithReasons = true)
                        "calling" -> ActiveCallScreen(sample(CallPhase.Calling), graph) {}
                        "ringing" -> ActiveCallScreen(sample(CallPhase.Ringing), graph) {}
                        "active" -> ActiveCallScreen(sample(CallPhase.Active), graph) {}
                        "voicemail" -> ActiveCallScreen(sample(CallPhase.Voicemail).copy(notice = "DECLINED · In a meeting"), graph) {}
                        "ended" -> ActiveCallScreen(sample(CallPhase.Ended).copy(endedText = "CALL ENDED"), graph) {}
                        else -> HomeScreen(graph, Navigator())
                    }
                }
            }
        }
    }

    private var media: CallMedia? = null

    private fun mediaTest(url: String, token: String) {
        val status = mutableStateOf("CONNECTING")
        val media = CallMedia(applicationContext, lifecycleScope) { reason -> status.value = "LOST · $reason" }
        this.media = media
        lifecycleScope.launch {
            val ok = media.connect(MediaGrant(url, token, "media-test", "media-test"), startMuted = false, ownAudioMode = true)
            Log.i("RingMediaTest", if (ok) "connected as ${media.identity}" else "connection failed")
            status.value = if (ok) "CONNECTED" else "FAILED"
            media.speakers.collect { Log.i("RingMediaTest", "active speakers: $it") }
        }
        setContent {
            RingTheme(Appearance.Light) {
                Box(Modifier.fillMaxSize().background(Ring.colors.background), contentAlignment = Alignment.Center) {
                    PixelLabel("Media test · ${status.value}")
                }
            }
        }
    }

    override fun onDestroy() {
        media?.disconnect()
        super.onDestroy()
    }

    private val me = Account("00000000-0000-4000-8000-000000000001", "c:shubham", "carbon", "Shubham")
    private val alice = Account("00000000-0000-4000-8000-000000000002", "c:alice", "carbon", "Alice Fernandes")
    private val scout = Account("00000000-0000-4000-8000-000000000003", "si:scout", "silicon", "Scout")
    private val tos = Account("00000000-0000-4000-8000-000000000004", "si:tos", "silicon", null)

    private fun sample(phase: CallPhase): CallUi {
        val now = Instant.now()
        val call = Call(
            ringid = "ring_sample",
            state = "active",
            createdAt = now.minusSeconds(200).toString(),
            answeredAt = now.minusSeconds(187).toString(),
            createdBy = scout,
            direction = "incoming",
            participants = listOf(
                Participant(scout, "joined", mediaIdentity = "rep:${scout.uuid}"),
                Participant(me, "joined", mediaIdentity = "carbon:${me.uuid}:dev_1"),
            ),
        )
        return CallUi(
            ringid = "ring_sample",
            incoming = true,
            phase = phase,
            call = call,
            peer = scout,
            others = listOf(scout),
            alreadyOn = if (phase == CallPhase.Incoming) listOf(alice) else emptyList(),
            invitationId = "inv_sample",
            silenced = false,
            muted = phase == CallPhase.Voicemail,
            speaking = setOf(scout.uuid),
            transcript = listOf(
                TranscriptLine("1", scout, "Hi Shubham, it's Scout. I looked into the launch dates you asked about.", true, false),
                TranscriptLine("2", me, "Great, what did you find?", true, false),
                TranscriptLine("3", null, "Alice joined", true, true),
                TranscriptLine("4", scout, "The venue is free on the 24th, and the press list is ready.", true, false),
                TranscriptLine("5", me, "Let's lock the 24th then and send", false, false),
            ),
            answeredAt = now.minusSeconds(187),
            route = null,
            routes = emptyList(),
            working = false,
            notice = null,
            endedText = null,
        )
    }

    private fun seedLists() {
        val repo = ring.repo
        val now = Instant.now()
        fun call(id: String, by: Account, to: Account, state: String, minutesAgo: Long, answered: Boolean, invitationState: String, reason: String? = null) = Call(
            ringid = id,
            state = "ended",
            createdAt = now.minus(minutesAgo, ChronoUnit.MINUTES).toString(),
            answeredAt = if (answered) now.minus(minutesAgo, ChronoUnit.MINUTES).plusSeconds(4).toString() else null,
            endedAt = now.minus(minutesAgo, ChronoUnit.MINUTES).plusSeconds(if (answered) 196 else 60).toString(),
            endReason = state,
            createdBy = by,
            direction = if (by == me) "outgoing" else "incoming",
            invitations = listOf(Invitation("inv_$id", id, by, to, invitationState, reason)),
        )
        listOf(
            call("ring_1", scout, me, "completed", 12, true, "accepted"),
            call("ring_2", alice, me, "missed", 95, false, "missed"),
            call("ring_3", me, tos, "declined", 60 * 26, false, "declined", "In a meeting"),
            call("ring_4", me, alice, "completed", 60 * 50, true, "accepted"),
        ).forEach(repo::upsertCall)
        listOf(
            Voicemail("vm_1", "ring_2", alice, me, "missed", null, now.minus(95, ChronoUnit.MINUTES).toString(), 8200, "ready", "Hey, call me back when you're free. It's about Friday.", "ready", false),
            Voicemail("vm_2", "ring_5", scout, me, "busy", null, now.minus(3, ChronoUnit.DAYS).toString(), 5400, "ready", "The weather report you asked for is ready.", "ready", true),
        ).forEach(repo::upsertVoicemail)
    }
}
