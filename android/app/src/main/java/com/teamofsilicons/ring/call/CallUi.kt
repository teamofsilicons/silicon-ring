package com.teamofsilicons.ring.call

import androidx.core.telecom.CallEndpointCompat
import com.teamofsilicons.ring.data.Account
import com.teamofsilicons.ring.data.Call
import java.time.Instant

enum class CallPhase {
    /** Ringing on this device, waiting for an answer. */
    Incoming,
    /** Placing an outgoing call: creating it and joining the room. */
    Calling,
    /** The callee's devices are ringing; ringback plays. */
    Ringing,
    /** Answered here, joining the media. */
    Connecting,
    Active,
    /** The callee did not take the call; the caller is leaving a message after the beep. */
    Voicemail,
    Ended,
}

data class TranscriptLine(
    val key: String,
    val speaker: Account?,
    val text: String,
    val final: Boolean,
    val system: Boolean,
)

data class AudioRoute(val endpoint: CallEndpointCompat) {
    val type: Int get() = endpoint.type
    val name: String get() = endpoint.name.toString()
}

/** An immutable snapshot of one call on this device, for the UI and notifications. */
data class CallUi(
    val ringid: String,
    val incoming: Boolean,
    val phase: CallPhase,
    val call: Call?,
    val peer: Account,
    /** Everyone else on or invited to the call, peer first. */
    val others: List<Account>,
    /** People already on the call when it rings here ("WITH ALICE, BOB"). */
    val alreadyOn: List<Account>,
    val invitationId: String?,
    val silenced: Boolean,
    val muted: Boolean,
    val speaking: Set<String>,
    val transcript: List<TranscriptLine>,
    val answeredAt: Instant?,
    val route: AudioRoute?,
    val routes: List<AudioRoute>,
    val working: Boolean,
    val notice: String?,
    val endedText: String?,
) {
    val live: Boolean get() = phase != CallPhase.Ended
    val joined: Boolean get() = phase in setOf(CallPhase.Calling, CallPhase.Ringing, CallPhase.Connecting, CallPhase.Active, CallPhase.Voicemail)
}
