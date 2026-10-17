package com.teamofsilicons.ring.call

import android.content.Context
import android.content.Intent
import android.os.SystemClock
import android.telecom.DisconnectCause
import android.util.Log
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.ProcessLifecycleOwner
import com.teamofsilicons.ring.data.Account
import com.teamofsilicons.ring.data.ApiException
import com.teamofsilicons.ring.data.Call
import com.teamofsilicons.ring.data.CallState
import com.teamofsilicons.ring.data.DeviceRef
import com.teamofsilicons.ring.data.EntryKind
import com.teamofsilicons.ring.data.Invitation
import com.teamofsilicons.ring.data.InvitationState
import com.teamofsilicons.ring.data.MediaGrant
import com.teamofsilicons.ring.data.Realtime
import com.teamofsilicons.ring.data.RingApi
import com.teamofsilicons.ring.data.RingEvent
import com.teamofsilicons.ring.data.RingRepository
import com.teamofsilicons.ring.data.RingingStoppedReason
import com.teamofsilicons.ring.data.TranscriptEntry
import com.teamofsilicons.ring.data.parseInstant
import com.teamofsilicons.ring.ui.call.CallActivity
import io.livekit.android.events.DisconnectReason
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Job
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.launch
import java.time.Instant
import java.util.TreeMap

/**
 * Every call on this device: incoming rings, the outgoing or active call, Telecom registration,
 * LiveKit media and the notification service. Runs on the main thread.
 */
