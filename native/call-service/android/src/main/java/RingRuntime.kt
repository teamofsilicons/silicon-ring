package com.teamofsilicons.callservice

import android.Manifest
import android.content.ComponentName
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.content.pm.ApplicationInfo
import android.media.*
import android.os.Handler
import android.os.Looper
import android.telecom.PhoneAccount
import android.telecom.PhoneAccountHandle
import android.telecom.TelecomManager
import android.util.Base64
import androidx.core.content.ContextCompat
import okhttp3.*
import org.json.JSONObject
import java.nio.ByteBuffer
import java.nio.ByteOrder
import java.util.UUID
import java.util.concurrent.TimeUnit
import kotlin.concurrent.thread
import kotlin.math.sqrt

internal typealias RingReply = (JSONObject?, String?) -> Unit
private fun environmentRealm(value: String): String {
    val realm = value.trim().lowercase()
    val uuid = runCatching { UUID.fromString(realm) }.getOrNull()
    require(realm in listOf("production", "test") || uuid?.toString() == realm && uuid != UUID(0, 0)) { "Choose production, legacy test, or a valid Honeycomb environment UUID." }
    return realm
}
private fun acknowledgesRealm(realm: String, result: JSONObject?): Boolean = result?.optString("realm") == realm || result?.has("realm") != true && realm in listOf("production", "test")
internal class RingRuntime private constructor(val context: Context) {
    companion object {
        @Volatile private var instance: RingRuntime? = null
        fun get(context: Context): RingRuntime = instance ?: synchronized(this) {
            instance ?: RingRuntime(context.applicationContext).also { instance = it; it.restore() }
        }
    }
    private val main = Handler(Looper.getMainLooper())
    private val client = OkHttpClient.Builder().pingInterval(20, TimeUnit.SECONDS).build()
    private var socket: WebSocket? = null
    private var connecting = false
    private var enabled = false
    private val pending = mutableMapOf<String, RingReply>()
    private val waiting = mutableListOf<(String?) -> Unit>()
    var credentials = JSONObject(); private set
    var ready = false; private set
    val audio = RingAudio(this)
    val actor get() = credentials.optString("actor")
    val device get() = credentials.optString("device_id")
    val phoneAccount = PhoneAccountHandle(ComponentName(context, RingConnectionService::class.java), "ring")
    val calls = mutableMapOf<String, JSONObject>()
    private var pushToken = ""
    private fun restore() { SessionVault.load(context)?.let { runCatching { configure(it) } } }
    fun configure(value: JSONObject) {
        val endpoint = java.net.URI(value.optString("url"))
        require(endpoint.scheme in listOf("ws", "wss") && endpoint.userInfo == null && value.optString("session_token").isNotEmpty() && value.optString("device_id").isNotEmpty()) { "Native calling needs a valid endpoint and authenticated session." }
        require(endpoint.scheme == "wss" || endpoint.host in listOf("localhost", "127.0.0.1", "::1", "[::1]")) { "Use a secure wss:// endpoint outside localhost." }
        val realm = environmentRealm(if (value.has("realm")) value.getString("realm") else "production")
        require(realm == "production" || value.optString("test_app_secret").isNotBlank()) { "A test app secret is required for test environments." }
        value.put("realm", realm)
        if (listOf("session_token", "device_id", "url", "realm", "org_id", "test_app_secret").any { value.optString(it) != credentials.optString(it) }) disconnect()
        credentials = value; enabled = true; SessionVault.save(context, value)
        val telecom = context.getSystemService(TelecomManager::class.java)
        telecom.registerPhoneAccount(PhoneAccount.builder(phoneAccount, "Silicon Ring").setCapabilities(PhoneAccount.CAPABILITY_SELF_MANAGED).setSupportedUriSchemes(listOf(PhoneAccount.SCHEME_SIP)).build())
        connect { if (it == null) registerPush(pushToken) }
    }
    fun connect(callback: (String?) -> Unit) {
        if (Looper.myLooper() != Looper.getMainLooper()) { main.post { connect(callback) }; return }
        if (ready) { callback(null); return }
        waiting.add(callback)
        if (connecting) return
        if (!enabled || credentials.optString("url").isEmpty()) { connected("Sign in to Ring first."); return }
        connecting = true
        socket = client.newWebSocket(Request.Builder().url(credentials.getString("url")).build(), object : WebSocketListener() {
            override fun onOpen(webSocket: WebSocket, response: Response) { main.post {
                if (socket !== webSocket) return@post
                val realm = credentials.getString("realm")
                val hello = JSONObject().put("versions", org.json.JSONArray().put(1)).put("client", JSONObject().put("name", "ring-android").put("version", "0.1.5")).put("realm", realm).put("org_id", credentials.optString("org_id"))
                if (realm != "production") hello.put("test_app_secret", credentials.getString("test_app_secret"))
                rawRequest("protocol.hello", hello) { result, error ->
                    val helloError = error ?: if (!acknowledgesRealm(realm, result)) "The server did not confirm the selected environment." else null
                    if (helloError != null) connected(helloError) else rawRequest("auth.resume", JSONObject().put("session_token", credentials.optString("session_token")).put("device_id", device)) { session, error ->
                        val authError = error ?: if (!acknowledgesRealm(realm, session)) "The session does not belong to the selected environment." else null
                        if (authError != null) connected(authError) else {
                            ready = true; rawRequest("events.subscribe", JSONObject()) { _, _ -> }; connected(null); registerPush(pushToken)
                            request("calls.list", JSONObject().put("state", "active")) { result, _ ->
                                val items = result?.optJSONArray("items"); if (items != null) for (index in 0 until items.length()) reconcile(items.getJSONObject(index))
                            }
                        }
                    }
                }
            } }
            override fun onMessage(webSocket: WebSocket, text: String) { main.post { if (socket === webSocket) receive(text) } }
            override fun onFailure(webSocket: WebSocket, t: Throwable, response: Response?) { main.post { if (socket === webSocket) lost(t.message ?: "Connection lost") } }
            override fun onClosed(webSocket: WebSocket, code: Int, reason: String) { main.post { if (socket === webSocket) lost("Connection closed") } }
        })
    }
    private fun connected(error: String?) {
        connecting = false
        val callbacks = waiting.toList(); waiting.clear(); callbacks.forEach { it(error) }
        if (error != null) { ready = false; socket?.cancel(); socket = null }
    }
    private fun lost(error: String) {
        ready = false; connecting = false; socket = null; audio.close()
        val callbacks = pending.values.toList(); pending.clear(); callbacks.forEach { it(null, error) }
        val connections = waiting.toList(); waiting.clear(); connections.forEach { it(error) }
        if (enabled) main.postDelayed({ connect {} }, 2000)
    }
    fun request(method: String, params: JSONObject = JSONObject(), callback: RingReply) {
        connect { error -> if (error != null) callback(null, error) else rawRequest(method, params, callback) }
    }
    private fun rawRequest(method: String, params: JSONObject, callback: RingReply) {
        val id = UUID.randomUUID().toString(); pending[id] = callback
        socket?.send(JSONObject().put("id", id).put("method", method).put("params", params).toString())
        main.postDelayed({ pending.remove(id)?.invoke(null, "$method timed out. Check call state before retrying.") }, 20000)
    }
    fun frame(type: String, data: JSONObject) { if (ready && (socket?.queueSize() ?: 0) < 256000) socket?.send(JSONObject().put("type", type).put("data", data).toString()) }
    private fun receive(text: String) {
        try {
            val message = JSONObject(text)
            val id = message.optString("id")
            if (id.isNotEmpty()) {
                pending.remove(id)?.let { reply -> if (message.optBoolean("ok")) reply(message.optJSONObject("result") ?: JSONObject(), null) else reply(null, message.optJSONObject("error")?.optString("message") ?: "Ring rejected the request") }
                return
            }
            val type = message.optString("type"); val data = message.optJSONObject("data") ?: JSONObject()
            if (type == "media.audio") { audio.output(data); return }
            if (type.startsWith("call.") || type.startsWith("participant.")) {
                val ring = data.optString("ringid")
                if (type == "call.ended" && audio.matches(ring)) audio.close()
                if (ring.isNotEmpty()) request("calls.get", JSONObject().put("ringid", ring)) { result, _ -> result?.let { reconcile(it) } }
            }
        } catch (_: Exception) { /* Malformed unsolicited frames do not change native state. */ }
    }
    private fun reconcile(call: JSONObject) {
        val ring = call.optString("ringid"); if (ring.isEmpty()) return
        calls[ring] = call
        val offers = call.optJSONArray("invitations")
        val incoming = offers != null && (0 until offers.length()).any { val item = offers.getJSONObject(it); item.optString("target") == actor && item.optString("state") == "pending" && !item.optBoolean("silenced") }
        if (incoming) { incoming(ring, call.optString("caller"), call.optString("caller")); return }
        val silenced = offers != null && (0 until offers.length()).any { val item = offers.getJSONObject(it); item.optString("target") == actor && item.optString("state") == "pending" && item.optBoolean("silenced") }
        if (silenced) { RingConnectionService.end(ring); context.stopService(Intent(context, RingCallService::class.java)); return }
        val participants = call.optJSONArray("participants")
        val here = participants != null && (0 until participants.length()).any { val item = participants.getJSONObject(it); item.optString("actor") == actor && item.optString("device_id") == device && item.isNull("left_at") }
        if (call.optString("state") == "active" && here) {
            RingConnectionService.connections[ring]?.setActive()
            if (!audio.voicemail && ContextCompat.checkSelfPermission(context, Manifest.permission.RECORD_AUDIO) == PackageManager.PERMISSION_GRANTED) startAudio(ring, null) { _, _ -> }
            RingCallService.show(context, ring, "Call in progress", false)
        } else if (call.optString("state") == "ended" || (!here && call.optString("state") != "ringing")) {
            if (audio.matches(ring)) stopAudio { _, _ -> }
            RingConnectionService.end(ring)
            if (RingConnectionService.connections.isEmpty()) context.stopService(Intent(context, RingCallService::class.java))
        }
    }
    fun incoming(ringid: String, caller: String, name: String) {
        if (ringid.isEmpty() || RingConnectionService.connections.containsKey(ringid) || RingConnectionService.incoming.containsKey(ringid)) return
        RingConnectionService.incoming[ringid] = Pair(caller, name)
        val extras = android.os.Bundle().apply { putString("ringid", ringid); putString("caller", caller); putString("display_name", name) }
        try { context.getSystemService(TelecomManager::class.java).addNewIncomingCall(phoneAccount, extras) }
        catch (_: SecurityException) { RingCallService.show(context, ringid, name, true) }
        connect {}
    }
    fun registerPush(token: String) { if (token.isEmpty()) return; pushToken = token; if (ready) request("devices.update", JSONObject().put("device_id", device).put("push_platform", "fcm").put("push_environment", if (context.applicationInfo.flags and ApplicationInfo.FLAG_DEBUGGABLE != 0) "sandbox" else "production").put("push_token", token)) { _, _ -> } }
    fun answer(ringid: String, reply: RingReply = { _, _ -> }) {
        request("calls.accept", JSONObject().put("ringid", ringid).put("device_id", device)) { value, error ->
            if (error == null) { RingConnectionService.connections[ringid]?.setActive(); startAudio(ringid, null) { _, _ -> }; RingCallService.show(context, ringid, "Call in progress", false) }
            reply(value, error)
        }
    }
    fun end(ringid: String, decline: Boolean, reply: RingReply = { _, _ -> }) {
        if (audio.matches(ringid)) stopAudio { _, _ -> }
        request(if (decline) "calls.decline" else "calls.cut", JSONObject().put("ringid", ringid).put("give_no_reason", true)) { value, error ->
            if (error == null) { RingConnectionService.end(ringid); context.stopService(Intent(context, RingCallService::class.java)) }
            reply(value, error)
        }
    }
    fun startAudio(ringid: String, voicemail: String?, reply: RingReply) {
        if (ringid.isEmpty()) { reply(null, "A call ID is required."); return }
        if (ContextCompat.checkSelfPermission(context, Manifest.permission.RECORD_AUDIO) != PackageManager.PERMISSION_GRANTED) { reply(null, "Open Ring and allow microphone access to join audio."); return }
        audio.start(ringid, voicemail, reply)
    }
    fun stopAudio(reply: RingReply) = audio.stop(reply)
    fun mute(muted: Boolean, reply: RingReply) {
        request("media.state", JSONObject().put("stream_id", audio.streamId).put("muted", muted)) { value, error -> if (error == null) audio.muted = muted; reply(value, error) }
    }
    private fun disconnect() {
        enabled = false; ready = false; connecting = false
        socket?.close(1000, "Signed out"); socket = null; audio.close()
        val callbacks = pending.values.toList(); pending.clear(); callbacks.forEach { it(null, "Signed out") }
        val connections = waiting.toList(); waiting.clear(); connections.forEach { it("Signed out") }
    }
    fun logout() { disconnect(); credentials = JSONObject(); SessionVault.save(context, null); calls.clear(); RingConnectionService.connections.keys.toList().forEach { RingConnectionService.end(it) }; context.stopService(Intent(context, RingCallService::class.java)) }
}

