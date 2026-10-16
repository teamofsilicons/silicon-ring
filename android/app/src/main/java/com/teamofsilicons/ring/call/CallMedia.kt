package com.teamofsilicons.ring.call

import android.content.Context
import android.media.AudioAttributes
import android.media.AudioFocusRequest
import android.media.AudioManager
import android.util.Log
import com.teamofsilicons.ring.data.MediaGrant
import io.livekit.android.AudioOptions
import io.livekit.android.AudioType
import io.livekit.android.LiveKit
import io.livekit.android.LiveKitOverrides
import io.livekit.android.RoomOptions
import io.livekit.android.audio.NoAudioHandler
import io.livekit.android.events.DisconnectReason
import io.livekit.android.events.RoomEvent
import io.livekit.android.events.collect
import io.livekit.android.room.Room
import io.livekit.android.room.track.LocalAudioTrackOptions
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Job
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.launch

/**
 * One device's audio in a call: a LiveKit room with a single published microphone track that
 * subscribes to everyone else. Telecom owns audio routing, so LiveKit's own audio handler is off;
 * [ownAudioMode] covers devices where the call could not be registered with Telecom.
 */
class CallMedia(
    private val context: Context,
    private val scope: CoroutineScope,
    private val onLost: (DisconnectReason) -> Unit,
) {
    enum class Status { Idle, Connecting, Connected, Reconnecting, Disconnected }

    private val _status = MutableStateFlow(Status.Idle)
    val status: StateFlow<Status> = _status.asStateFlow()

    /** LiveKit identities that are speaking right now (including this device's own). */
    private val _speakers = MutableStateFlow<Set<String>>(emptySet())
    val speakers: StateFlow<Set<String>> = _speakers.asStateFlow()

    var identity: String? = null
        private set

    private var room: Room? = null
    private var events: Job? = null
    private var closing = false
    private var muted = false
    private var focus: AudioFocusRequest? = null

    /** Joins the room in [grant]. Returns false when the connection failed. */
    suspend fun connect(grant: MediaGrant, startMuted: Boolean, ownAudioMode: Boolean): Boolean {
        disconnectRoom()
        closing = false
        muted = startMuted
        identity = grant.identity
        if (ownAudioMode) takeAudioMode()
        val room = LiveKit.create(
            appContext = context.applicationContext,
            options = RoomOptions(
                adaptiveStream = false,
                dynacast = false,
                audioTrackCaptureDefaults = LocalAudioTrackOptions(
                    noiseSuppression = true,
                    echoCancellation = true,
                    autoGainControl = true,
                    highPassFilter = true,
                    typingNoiseDetection = true,
                ),
            ),
            overrides = LiveKitOverrides(
                audioOptions = AudioOptions(
                    audioOutputType = AudioType.CallAudioType(),
                    audioHandler = NoAudioHandler(),
                ),
            ),
        )
        this.room = room
        _status.value = Status.Connecting
        events = scope.launch {
            room.events.collect { event ->
                when (event) {
                    is RoomEvent.ActiveSpeakersChanged ->
                        _speakers.value = event.speakers.mapNotNull { it.identity?.value }.toSet()
                    is RoomEvent.Reconnecting -> _status.value = Status.Reconnecting
                    is RoomEvent.Reconnected -> _status.value = Status.Connected
                    is RoomEvent.Disconnected -> {
                        _status.value = Status.Disconnected
                        _speakers.value = emptySet()
                        if (!closing && event.reason != DisconnectReason.CLIENT_INITIATED) {
                            Log.i(TAG, "Media disconnected: ${event.reason}")
                            onLost(event.reason)
                        }
                    }
                    else -> Unit
                }
            }
        }
        return try {
            room.connect(grant.url, grant.token)
            publishMicrophone(room)
            _status.value = Status.Connected
            true
        } catch (e: Exception) {
            Log.w(TAG, "Could not join the call media", e)
            _status.value = Status.Disconnected
            disconnectRoom()
            false
        }
    }

    /**
     * Publishes the microphone even when muted: voicemail recording and handoff expect a track.
     * Without microphone access (a call answered from a watch or headset before the permission
     * was granted) the call still plays; the other side just cannot hear this device.
     */
    private suspend fun publishMicrophone(room: Room) {
        try {
            room.localParticipant.setMicrophoneEnabled(true)
            if (muted) room.localParticipant.setMicrophoneEnabled(false)
        } catch (e: Exception) {
            Log.w(TAG, "Could not publish the microphone; the call continues without it", e)
        }
    }

    suspend fun setMuted(value: Boolean) {
        muted = value
        runCatching { room?.localParticipant?.setMicrophoneEnabled(!value) }
            .onFailure { Log.w(TAG, "Mute change failed", it) }
    }

    fun disconnect() {
        closing = true
        disconnectRoom()
        releaseAudioMode()
        _status.value = Status.Idle
    }

    private fun disconnectRoom() {
        events?.cancel()
        events = null
        room?.let {
            runCatching { it.disconnect() }
            runCatching { it.release() }
        }
        room = null
        _speakers.value = emptySet()
    }

    private fun takeAudioMode() {
        val audio = context.getSystemService(AudioManager::class.java)
        val request = AudioFocusRequest.Builder(AudioManager.AUDIOFOCUS_GAIN_TRANSIENT)
            .setAudioAttributes(
                AudioAttributes.Builder()
                    .setUsage(AudioAttributes.USAGE_VOICE_COMMUNICATION)
                    .setContentType(AudioAttributes.CONTENT_TYPE_SPEECH)
                    .build(),
            )
            .build()
        audio.requestAudioFocus(request)
        audio.mode = AudioManager.MODE_IN_COMMUNICATION
        focus = request
    }

    private fun releaseAudioMode() {
        val request = focus ?: return
        val audio = context.getSystemService(AudioManager::class.java)
        audio.abandonAudioFocusRequest(request)
        audio.mode = AudioManager.MODE_NORMAL
        focus = null
    }

    private companion object {
        const val TAG = "RingMedia"
    }
}