class CallController(
    private val context: Context,
    private val api: RingApi,
    private val repo: RingRepository,
    private val realtime: Realtime,
    private val scope: CoroutineScope,
) {
    private val runtimes = LinkedHashMap<String, Runtime>()
    private val ringer = Ringer(context)
    private val tones = Tones(context)

    private val _calls = MutableStateFlow<List<CallUi>>(emptyList())
    val calls: StateFlow<List<CallUi>> = _calls.asStateFlow()

    private val _primary = MutableStateFlow<CallUi?>(null)
    /** The call the call screen and the service notification show. */
    val primary: StateFlow<CallUi?> = _primary.asStateFlow()

    private val _elsewhere = MutableStateFlow<List<Call>>(emptyList())
    /** Live calls this account is on from another device (handoff candidates). */
    val elsewhere: StateFlow<List<Call>> = _elsewhere.asStateFlow()

    private val _movedAway = MutableStateFlow<String?>(null)
    /** "Moved to Mac" after this device handed a call off; cleared after a few seconds. */
    val movedAway: StateFlow<String?> = _movedAway.asStateFlow()

    // Invitations answered or declined here and calls left here. Late events and reconnects must
    // not ring or rejoin them again.
    private val settledInvitations = mutableSetOf<String>()
    private val leftCalls = mutableSetOf<String>()
    private val missedNotified = mutableSetOf<String>()

    private val me: String? get() = repo.myUuid
    private val myDevice: String? get() = repo.myDeviceId

    // Events

    fun onEvent(event: RingEvent) {
        when (event) {
            is RingEvent.Ready -> reconcile()
            is RingEvent.CallIncoming -> onIncoming(event.call, event.invitation)
            is RingEvent.CallUpdated -> onCallUpdated(event.call)
            is RingEvent.CallEnded -> onCallUpdated(event.call)
            is RingEvent.RingingStopped -> onRingingStopped(event.ringid, event.invitationId, event.reason, event.from)
            is RingEvent.Handoff -> onHandoff(event.ringid, event.deviceId, event.device)
            is RingEvent.Transcript -> runtimes[event.ringid]?.let {
                it.addEntry(event.entry)
                publish()
            }
            is RingEvent.TranscriptPartial -> runtimes[event.ringid]?.let {
                it.addPartial(event.speaker, event.segmentId, event.text)
                publish()
            }
            is RingEvent.VoicemailReceived -> CallNotifications.voicemail(context, event.voicemail.voicemailId, event.voicemail.from, event.voicemail.transcript)
            else -> Unit
        }
    }

    /** A `call.incoming` push, possibly waking a cold process. */
    fun onIncomingPush(ringid: String, invitationId: String, from: Account, alreadyOn: List<Account>, expiresAt: Instant?) {
        if (repo.session.value == null) return
        if (expiresAt != null && expiresAt.isBefore(Instant.now())) return
        if (runtimes.containsKey(ringid) || invitationId in settledInvitations) return
        val rt = Runtime(ringid, incoming = true, peer = from).apply {
            this.invitationId = invitationId
            this.alreadyOnFromPush = alreadyOn
            phase = CallPhase.Incoming
        }
        startIncoming(rt)
        // Fill in the full call (and make sure it is still ringing) once the network is up.
        scope.launch {
            try {
                onCallUpdated(api.call(ringid))
            } catch (e: ApiException) {
                Log.i(TAG, "Could not load pushed call: ${e.code}")
                if (e.status == 404 || e.status == 403) end(rt, DisconnectCause.MISSED, null)
            }
        }
    }

    fun onPushCancelled(ringid: String, invitationId: String?, reason: String) =
        onRingingStopped(ringid, invitationId.orEmpty(), reason, from = null)

    /** Posts the missed-call notification for [ringid] once, whichever signal arrives first. */
    fun notifyMissed(ringid: String, from: Account) {
        if (!missedNotified.add(ringid)) return
        CallNotifications.missed(context, ringid, from)
    }

    /**
     * Debug builds only: rings a local incoming call that has no server behind it, to exercise
     * Telecom, the call service, the CallStyle notification and the lock-screen UI on a device.
     */
    fun simulateIncomingForDebug(from: Account) {
        check(com.teamofsilicons.ring.BuildConfig.DEBUG) { "Debug builds only" }
        val rt = Runtime("ring_debug_${System.currentTimeMillis()}", incoming = true, peer = from).apply {
            invitationId = "inv_debug"
            phase = CallPhase.Incoming
        }
        startIncoming(rt)
    }

    private fun onIncoming(call: Call, invitation: Invitation) {
        val me = me ?: return
        if (invitation.to.uuid != me || !invitation.ringing || invitation.invitationId in settledInvitations) return
        val expires = parseInstant(invitation.expiresAt)
        if (expires != null && expires.isBefore(Instant.now())) return
        val existing = runtimes[call.ringid]
        if (existing != null) {
            existing.call = call
            existing.invitationId = invitation.invitationId
            existing.silenced = existing.silenced || invitation.silenced
            publish()
            return
        }
        if (repo.currentDevice.value?.ringEnabled == false) return
        val rt = Runtime(call.ringid, incoming = true, peer = invitation.from).apply {
            this.call = call
            invitationId = invitation.invitationId
            silenced = invitation.silenced
            phase = CallPhase.Incoming
        }
        startIncoming(rt)
    }

    private fun startIncoming(rt: Runtime) {
        runtimes[rt.ringid] = rt
        rt.telecom = Telecom.manager(context)?.let { manager ->
            TelecomCall(
                scope, manager, rt.peer.label, rt.peer.id, incoming = true,
                onAnswerRequested = { scope.launch { accept(rt.ringid) } },
                onDisconnectRequested = { scope.launch { hangUp(rt.ringid) } },
            ).also { it.start() }
        }
        rt.observeTelecom()
        publish()
        if (isAppVisible()) openCallScreen()
    }

    private fun onCallUpdated(call: Call) {
        updateElsewhere(call)
        val me = me ?: return
        val rt = runtimes[call.ringid]
        if (rt == null) {
            call.ringingInvitationFor(me)?.let { if (!it.silenced) onIncoming(call, it) }
            return
        }
        rt.call = call
        if (rt.phase == CallPhase.Ended) return publish()

        if (rt.incoming && !rt.accepted) {
            val invitation = call.invitations.firstOrNull { it.invitationId == rt.invitationId } ?: call.latestInvitationFor(me)
            when {
                invitation != null && !invitation.ringing -> {
                    val (cause, text) = endingFor(invitation.state)
                    end(rt, cause, text)
                    if (invitation.state == InvitationState.MISSED || invitation.state == InvitationState.CANCELLED) {
                        notifyMissed(call.ringid, rt.peer)
                    }
                }
                call.state == CallState.ENDED -> end(rt, DisconnectCause.MISSED, "MISSED")
                else -> {
                    rt.invitationId = invitation?.invitationId ?: rt.invitationId
                    rt.silenced = rt.silenced || invitation?.silenced == true
                }
            }
            return publish()
        }

        val mine = call.participant(me)
        when (call.state) {
            CallState.ENDED -> end(rt, DisconnectCause.REMOTE, endedText(call))
            CallState.VOICEMAIL -> {
                if (!rt.incoming && rt.phase != CallPhase.Voicemail) {
                    tones.stopRingback()
                    rt.phase = CallPhase.Voicemail
                    rt.notice = voicemailReason(call)
                    scope.launch { rt.telecom?.setActive() }
                }
            }
            CallState.ACTIVE -> when {
                mine != null && mine.joined && mine.device != null && myDevice != null && mine.device.deviceId != myDevice ->
                    moveAway(rt, mine.device.label)
                mine != null && !mine.joined && rt.phase != CallPhase.Connecting -> end(rt, DisconnectCause.LOCAL, "CALL ENDED")
                rt.phase == CallPhase.Calling || rt.phase == CallPhase.Ringing -> {
                    tones.stopRingback()
                    tones.connected()
                    rt.phase = CallPhase.Active
                    rt.notice = null
                    scope.launch { rt.telecom?.setActive() }
                }
            }
            CallState.RINGING -> if (!rt.incoming && rt.phase == CallPhase.Calling && rt.media?.status?.value == CallMedia.Status.Connected) {
                rt.phase = CallPhase.Ringing
                tones.startRingback()
            }
        }
        publish()
    }

    private fun onRingingStopped(ringid: String, invitationId: String, reason: String, from: Account?) {
        val missed = reason == RingingStoppedReason.MISSED || reason == RingingStoppedReason.CANCELLED
        val rt = runtimes[ringid]
        if (rt == null) {
            // Not ringing here (ringing off on this device, or the process restarted): the event
            // names the caller, so the missed call can still be noted.
            if (missed && from != null && invitationId !in settledInvitations) notifyMissed(ringid, from)
            return
        }
        if (!rt.incoming || rt.accepted || rt.phase != CallPhase.Incoming) return
        if (reason == RingingStoppedReason.SILENCED) {
            rt.silenced = true
            return publish()
        }
        val (cause, text) = when (reason) {
            RingingStoppedReason.ACCEPTED_ELSEWHERE -> DisconnectCause.ANSWERED_ELSEWHERE to "ANSWERED ON ANOTHER DEVICE"
            RingingStoppedReason.DECLINED -> DisconnectCause.REJECTED to "DECLINED"
            else -> DisconnectCause.MISSED to "MISSED"
        }
        end(rt, cause, text)
        if (missed) notifyMissed(ringid, from ?: rt.peer)
        publish()
    }

    private fun onHandoff(ringid: String, deviceId: String, device: DeviceRef?) {
        val rt = runtimes[ringid]
        if (rt != null && deviceId.isNotEmpty() && deviceId != myDevice && rt.phase != CallPhase.Ended && rt.phase != CallPhase.Incoming) {
            val where = device?.label ?: rt.call?.participants?.firstOrNull { it.device?.deviceId == deviceId }?.device?.label
            moveAway(rt, where)
            publish()
        }
        scope.launch {
            runCatching { api.call(ringid) }.onSuccess { call ->
                // A server that leaves the device out of the event: name it from the fresh call.
                val moved = rt?.takeIf { it.phase == CallPhase.Ended && it.endedText == MOVED_SOMEWHERE }
                val label = call.participant(me.orEmpty())?.device?.takeIf { it.deviceId == deviceId }?.label
                if (moved != null && label != null) {
                    moved.endedText = "MOVED TO ${label.uppercase()}"
                    if (_movedAway.value != null) _movedAway.value = "Moved to $label"
                    publish()
                }
                onCallUpdated(call)
            }
        }
    }

    /** Brings local state in line with the server after (re)connecting. */
    fun reconcile() {
        if (me == null) return
        scope.launch { reconcileNow() }
    }

    /** The app came to the foreground: reconcile, and let a live call keep the microphone. */
    fun onAppForeground() {
        reconcile()
        // A call rejoined while Ring was in the background could not claim the microphone
        // foreground-service type; now that Ring is visible it can.
        if (runtimes.values.any { it.joinedHere }) CallService.refresh(context)
    }

    /**
     * The call service was restarted by the system after Ring's process died (a crash, or the
     * system reclaiming memory). If the server says this device still carries a call, rejoin it
     * within the server's 30 s grace; otherwise let the service stop.
     */
    fun resumeAfterRestart() {
        if (recovering || me == null) return
        recovering = true
        scope.launch {
            try {
                // The network can take a moment to come back with the process.
                for (attempt in 0 until 5) {
                    if (reconcileNow()) break
                    delay(2_000L * (attempt + 1))
                }
            } finally {
                recovering = false
                publish()
            }
        }
    }

    /** Returns false when the server could not be reached, so a caller may retry. */
    private suspend fun reconcileNow(): Boolean {
        val me = me ?: return true
        val live = try {
            api.calls(states = "${CallState.RINGING},${CallState.ACTIVE},${CallState.VOICEMAIL}", limit = 50).items
        } catch (e: ApiException) {
            Log.i(TAG, "Reconcile failed: ${e.code}")
            return !e.isNetwork && !e.retryable
        }
        val seen = live.map { it.ringid }.toSet()
        _elsewhere.value = emptyList()
        for (call in live) {
            val invitation = call.ringingInvitationFor(me)
            when {
                invitation != null && runtimes[call.ringid] == null -> onIncoming(call, invitation)
                runtimes[call.ringid] == null && isMineHere(call) && call.ringid !in leftCalls -> rejoin(call)
                else -> onCallUpdated(call)
            }
        }
        runtimes.values.filter { it.ringid !in seen && it.phase != CallPhase.Ended }.forEach { rt ->
            runCatching { api.call(rt.ringid) }
                .onSuccess { onCallUpdated(it) }
                .onFailure { end(rt, DisconnectCause.MISSED, null) }
        }
        publish()
        return true
    }

    // Actions

    /** Calls [ref]. Returns an error message, or null once the call is ringing out. */
    suspend fun placeCall(ref: String): String? {
        if (runtimes.values.any { it.joinedHere }) return "Hang up your current call first."
        val created = try {
            api.createCall(ref)
        } catch (e: ApiException) {
            return when (e.code) {
                "account_not_found" -> "No one uses that id."
                "forbidden" -> e.message
                else -> e.userMessage
            }
        }
        val call = created.call
        val rt = Runtime(call.ringid, incoming = false, peer = created.invitation.to).apply {
            this.call = call
            invitationId = created.invitation.invitationId
            phase = if (call.state == CallState.VOICEMAIL) CallPhase.Voicemail else CallPhase.Calling
            notice = if (call.state == CallState.VOICEMAIL) voicemailReason(call) else null
        }
        runtimes[rt.ringid] = rt
        rt.telecom = Telecom.manager(context)?.let { manager ->
            TelecomCall(
                scope, manager, rt.peer.label, rt.peer.id, incoming = false,
                onAnswerRequested = {},
                onDisconnectRequested = { scope.launch { hangUp(rt.ringid) } },
            ).also { it.start() }
        }
        rt.observeTelecom()
        publish()
        repo.upsertCall(call)
        val grant = created.media
        if (grant == null) {
            end(rt, DisconnectCause.ERROR, "COULD NOT CONNECT")
            return "Ring did not return call audio."
        }
        scope.launch {
            if (rt.phase == CallPhase.Voicemail) rt.telecom?.setActive()
            if (!connectMedia(rt, grant)) {
                if (rt.phase != CallPhase.Ended) {
                    runCatching { api.leave(rt.ringid) }
                    end(rt, DisconnectCause.ERROR, "COULD NOT CONNECT")
                }
            } else if (rt.phase == CallPhase.Calling) {
                if (rt.call?.state == CallState.ACTIVE) {
                    rt.phase = CallPhase.Active
                    rt.telecom?.setActive()
                } else {
                    rt.phase = CallPhase.Ringing
                    tones.startRingback()
                }
            }
            publish()
        }
        return null
    }

    fun accept(ringid: String) {
        val rt = runtimes[ringid] ?: return
        if (!rt.incoming || rt.accepted || rt.phase != CallPhase.Incoming || rt.working) return
        rt.working = true
        rt.phase = CallPhase.Connecting
        rt.notice = null
        publish()
        scope.launch {
            runtimes.values.filter { it !== rt && it.joinedHere }.forEach { hangUp(it.ringid) }
            val result = try {
                api.accept(ringid, rt.invitationId)
            } catch (e: ApiException) {
                rt.working = false
                when (e.code) {
                    "invitation_not_ringing" -> {
                        rt.invitationId?.let { settledInvitations += it }
                        end(rt, DisconnectCause.ANSWERED_ELSEWHERE, "ANSWERED ELSEWHERE")
                    }
                    "invitation_expired", "call_ended", "not_found" -> {
                        rt.invitationId?.let { settledInvitations += it }
                        end(rt, DisconnectCause.MISSED, "MISSED")
                    }
                    else -> {
                        rt.phase = CallPhase.Incoming
                        rt.notice = e.userMessage
                    }
                }
                publish()
                return@launch
            }
            rt.accepted = true
            rt.working = false
            rt.invitationId?.let { settledInvitations += it }
            rt.call = result.call
            repo.upsertCall(result.call)
            if (rt.phase == CallPhase.Ended) {
                // Hung up while the answer was in flight.
                runCatching { api.leave(ringid) }
                return@launch publish()
            }
            rt.telecom?.answer()
            CallService.refresh(context)
            val grant = result.media
            if (grant == null || !connectMedia(rt, grant)) {
                if (rt.phase != CallPhase.Ended) {
                    runCatching { api.leave(ringid) }
                    end(rt, DisconnectCause.ERROR, "COULD NOT CONNECT")
                }
                return@launch publish()
            }
            rt.phase = CallPhase.Active
            tones.connected()
            loadTranscript(rt)
            publish()
        }
    }

    /** Declines a ringing call, with an optional reason (null declines with no reason). */
    fun decline(ringid: String, reason: String?) {
        val rt = runtimes[ringid] ?: return
        if (!rt.incoming || rt.accepted || rt.phase == CallPhase.Ended) return
        rt.invitationId?.let { settledInvitations += it }
        end(rt, DisconnectCause.REJECTED, if (reason.isNullOrBlank()) "DECLINED" else "DECLINED · $reason")
        publish()
        scope.launch {
            runCatching { api.decline(ringid, rt.invitationId, reason?.trim()) }
                .onFailure { Log.i(TAG, "Decline failed: ${(it as? ApiException)?.code}") }
        }
    }

    /** Stops the ringing on every device without declining. */
    fun silence(ringid: String) {
        val rt = runtimes[ringid] ?: return
        if (rt.phase != CallPhase.Incoming) return
        rt.silenced = true
        publish()
        scope.launch { runCatching { api.silence(ringid, rt.invitationId) } }
    }

    fun hangUp(ringid: String) {
        val rt = runtimes[ringid] ?: return
        if (rt.phase == CallPhase.Ended) return
        if (rt.incoming && !rt.accepted) {
            if (rt.phase != CallPhase.Connecting) return decline(ringid, null)
            // Answer in flight: accept() leaves the call when it completes.
            leftCalls += ringid
            end(rt, DisconnectCause.LOCAL, "CALL ENDED")
            return publish()
        }
        val text = if (rt.phase == CallPhase.Voicemail) "MESSAGE SENT" else "CALL ENDED"
        leftCalls += ringid
        end(rt, DisconnectCause.LOCAL, text)
        publish()
        scope.launch {
            for (attempt in 0 until 3) {
                try {
                    repo.upsertCall(api.leave(ringid).call)
                    return@launch
                } catch (e: ApiException) {
                    if (!e.retryable && !e.isNetwork) return@launch
                    delay(1_000L * (attempt + 1))
                }
            }
        }
    }

    fun setMuted(ringid: String, muted: Boolean) {
        val rt = runtimes[ringid] ?: return
        rt.muted = muted
        publish()
        scope.launch { rt.media?.setMuted(muted) }
    }

    fun showNotice(ringid: String, text: String) {
        val rt = runtimes[ringid] ?: return
        rt.notice = text
        publish()
    }

    /**
     * Plays the call through [route]: through Telecom when it holds the call, otherwise through
     * AudioManager. The call screen follows the route Telecom or AudioManager then reports.
     */
    fun setRoute(ringid: String, route: AudioRoute) {
        val rt = runtimes[ringid] ?: return
        val media = rt.media
        if (media != null && media.ownsAudio) {
            if (!media.selectRoute(route.endpoint)) routeFailed(rt, route)
            return publish()
        }
        scope.launch {
            if (rt.telecom?.requestEndpoint(route.endpoint) != true) routeFailed(rt, route)
        }
    }

    /** Says for a moment that [route] could not be used, instead of the button doing nothing. */
    private fun routeFailed(rt: Runtime, route: AudioRoute) {
        Log.w(TAG, "Could not switch the call to ${route.name} (type ${route.type})")
        if (rt.phase == CallPhase.Ended || rt.rejoining) return
        val text = "COULD NOT SWITCH TO ${route.name.uppercase()}"
        rt.notice = text
        publish()
        scope.launch {
            delay(3_000)
            if (rt.notice == text) {
                rt.notice = null
                publish()
            }
        }
    }

    /** Invites [ref] into the call. Returns an error message or null. */
    suspend fun invite(ringid: String, ref: String): String? {
        val rt = runtimes[ringid] ?: return "The call has ended."
        return try {
            val result = api.invite(ringid, ref)
            rt.call = result.call
            repo.upsertCall(result.call)
            publish()
            null
        } catch (e: ApiException) {
            if (e.code == "account_not_found") "No one uses that id." else e.userMessage
        }
    }

    /** Moves a call that is live on another of this account's devices to this one. */
    suspend fun switchHere(ringid: String): String? {
        if (runtimes[ringid]?.joinedHere == true) return null
        if (runtimes.values.any { it.joinedHere }) return "Hang up your current call first."
        val result = try {
            api.media(ringid)
        } catch (e: ApiException) {
            return e.userMessage
        }
        _elsewhere.value = _elsewhere.value.filterNot { it.ringid == ringid }
        startJoined(result.call, result.media)
        return null
    }

    private val rejoinsInFlight = mutableSetOf<String>()

    // True while the call service, restarted after a process death, waits for resumeAfterRestart.
    private var recovering = false

    /**
     * Picks up a call the server says this device carries but this process has no state for: the
     * app was restarted or crashed mid-call. Same path as a handoff (`POST /media`), and it has to
     * land within the server's 30 s grace, so network failures are retried briefly.
     */
    private suspend fun rejoin(call: Call) {
        if (runtimes.values.any { it.joinedHere }) return
        if (!rejoinsInFlight.add(call.ringid)) return
        try {
            for (attempt in 0 until 4) {
                try {
                    val result = api.media(call.ringid)
                    if (runtimes[call.ringid] == null && call.ringid !in leftCalls) {
                        Log.i(TAG, "Rejoining a call this device carried before the app restarted")
                        startJoined(result.call, result.media)
                    }
                    return
                } catch (e: ApiException) {
                    Log.i(TAG, "Rejoin failed: ${e.code}")
                    if (!e.isNetwork && !e.retryable) return
                    delay(1_500L * (attempt + 1))
                }
            }
        } finally {
            rejoinsInFlight -= call.ringid
        }
    }

    private fun startJoined(call: Call, grant: MediaGrant?) {
        val me = me ?: return
        if (runtimes[call.ringid]?.joinedHere == true) return
        val peer = call.others(me).firstOrNull() ?: call.createdBy
        val rt = Runtime(call.ringid, incoming = !call.isOutgoing, peer = peer).apply {
            this.call = call
            accepted = true
            phase = if (call.state == CallState.VOICEMAIL) CallPhase.Voicemail else CallPhase.Connecting
        }
        runtimes[rt.ringid] = rt
        rt.telecom = Telecom.manager(context)?.let { manager ->
            TelecomCall(
                scope, manager, peer.label, peer.id, incoming = false,
                onAnswerRequested = {},
                onDisconnectRequested = { scope.launch { hangUp(rt.ringid) } },
            ).also { it.start() }
        }
        rt.observeTelecom()
        publish()
        // Claims the microphone service type when the call service was already running (it was
        // restarted after a process death and holds a placeholder notification).
        CallService.refresh(context)
        if (isAppVisible()) openCallScreen()
        scope.launch {
            rt.telecom?.setActive()
            if (grant == null || !connectMedia(rt, grant)) {
                if (rt.phase != CallPhase.Ended) end(rt, DisconnectCause.ERROR, "COULD NOT CONNECT")
            } else {
                if (rt.phase == CallPhase.Connecting) rt.phase = CallPhase.Active
                loadTranscript(rt)
            }
            publish()
        }
    }

    // Internals

    private suspend fun connectMedia(rt: Runtime, grant: MediaGrant): Boolean {
        if (rt.phase == CallPhase.Ended) return false
        val media = rt.media ?: CallMedia(context, scope) { reason -> onMediaLost(rt, reason) }.also { rt.media = it }
        rt.observeMedia()
        val ownAudio = rt.telecom == null || !rt.telecom!!.awaitReady()
        val connected = media.connect(grant, rt.muted, ownAudio)
        // The call may have been hung up while the room was connecting.
        if (rt.phase == CallPhase.Ended) {
            media.disconnect()
            return false
        }
        return connected
    }

    /** The room dropped us: rejoin within the server's 30 s grace unless the call moved or ended. */
    private fun onMediaLost(rt: Runtime, reason: DisconnectReason) {
        if (rt.phase == CallPhase.Ended || rt.rejoining) return
        if (reason == DisconnectReason.PARTICIPANT_REMOVED || reason == DisconnectReason.ROOM_DELETED ||
            reason == DisconnectReason.DUPLICATE_IDENTITY || reason == DisconnectReason.ROOM_CLOSED
        ) {
            scope.launch { runCatching { api.call(rt.ringid) }.onSuccess { onCallUpdated(it) } }
            return
        }
        rt.rejoining = true
        rt.notice = "RECONNECTING"
        publish()
        scope.launch {
            repeat(4) { attempt ->
                if (rt.phase == CallPhase.Ended) return@launch
                delay(1_500L * (attempt + 1))
                try {
                    val result = api.media(rt.ringid)
                    rt.call = result.call
                    if (result.media != null && connectMedia(rt, result.media)) {
                        rt.rejoining = false
                        rt.notice = null
                        publish()
                        return@launch
                    }
                } catch (e: ApiException) {
                    if (!e.isNetwork && !e.retryable) {
                        runCatching { api.call(rt.ringid) }.onSuccess { onCallUpdated(it) }
                        rt.rejoining = false
                        if (rt.phase != CallPhase.Ended) end(rt, DisconnectCause.ERROR, "CONNECTION LOST")
                        publish()
                        return@launch
                    }
                }
            }
            rt.rejoining = false
            end(rt, DisconnectCause.ERROR, "CONNECTION LOST")
            publish()
        }
    }

    private suspend fun loadTranscript(rt: Runtime) {
        runCatching { api.fullTranscript(rt.ringid) }.onSuccess { entries ->
            entries.forEach { rt.addEntry(it) }
            publish()
        }
    }

    private var movedAwayClear: Job? = null

    /** The call moved to another of this account's devices; [where] is its name or platform word. */
    private fun moveAway(rt: Runtime, where: String?) {
        val place = where?.takeIf { it.isNotBlank() } ?: "another device"
        end(rt, DisconnectCause.LOCAL, if (where.isNullOrBlank()) MOVED_SOMEWHERE else "MOVED TO ${place.uppercase()}", playTone = false)
        _movedAway.value = "Moved to $place"
        movedAwayClear?.cancel()
        movedAwayClear = scope.launch {
            delay(6_000)
            _movedAway.value = null
        }
    }

    private fun end(rt: Runtime, cause: Int, text: String?, playTone: Boolean = true) {
        if (rt.phase == CallPhase.Ended) return
        val wasConnected = rt.phase == CallPhase.Active || rt.phase == CallPhase.Voicemail
        rt.phase = CallPhase.Ended
        rt.endedText = text
        rt.working = false
        tones.stopRingback()
        rt.media?.disconnect()
        rt.telecom?.disconnect(cause)
        rt.jobs.forEach { it.cancel() }
        if (wasConnected && playTone) tones.ended()
        CallNotifications.cancelIncoming(context, rt.ringid)
        scope.launch {
            delay(if (text == null) 0 else 2_500)
            if (runtimes[rt.ringid] === rt) runtimes.remove(rt.ringid)
            publish()
            scope.launch { runCatching { api.call(rt.ringid) }.onSuccess { repo.upsertCall(it) } }
        }
    }

    private fun updateElsewhere(call: Call) {
        val me = me ?: return
        val mine = call.participant(me)
        val elsewhere = call.state in setOf(CallState.ACTIVE, CallState.VOICEMAIL) &&
            mine != null && mine.joined && mine.device != null && mine.device.deviceId != myDevice &&
            runtimes[call.ringid]?.joinedHere != true
        val rest = _elsewhere.value.filterNot { it.ringid == call.ringid }
        _elsewhere.value = if (elsewhere) rest + call else rest
    }

    private fun isMineHere(call: Call): Boolean {
        val device = myDevice ?: return false
        val mine = call.participant(me ?: return false) ?: return false
        return call.state in setOf(CallState.ACTIVE, CallState.VOICEMAIL) && mine.joined && mine.device?.deviceId == device
    }

    private fun endingFor(state: String): Pair<Int, String> = when (state) {
        InvitationState.ACCEPTED -> DisconnectCause.ANSWERED_ELSEWHERE to "ANSWERED ON ANOTHER DEVICE"
        InvitationState.DECLINED -> DisconnectCause.REJECTED to "DECLINED"
        InvitationState.CANCELLED -> DisconnectCause.MISSED to "CALLER HUNG UP"
        else -> DisconnectCause.MISSED to "MISSED"
    }

    private fun endedText(call: Call): String = when (call.endReason) {
        "declined" -> call.invitations.firstOrNull { it.state == InvitationState.DECLINED }?.reason
            ?.let { "DECLINED · $it" } ?: "DECLINED"
        "missed" -> "NO ANSWER"
        "busy" -> "BUSY"
        "cancelled" -> "CANCELLED"
        "failed" -> "CALL FAILED"
        else -> "CALL ENDED"
    }

    private fun voicemailReason(call: Call): String {
        val invitation = call.invitations.lastOrNull()
        return when (invitation?.state) {
            InvitationState.BUSY -> "BUSY"
            InvitationState.DECLINED -> invitation.reason?.let { "DECLINED · $it" } ?: "DECLINED"
            InvitationState.MISSED -> "NO ANSWER"
            else -> "NO ANSWER"
        }
    }

    private fun isAppVisible(): Boolean =
        ProcessLifecycleOwner.get().lifecycle.currentState.isAtLeast(Lifecycle.State.STARTED)

    fun openCallScreen() {
        val intent = Intent(context, CallActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
        runCatching { context.startActivity(intent) }
    }

    /** Ends everything locally (sign-out). */
    fun shutdown() {
        runtimes.values.toList().forEach { end(it, DisconnectCause.LOCAL, null, playTone = false) }
        runtimes.clear()
        settledInvitations.clear()
        leftCalls.clear()
        missedNotified.clear()
        _elsewhere.value = emptyList()
        publish()
    }

    private fun publish() {
        val snapshots = runtimes.values.map { it.snapshot() }
        _calls.value = snapshots
        val primary = snapshots.firstOrNull { it.joined }
            ?: snapshots.firstOrNull { it.phase == CallPhase.Incoming }
            ?: snapshots.lastOrNull()
        _primary.value = primary

        // The service and its CallStyle notification come first: Telecom and the FCM exemption
        // give only a few seconds, and starting the ringtone can be slow.
        if (snapshots.isEmpty()) {
            // A rejoin after a restart keeps the (restarted) service until it knows the outcome.
            if (!recovering && rejoinsInFlight.isEmpty()) CallService.stop(context)
            if (holdingSocket) realtime.release(HOLD)
            holdingSocket = false
        } else {
            CallService.sync(context, primary)
            if (!holdingSocket) realtime.hold(HOLD)
            holdingSocket = true
        }
        // A second call ringing while on a call gets its own notification, until it becomes the
        // service's call.
        snapshots.filter { it.phase == CallPhase.Incoming && it !== primary }.forEach { CallNotifications.postIncoming(context, it) }
        primary?.takeIf { it.phase == CallPhase.Incoming }?.let { CallNotifications.cancelIncoming(context, it.ringid) }

        val anyJoined = snapshots.any { it.joined }
        val ringingHere = snapshots.filter { it.phase == CallPhase.Incoming && !it.silenced }
        when {
            ringingHere.isEmpty() -> ringer.stop()
            anyJoined -> ringer.callWaiting()
            else -> ringer.ring()
        }
    }

    private var holdingSocket = false

    /** Mutable per-call state; only touched on the main thread. */
    private inner class Runtime(val ringid: String, val incoming: Boolean, var peer: Account) {
        var call: Call? = null
            set(value) {
                field = value
                if (value != null) {
                    val me = me
                    val other = if (incoming) value.invitations.firstOrNull { it.to.uuid == me }?.from ?: value.createdBy
                    else value.invitations.firstOrNull { it.from.uuid == me }?.to
                    if (other != null && other.uuid != me) peer = other
                }
            }
        var phase = CallPhase.Incoming
        var invitationId: String? = null
        var alreadyOnFromPush: List<Account> = emptyList()
        var accepted = false
        var silenced = false
        var muted = false
        var working = false
        var rejoining = false
        var notice: String? = null
        var endedText: String? = null
        var telecom: TelecomCall? = null
        var media: CallMedia? = null
        val entries = TreeMap<Long, TranscriptEntry>()
        // Interim speech by segment id (or "speaker:<uuid>" when a partial has none), oldest
        // first; a segment's final entry replaces its line.
        private val partials = LinkedHashMap<String, Partial>()
        private val finalSegments = LinkedHashSet<String>()
        val jobs = mutableListOf<Job>()
        private var mediaObserved = false

        val joinedHere: Boolean
            get() = phase in setOf(CallPhase.Calling, CallPhase.Ringing, CallPhase.Connecting, CallPhase.Active, CallPhase.Voicemail)

        fun addEntry(entry: TranscriptEntry) {
            entries[entry.seq] = entry
            if (entry.kind != EntryKind.SPEECH) return
            val speaker = entry.actor?.uuid
            val segment = entry.segmentId
            if (segment != null) {
                finalSegments += segment
                if (finalSegments.size > 400) finalSegments.remove(finalSegments.first())
                if (partials.remove(segment) == null && speaker != null) partials.remove(speakerKey(speaker))
                return
            }
            // No segment id: the final replaces the newest interim line of its speaker.
            speaker ?: return
            partials.entries.lastOrNull { it.value.speaker.uuid == speaker }?.let { partials.remove(it.key) }
        }

        fun addPartial(speaker: Account, segmentId: String, text: String) {
            // A partial that arrives after its segment's final would otherwise linger.
            if (segmentId in finalSegments) return
            val key = segmentId.ifEmpty { speakerKey(speaker.uuid) }
            // Updating a segment keeps its place, so two people talking at once do not swap lines.
            if (text.isBlank()) partials.remove(key) else partials[key] = Partial(speaker, text, SystemClock.elapsedRealtime())
        }

        private fun speakerKey(uuid: String) = "speaker:$uuid"

        fun observeTelecom() {
            val telecom = telecom ?: return
            jobs += scope.launch { telecom.endpoints.collect { publish() } }
            jobs += scope.launch { telecom.endpoint.collect { publish() } }
            jobs += scope.launch {
                telecom.muted.collect { systemMuted ->
                    // A headset or the system mute button changed the mute state.
                    if (systemMuted != muted && joinedHere) setMuted(ringid, systemMuted)
                }
            }
        }

        fun observeMedia() {
            if (mediaObserved) return
            val media = media ?: return
            mediaObserved = true
            jobs += scope.launch { media.speakers.collect { publish() } }
            // Routes the call switches itself when Telecom does not hold it.
            jobs += scope.launch { media.routes.collect { publish() } }
            jobs += scope.launch { media.route.collect { publish() } }
            jobs += scope.launch {
                media.status.collect { status ->
                    // Outgoing calls start their ringback once the room is joined.
                    if (status == CallMedia.Status.Connected && !incoming && phase == CallPhase.Calling && call?.state == CallState.RINGING) {
                        phase = CallPhase.Ringing
                        tones.startRingback()
                        publish()
                    }
                }
            }
        }

        fun snapshot(): CallUi {
            val me = me
            val call = call
            val others = buildList {
                add(peer)
                call?.others(me.orEmpty())?.forEach { if (it.uuid != peer.uuid) add(it) }
            }
            val alreadyOn = call?.joined()?.map { it.account }?.filter { it.uuid != me && it.uuid != peer.uuid }
                ?: alreadyOnFromPush.filter { it.uuid != peer.uuid }
            val speakingUuids = media?.speakers?.value.orEmpty().mapNotNull { identityToUuid(it, call) }.toSet()
            val ownAudio = media?.ownsAudio == true
            val routes = (if (ownAudio) media?.routes?.value else telecom?.endpoints?.value).orEmpty().map { AudioRoute(it) }
            return CallUi(
                ringid = ringid,
                incoming = incoming,
                phase = phase,
                call = call,
                peer = peer,
                others = others,
                alreadyOn = alreadyOn,
                invitationId = invitationId,
                silenced = silenced,
                muted = muted,
                speaking = speakingUuids,
                transcript = transcriptLines(),
                answeredAt = parseInstant(call?.answeredAt),
                route = (if (ownAudio) media?.route?.value else telecom?.endpoint?.value)?.let { AudioRoute(it) },
                routes = routes,
                working = working,
                notice = notice,
                endedText = endedText,
            )
        }

        private fun identityToUuid(identity: String, call: Call?): String? {
            if (identity == media?.identity) return me
            call?.participants?.firstOrNull { it.mediaIdentity == identity }?.let { return it.account.uuid }
            val parts = identity.split(':')
            return when (parts.firstOrNull()) {
                "carbon", "rep" -> parts.getOrNull(1)
                else -> null
            }
        }

        private fun transcriptLines(): List<TranscriptLine> {
            val finals = entries.values.mapNotNull { e ->
                val who = e.actor?.label ?: "Someone"
                when (e.kind) {
                    // Keyed by segment so the final takes the place of its interim line in the list.
                    EntryKind.SPEECH -> e.text?.takeIf { it.isNotBlank() }?.let {
                        TranscriptLine(e.segmentId?.let { id -> "g:$id" } ?: "s${e.seq}", e.actor, it, final = true, system = false)
                    }
                    EntryKind.JOINED -> TranscriptLine("s${e.seq}", null, "$who joined", final = true, system = true)
                    EntryKind.LEFT -> TranscriptLine("s${e.seq}", null, "$who left", final = true, system = true)
                    EntryKind.INVITED -> {
                        val invitee = e.dataElement("invitee")?.let {
                            runCatching { com.teamofsilicons.ring.data.RingJson.decodeFromJsonElement(Account.serializer(), it).label }.getOrNull()
                        }
                        TranscriptLine("s${e.seq}", null, if (invitee != null) "$who invited $invitee" else "$who invited someone", final = true, system = true)
                    }
                    EntryKind.DECLINED -> TranscriptLine(
                        "s${e.seq}", null,
                        e.dataString("reason")?.let { "$who declined: $it" } ?: "$who declined",
                        final = true, system = true,
                    )
                    else -> null
                }
            }
            // An interim line whose final never came (the speaker dropped) fades out after a while.
            val now = SystemClock.elapsedRealtime()
            val interim = partials.filter { now - it.value.at < PARTIAL_TTL_MS }
                .map { (key, p) -> TranscriptLine(if (key.startsWith("speaker:")) "p:$key" else "g:$key", p.speaker, p.text, final = false, system = false) }
            // The list is keyed: a repeated key (a server repeating a segment id) would crash it.
            val used = HashSet<String>()
            return (finals + interim).takeLast(80).mapIndexed { i, line -> if (used.add(line.key)) line else line.copy(key = "${line.key}#$i") }
        }
    }

    private class Partial(val speaker: Account, val text: String, val at: Long)

    private companion object {
        const val TAG = "RingCalls"
        const val HOLD = "call"
        const val PARTIAL_TTL_MS = 30_000L
        const val MOVED_SOMEWHERE = "MOVED TO ANOTHER DEVICE"
    }
}