internal class RingAudio(private val runtime: RingRuntime) {
    @Volatile var streamId = ""; private set
    @Volatile var ringid = ""; private set
    @Volatile var muted = false
    @Volatile var voicemail = false; private set
    @Volatile private var recording = false
    @Volatile private var recorder: AudioRecord? = null
    private var track: AudioTrack? = null
    private var seq = 0
    private var starting = false
    private var generation = 0L
    private var pendingRing = ""
    private var pendingVoicemail: String? = null
    private val startWaiters = mutableListOf<RingReply>()
    fun matches(ring: String) = ring.isNotEmpty() && (ringid == ring || pendingRing == ring)
    private var lastOutput = -1
    private var startedAt = android.os.SystemClock.elapsedRealtime()
    private var lastOffset = -20
    fun start(ring: String, voicemailId: String?, reply: RingReply) {
        if (ring == ringid && streamId.isNotEmpty() && voicemail == (voicemailId != null)) { reply(JSONObject().put("stream_id", streamId), null); return }
        if (starting) {
            if (pendingRing != ring || pendingVoicemail != voicemailId) reply(null, "Another audio connection is in progress. Stop it before switching audio.")
            else startWaiters.add(reply)
            return
        }
        val attempt = ++generation
        starting = true; pendingRing = ring; pendingVoicemail = voicemailId; startWaiters.add(reply)
        detachCurrent(afterDetach@{ _, _ ->
            if (attempt != generation) return@afterDetach
            val params = JSONObject().put("ringid", ring).put("device_id", runtime.device).put("purpose", if (voicemailId == null) "call" else "voicemail")
            if (voicemailId != null) params.put("voicemail_id", voicemailId)
            runtime.request("media.attach", params) { value, error ->
                if (attempt != generation) {
                    value?.optString("stream_id")?.takeIf { it.isNotEmpty() }?.let {
                        runtime.request("media.detach", JSONObject().put("stream_id", it)) { _, _ -> }
                    }
                    return@request
                }
                var failure = error
                if (failure == null) try {
                    streamId = value!!.getString("stream_id"); ringid = ring; voicemail = voicemailId != null; seq = 0; lastOutput = -1; muted = false; startedAt = android.os.SystemClock.elapsedRealtime(); lastOffset = -20
                    open(); RingCallService.show(runtime.context, ring, if (voicemail) "Recording voicemail" else "Call in progress", false)
                } catch (e: Exception) { failure = e.message ?: "Could not start native microphone"; closeMedia() }
                starting = false; pendingRing = ""; pendingVoicemail = null
                val callbacks = startWaiters.toList(); startWaiters.clear(); callbacks.forEach { it(value, failure) }
            }
        }, false)
    }
    @Suppress("MissingPermission")
    private fun open() {
        val audioManager = runtime.context.getSystemService(AudioManager::class.java); audioManager.mode = AudioManager.MODE_IN_COMMUNICATION
        val inputSize = AudioRecord.getMinBufferSize(24000, AudioFormat.CHANNEL_IN_MONO, AudioFormat.ENCODING_PCM_16BIT)
        require(inputSize > 0) { "This microphone does not support 24 kHz audio." }
        val input = AudioRecord(MediaRecorder.AudioSource.VOICE_COMMUNICATION, 24000, AudioFormat.CHANNEL_IN_MONO, AudioFormat.ENCODING_PCM_16BIT, maxOf(inputSize, 960 * 8))
        require(input.state == AudioRecord.STATE_INITIALIZED) { "Microphone could not initialize." }
        recorder = input
        val output = AudioTrack.Builder().setAudioAttributes(AudioAttributes.Builder().setUsage(AudioAttributes.USAGE_VOICE_COMMUNICATION).setContentType(AudioAttributes.CONTENT_TYPE_SPEECH).build())
            .setAudioFormat(AudioFormat.Builder().setSampleRate(24000).setChannelMask(AudioFormat.CHANNEL_OUT_MONO).setEncoding(AudioFormat.ENCODING_PCM_16BIT).build())
            .setBufferSizeInBytes(maxOf(AudioTrack.getMinBufferSize(24000, AudioFormat.CHANNEL_OUT_MONO, AudioFormat.ENCODING_PCM_16BIT), 960 * 8)).setTransferMode(AudioTrack.MODE_STREAM).build()
        track = output; output.play(); input.startRecording(); recording = true
        thread(name = "ring-microphone", isDaemon = true) {
            val samples = ShortArray(480)
            while (recording && recorder === input) {
                var count = 0
                while (count < samples.size && recording && recorder === input) {
                    val read = input.read(samples, count, samples.size - count, AudioRecord.READ_BLOCKING)
                    if (read <= 0) { if (recorder === input) recording = false; break }
                    count += read
                }
                if (recorder !== input) break
                if (count != 480 || muted || !runtime.ready || streamId.isEmpty()) continue
                val bytes = ByteBuffer.allocate(960).order(ByteOrder.LITTLE_ENDIAN); for (sample in samples) bytes.putShort(sample)
                val number = seq++; val offset = maxOf(lastOffset + 20, (android.os.SystemClock.elapsedRealtime() - startedAt).toInt()); lastOffset = offset
                runtime.frame("media.audio", JSONObject().put("stream_id", streamId).put("seq", number).put("offset_ms", offset).put("audio_base64", Base64.encodeToString(bytes.array(), Base64.NO_WRAP)))
                val rms = sqrt(samples.sumOf { val v = it.toDouble() / 32768; v * v } / 480)
                if (rms > 0.018) runtime.frame("media.speech", JSONObject().put("stream_id", streamId).put("seq", number).put("start_ms", offset).put("end_ms", offset + 20).put("confidence", minOf(1.0, rms * 10)))
            }
        }
    }
    fun output(data: JSONObject) {
        if (data.optString("stream_id") != streamId || data.optInt("seq") <= lastOutput) return
        val bytes = try { Base64.decode(data.optString("audio_base64"), Base64.NO_WRAP) } catch (_: Exception) { return }
        if (bytes.size != 960) return
        lastOutput = data.optInt("seq"); track?.write(bytes, 0, bytes.size, AudioTrack.WRITE_NON_BLOCKING)
    }
    fun close() {
        generation++; starting = false; pendingRing = ""; pendingVoicemail = null
        val callbacks = startWaiters.toList(); startWaiters.clear()
        closeMedia()
        callbacks.forEach { it(null, "Audio request cancelled.") }
    }
    private fun closeMedia() {
        recording = false; streamId = ""; ringid = ""
        try { recorder?.stop() } catch (_: Exception) {}
        recorder?.release(); recorder = null
        try { track?.stop() } catch (_: Exception) {}
        track?.release(); track = null
        runtime.context.getSystemService(AudioManager::class.java).mode = AudioManager.MODE_NORMAL
    }
    fun stop(reply: RingReply) = detachCurrent(reply, true)
    private fun detachCurrent(reply: RingReply, cancelStart: Boolean) {
        val id = streamId; val last = seq - 1; val privateAudio = voicemail
        if (cancelStart) close() else closeMedia()
        if (id.isEmpty() || !runtime.ready) { reply(JSONObject().put("complete", id.isEmpty() || !privateAudio), null); return }
        val params = JSONObject().put("stream_id", id)
        if (privateAudio) params.put("last_seq", maxOf(0, last))
        runtime.request("media.detach", params, reply)
    }
}
