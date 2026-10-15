package com.teamofsilicons.ring.data

import android.content.Context
import android.net.ConnectivityManager
import android.net.Network
import android.util.Log
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.CompletableJob
import kotlinx.coroutines.Job
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableSharedFlow
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.SharedFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asSharedFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.isActive
import kotlinx.coroutines.launch
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.contentOrNull
import kotlinx.serialization.json.jsonObject
import okhttp3.OkHttpClient
import okhttp3.Request
import okhttp3.Response
import okhttp3.WebSocket
import okhttp3.WebSocketListener
import java.util.concurrent.TimeUnit
import kotlin.random.Random

/** A server frame from `GET /v1/ws`. */
sealed interface RingEvent {
    data class Ready(val account: Account, val deviceId: String) : RingEvent
    data class CallIncoming(val call: Call, val invitation: Invitation) : RingEvent
    data class CallUpdated(val call: Call) : RingEvent
    data class RingingStopped(val ringid: String, val invitationId: String, val reason: String) : RingEvent
    data class Handoff(val ringid: String, val deviceId: String) : RingEvent
    data class CallEnded(val call: Call) : RingEvent
    data class Transcript(val ringid: String, val entry: TranscriptEntry) : RingEvent
    data class TranscriptPartial(val ringid: String, val speaker: Account, val segmentId: String, val text: String) : RingEvent
    data class VoicemailReceived(val voicemail: Voicemail) : RingEvent
    data class VoicemailUpdated(val voicemail: Voicemail) : RingEvent
    data class DeviceUpdated(val device: Device) : RingEvent
    data object SessionRevoked : RingEvent
}

object RingingStoppedReason {
    const val ACCEPTED_ELSEWHERE = "accepted_elsewhere"
    const val DECLINED = "declined"
    const val SILENCED = "silenced"
    const val CANCELLED = "cancelled"
    const val MISSED = "missed"
}

internal fun parseEvent(text: String): RingEvent? {
    val frame = RingJson.parseToJsonElement(text).jsonObject
    val type = (frame["type"] as? JsonPrimitive)?.contentOrNull ?: return null
    val data = frame["data"] as? JsonObject ?: JsonObject(emptyMap())
    fun str(key: String) = (data[key] as? JsonPrimitive)?.contentOrNull.orEmpty()
    fun call() = RingJson.decodeFromJsonElement(Call.serializer(), data.getValue("call"))
    return when (type) {
        "ready" -> RingEvent.Ready(RingJson.decodeFromJsonElement(Account.serializer(), data.getValue("account")), str("device_id"))
        "call.incoming" -> RingEvent.CallIncoming(call(), RingJson.decodeFromJsonElement(Invitation.serializer(), data.getValue("invitation")))
        "call.updated" -> RingEvent.CallUpdated(call())
        "call.ringing_stopped" -> RingEvent.RingingStopped(str("ringid"), str("invitation_id"), str("reason"))
        "call.handoff" -> RingEvent.Handoff(str("ringid"), str("device_id"))
        "call.ended" -> RingEvent.CallEnded(call())
        "transcript.entry" -> RingEvent.Transcript(str("ringid"), RingJson.decodeFromJsonElement(TranscriptEntry.serializer(), data.getValue("entry")))
        "transcript.partial" -> RingEvent.TranscriptPartial(
            str("ringid"),
            RingJson.decodeFromJsonElement(Account.serializer(), data.getValue("speaker")),
            str("segment_id"),
            str("text"),
        )
        "voicemail.received" -> RingEvent.VoicemailReceived(RingJson.decodeFromJsonElement(Voicemail.serializer(), data.getValue("voicemail")))
        "voicemail.updated" -> RingEvent.VoicemailUpdated(RingJson.decodeFromJsonElement(Voicemail.serializer(), data.getValue("voicemail")))
        "device.updated" -> RingEvent.DeviceUpdated(RingJson.decodeFromJsonElement(Device.serializer(), data.getValue("device")))
        "session.revoked" -> RingEvent.SessionRevoked
        else -> null // pong, delegation.* (silicons only) and anything newer
    }
}

/**
 * The realtime socket. It stays open while something holds it (the app on screen, a live call)
 * and reconnects with jittered backoff, immediately when the network comes back.
 */
