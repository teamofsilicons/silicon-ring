package com.teamofsilicons.ring.call

import android.content.Context
import android.media.AudioAttributes
import android.media.AudioDeviceCallback
import android.media.AudioDeviceInfo
import android.media.AudioFocusRequest
import android.media.AudioManager
import android.os.Build
import android.os.Handler
import android.os.Looper
import android.os.ParcelUuid
import android.util.Log
import androidx.core.telecom.CallEndpointCompat
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
import io.livekit.android.room.participant.AudioTrackPublishDefaults
import io.livekit.android.room.track.LocalAudioTrackOptions
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Job
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.launch
import java.util.UUID

/**
 * One device's audio in a call: a LiveKit room with a single published microphone track that
 * subscribes to everyone else. Telecom owns audio routing, so LiveKit's own audio handler is off;
 * [ownAudioMode] covers devices where the call could not be registered with Telecom, and then
 * this class also switches routes itself ([routes], [route], [selectRoute]).
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
    private val localRoutes = LocalRoutes(context.getSystemService(AudioManager::class.java))

    /** True when this call set the audio mode itself because Telecom does not hold it. */
    val ownsAudio: Boolean get() = focus != null

    /** Routes this call can play through while it [ownsAudio]; Telecom reports them otherwise. */
    val routes: StateFlow<List<CallEndpointCompat>> = localRoutes.endpoints.asStateFlow()

    /** The route the call plays through while it [ownsAudio]. */
    val route: StateFlow<CallEndpointCompat?> = localRoutes.current.asStateFlow()

    /** Joins the room in [grant]. Returns false when the connection failed. */
    suspend fun connect(grant: MediaGrant, startMuted: Boolean, ownAudioMode: Boolean): Boolean {
        disconnectRoom()
        closing = false
        muted = startMuted
        identity = grant.identity
        if (ownAudioMode) takeAudioMode() else releaseAudioMode()
        val room = LiveKit.create(
            appContext = context.applicationContext,
            options = RoomOptions(
                adaptiveStream = false,
                dynacast = false,
                // The platform's voice processing (hardware echo cancellation and noise
                // suppression where the device has them, WebRTC's otherwise) plus automatic gain.
                audioTrackCaptureDefaults = LocalAudioTrackOptions(
                    noiseSuppression = true,
                    echoCancellation = true,
                    autoGainControl = true,
                    highPassFilter = true,
                    typingNoiseDetection = true,
                ),
                // Mono Opus at 32 kbps with redundant audio (RED) for loss. DTX stays off: its
                // voice detection clips the start of words, which hurts the transcript and the
                // silicon on the other end more than the bandwidth it saves helps.
                audioTrackPublishDefaults = AudioTrackPublishDefaults(
                    audioBitrate = PUBLISH_BITRATE,
                    dtx = false,
                    red = true,
                ),
            ),
            overrides = LiveKitOverrides(
                audioOptions = AudioOptions(
                    audioOutputType = AudioType.CallAudioType(),
                    audioHandler = NoAudioHandler(),
                    // Capture full-band mono. WebRTC otherwise takes the device's output rate,
                    // which some devices report as 16 kHz (8 kHz on emulators).
                    javaAudioDeviceModuleCustomizer = { builder ->
                        builder.setInputSampleRate(CAPTURE_SAMPLE_RATE)
                        builder.setUseStereoInput(false)
                    },
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

    /** Plays the call through [endpoint] (one of [routes]); false when that did not work. */
    fun selectRoute(endpoint: CallEndpointCompat): Boolean = ownsAudio && localRoutes.select(endpoint)

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
        if (focus != null) return
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
        localRoutes.start()
    }

    private fun releaseAudioMode() {
        val request = focus ?: return
        localRoutes.stop()
        val audio = context.getSystemService(AudioManager::class.java)
        audio.abandonAudioFocusRequest(request)
        audio.mode = AudioManager.MODE_NORMAL
        focus = null
    }

    private companion object {
        const val TAG = "RingMedia"
        const val CAPTURE_SAMPLE_RATE = 48_000
        const val PUBLISH_BITRATE = 32_000
    }
}

/**
 * Earpiece, loudspeaker and headset routing through AudioManager, for a call Telecom does not
 * hold. Routes are reported as [CallEndpointCompat] so the call screen treats both paths alike.
 * Android 12+ picks any communication device; before that only the loudspeaker can be chosen.
 */
private class LocalRoutes(private val audio: AudioManager) {
    val endpoints = MutableStateFlow<List<CallEndpointCompat>>(emptyList())
    val current = MutableStateFlow<CallEndpointCompat?>(null)

    private val main = Handler(Looper.getMainLooper())
    private val devices = HashMap<ParcelUuid, AudioDeviceInfo>()
    private var started = false
    private var listener: AudioManager.OnCommunicationDeviceChangedListener? = null
    private val deviceCallback = object : AudioDeviceCallback() {
        override fun onAudioDevicesAdded(addedDevices: Array<out AudioDeviceInfo>) = refresh()
        override fun onAudioDevicesRemoved(removedDevices: Array<out AudioDeviceInfo>) = refresh()
    }

    fun start() {
        if (started) return
        started = true
        if (Build.VERSION.SDK_INT >= 31) {
            val changed = AudioManager.OnCommunicationDeviceChangedListener { refresh() }
            audio.addOnCommunicationDeviceChangedListener({ main.post(it) }, changed)
            listener = changed
        }
        // Also reports the initial devices, which fills the lists.
        audio.registerAudioDeviceCallback(deviceCallback, main)
        refresh()
    }

    fun stop() {
        if (!started) return
        started = false
        audio.unregisterAudioDeviceCallback(deviceCallback)
        if (Build.VERSION.SDK_INT >= 31) {
            listener?.let { audio.removeOnCommunicationDeviceChangedListener(it) }
            listener = null
            audio.clearCommunicationDevice()
        } else {
            @Suppress("DEPRECATION")
            audio.isSpeakerphoneOn = false
        }
        devices.clear()
        endpoints.value = emptyList()
        current.value = null
    }

    fun select(endpoint: CallEndpointCompat): Boolean {
        if (!started) return false
        val ok = if (Build.VERSION.SDK_INT >= 31) {
            val device = devices[endpoint.identifier]
            device != null && audio.setCommunicationDevice(device)
        } else {
            @Suppress("DEPRECATION")
            audio.isSpeakerphoneOn = endpoint.type == CallEndpointCompat.TYPE_SPEAKER
            true
        }
        if (!ok) Log.w(TAG, "Could not route the call to ${endpoint.name}")
        refresh()
        return ok
    }

    private fun refresh() {
        if (!started) return
        if (Build.VERSION.SDK_INT >= 31) {
            devices.clear()
            val list = audio.availableCommunicationDevices.mapNotNull { device ->
                val type = endpointType(device.type) ?: return@mapNotNull null
                val id = ParcelUuid(UUID.nameUUIDFromBytes("${device.type}:${device.id}".toByteArray()))
                devices[id] = device
                CallEndpointCompat(endpointName(type, device), type, id)
            }.distinctBy { it.type to it.name.toString() }
            endpoints.value = list
            val now = audio.communicationDevice
            current.value = list.firstOrNull { devices[it.identifier]?.id == now?.id }
                ?: list.firstOrNull { it.type == now?.type?.let(::endpointType) }
        } else {
            @Suppress("DEPRECATION")
            val speakerOn = audio.isSpeakerphoneOn
            endpoints.value = listOf(EARPIECE, SPEAKER)
            current.value = if (speakerOn) SPEAKER else EARPIECE
        }
    }

    private fun endpointType(deviceType: Int): Int? = when (deviceType) {
        AudioDeviceInfo.TYPE_BUILTIN_EARPIECE -> CallEndpointCompat.TYPE_EARPIECE
        AudioDeviceInfo.TYPE_BUILTIN_SPEAKER -> CallEndpointCompat.TYPE_SPEAKER
        AudioDeviceInfo.TYPE_WIRED_HEADSET, AudioDeviceInfo.TYPE_WIRED_HEADPHONES, AudioDeviceInfo.TYPE_USB_HEADSET ->
            CallEndpointCompat.TYPE_WIRED_HEADSET
        AudioDeviceInfo.TYPE_BLUETOOTH_SCO, AudioDeviceInfo.TYPE_BLE_HEADSET, AudioDeviceInfo.TYPE_HEARING_AID ->
            CallEndpointCompat.TYPE_BLUETOOTH
        else -> null
    }

    private fun endpointName(type: Int, device: AudioDeviceInfo): String = when (type) {
        CallEndpointCompat.TYPE_EARPIECE -> "Phone"
        CallEndpointCompat.TYPE_SPEAKER -> "Speaker"
        CallEndpointCompat.TYPE_WIRED_HEADSET -> "Wired headset"
        else -> device.productName?.toString()?.takeIf { it.isNotBlank() } ?: "Bluetooth"
    }

    private companion object {
        const val TAG = "RingMedia"
        val EARPIECE = CallEndpointCompat("Phone", CallEndpointCompat.TYPE_EARPIECE, ParcelUuid(UUID.nameUUIDFromBytes("earpiece".toByteArray())))
        val SPEAKER = CallEndpointCompat("Speaker", CallEndpointCompat.TYPE_SPEAKER, ParcelUuid(UUID.nameUUIDFromBytes("speaker".toByteArray())))
    }
}