class Realtime(
    context: Context,
    http: OkHttpClient,
    private val sessions: SessionStore,
    private val scope: CoroutineScope,
) {
    enum class Status { Offline, Connecting, Connected }

    private val client = http.newBuilder()
        .pingInterval(20, TimeUnit.SECONDS)
        .readTimeout(0, TimeUnit.MILLISECONDS)
        .build()

    private val _events = MutableSharedFlow<RingEvent>(extraBufferCapacity = 256)
    val events: SharedFlow<RingEvent> = _events.asSharedFlow()

    private val _status = MutableStateFlow(Status.Offline)
    val status: StateFlow<Status> = _status.asStateFlow()

    private val holders = mutableSetOf<String>()
    private var socket: WebSocket? = null
    private var loop: Job? = null
    private var attempt = 0
    private var wake: CompletableJob = Job()

    init {
        val connectivity = context.getSystemService(ConnectivityManager::class.java)
        runCatching {
            connectivity.registerDefaultNetworkCallback(object : ConnectivityManager.NetworkCallback() {
                override fun onAvailable(network: Network) {
                    scope.launch { attempt = 0; wake.complete() }
                }
            })
        }
    }

    /** Keeps the socket open on behalf of [reason] until [release] is called with it. */
    fun hold(reason: String) = scope.launch {
        holders += reason
        ensureLoop()
    }

    fun release(reason: String) = scope.launch {
        holders -= reason
        if (holders.isEmpty()) stop()
    }

    /** Drops the current connection (for example after sign-out or a server change). */
    fun reset() = scope.launch {
        // cancel(), not close(): a graceful close waits up to a minute for a server that may be gone.
        socket?.cancel()
        socket = null
        attempt = 0
        wake.complete()
    }

    private fun ensureLoop() {
        if (loop?.isActive == true) return
        loop = scope.launch {
            while (isActive && holders.isNotEmpty()) {
                val session = sessions.current
                if (session == null) {
                    _status.value = Status.Offline
                    awaitWake(Long.MAX_VALUE)
                    continue
                }
                val closed = Job()
                _status.value = Status.Connecting
                val url = session.apiUrl.trimEnd('/').replaceFirst(Regex("^http"), "ws") + "/v1/ws"
                val request = Request.Builder()
                    .url(url)
                    .header("Authorization", "Bearer ${session.token}")
                    .build()
                val ws = client.newWebSocket(request, Listener(closed))
                socket = ws
                val pinger = launch {
                    while (isActive) {
                        delay(30_000)
                        ws.send("{\"type\":\"ping\"}")
                    }
                }
                closed.join()
                pinger.cancel()
                if (socket === ws) socket = null
                _status.value = Status.Offline
                if (holders.isEmpty()) break
                val backoff = (1_000L shl attempt.coerceAtMost(5)).coerceAtMost(30_000L)
                attempt++
                awaitWake(backoff + Random.nextLong(0, 750))
            }
            _status.value = Status.Offline
        }
    }

    private suspend fun awaitWake(timeoutMs: Long) {
        wake = Job()
        kotlinx.coroutines.withTimeoutOrNull(timeoutMs) { wake.join() }
    }

    private fun stop() {
        loop?.cancel()
        loop = null
        socket?.close(1000, null)
        socket = null
        _status.value = Status.Offline
    }

    private inner class Listener(private val closed: CompletableJob) : WebSocketListener() {
        override fun onOpen(webSocket: WebSocket, response: Response) {
            scope.launch { attempt = 0 }
        }

        override fun onMessage(webSocket: WebSocket, text: String) {
            val event = try {
                parseEvent(text)
            } catch (e: Exception) {
                // Only the exception type: its message can quote the frame, which may hold call text.
                Log.w(TAG, "Ignoring an unreadable realtime frame: ${e.javaClass.simpleName}")
                null
            } ?: return
            scope.launch {
                if (event is RingEvent.Ready) _status.value = Status.Connected
                _events.emit(event)
            }
        }

        override fun onClosing(webSocket: WebSocket, code: Int, reason: String) {
            webSocket.close(1000, null)
        }

        override fun onClosed(webSocket: WebSocket, code: Int, reason: String) {
            closed.complete()
        }

        override fun onFailure(webSocket: WebSocket, t: Throwable, response: Response?) {
            if (response?.code == 401) scope.launch { _events.emit(RingEvent.SessionRevoked) }
            Log.i(TAG, "Realtime connection dropped: ${t.javaClass.simpleName}")
            closed.complete()
        }
    }

    private companion object {
        const val TAG = "RingRealtime"
    }
}
