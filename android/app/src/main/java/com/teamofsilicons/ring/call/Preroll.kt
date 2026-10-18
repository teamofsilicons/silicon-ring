package com.teamofsilicons.ring.call

import android.util.Log
import io.livekit.android.events.RoomEvent
import io.livekit.android.events.collect
import io.livekit.android.room.Room
import io.livekit.android.room.datastream.StreamBytesOptions
import io.livekit.android.room.datastream.outgoing.ByteStreamSender
import io.livekit.android.room.participant.Participant
import io.livekit.android.room.track.LocalAudioTrack
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Job
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlinx.coroutines.withTimeoutOrNull
import livekit.org.webrtc.AudioTrackSink
import java.io.ByteArrayOutputStream
import java.nio.ByteBuffer
import java.nio.ByteOrder
import kotlin.math.PI
import kotlin.math.cos
import kotlin.math.sin

/**
 * The microphone's pre-roll: what it hears from the moment the call is answered, while the call's
 * audio connects and before the published track reaches anyone. It streams to ring-media
 * ([com.teamofsilicons.ring.data.MediaGrant.prerollTo]) as LiveKit pre-connect audio (16-bit
 * little-endian PCM), and ring-media puts it in front of the live track, so the first words reach
 * the recording, the transcript and a silicon's representative.
 */
class Preroll private constructor(
    private val track: LocalAudioTrack,
    private val scope: CoroutineScope,
) : AudioTrackSink {
    private val lock = Any()
    private val pending = ByteArrayOutputStream()
    private val halver = Halver()
    /** The rate sent: 48 kHz capture is halved to save uplink, other rates go as they are. */
    private var rate = 0
    private var stopped = false
    private var sender: ByteStreamSender? = null
    private var jobs = emptyList<Job>()

    /** Called on WebRTC's audio thread with each 10 ms of captured audio. */
    override fun onData(
        audioData: ByteBuffer,
        bitsPerSample: Int,
        sampleRate: Int,
        numberOfChannels: Int,
        numberOfFrames: Int,
        absoluteCaptureTimestampMs: Long,
    ) {
        if (bitsPerSample != 16 || numberOfChannels < 1) return
        val samples = audioData.duplicate().order(ByteOrder.LITTLE_ENDIAN).asShortBuffer()
        val mono = ShortArray(numberOfFrames) { samples.get(it * numberOfChannels) }
        val halve = sampleRate == CAPTURE_RATE
        val out = if (halve) halver.process(mono) else mono
        val outRate = if (halve) sampleRate / 2 else sampleRate
        val bytes = ByteBuffer.allocate(out.size * 2).order(ByteOrder.LITTLE_ENDIAN)
        out.forEach { bytes.putShort(it) }
        synchronized(lock) {
            if (stopped) return
            // A stream carries one rate; a route change mid-way (to Bluetooth) ends it.
            if (rate != 0 && rate != outRate) {
                stopLocked()
                return
            }
            rate = outRate
            pending.write(bytes.array())
            if (sender == null && pending.size() > MAX_BYTES) {
                val kept = pending.toByteArray().copyOfRange(pending.size() - MAX_BYTES, pending.size())
                pending.reset()
                pending.write(kept)
            }
        }
    }

    /**
     * Opens the stream to [identity] and starts sending. Done before the track is published, so
     * ring-media expects the pre-roll by the time the track arrives.
     */
    suspend fun open(room: Room, identity: String) {
        // The rate is known once the microphone has delivered audio, which it has while connecting.
        val rate = withTimeoutOrNull(FIRST_AUDIO_TIMEOUT_MS) {
            while (synchronized(lock) { rate == 0 && !stopped }) delay(20)
            synchronized(lock) { rate }
        }
        if (rate == null || rate == 0) {
            stop()
            return
        }
        val sender = try {
            room.localParticipant.streamBytes(
                StreamBytesOptions(
                    topic = TOPIC,
                    attributes = mapOf("sampleRate" to rate.toString(), "channels" to "1"),
                    destinationIdentities = listOf(Participant.Identity(identity)),
                ),
            )
        } catch (e: Exception) {
            Log.w(TAG, "Could not open the pre-roll", e)
            stop()
            return
        }
        synchronized(lock) { this.sender = sender }
        jobs = jobs + scope.launch { pump(sender) }
    }

    /** The track went out: the pre-roll ends once it has been heard. */
    fun published(room: Room) {
        jobs = jobs + scope.launch {
            room.events.collect { event ->
                if (event is RoomEvent.LocalTrackSubscribed) {
                    delay(TAIL_MS)
                    stop()
                }
            }
        } + scope.launch {
            // LiveKit does not tell a publisher that the (hidden) recorder subscribed.
            delay(LIMIT_MS)
            stop()
        }
    }

    /** Stops listening; whatever was heard is still sent, then the stream closes. */
    fun stop() {
        synchronized(lock) { stopLocked() }
    }

    private fun stopLocked() {
        if (stopped) return
        stopped = true
        runCatching { track.removeSink(this) }
        if (sender == null) jobs.forEach { it.cancel() }
    }

    private suspend fun pump(sender: ByteStreamSender) {
        while (true) {
            val (bytes, done) = synchronized(lock) {
                val bytes = pending.toByteArray()
                pending.reset()
                bytes to stopped
            }
            if (bytes.isNotEmpty() && sender.write(bytes).isFailure) {
                stop()
                break
            }
            if (done) break
            delay(SEND_INTERVAL_MS)
        }
        runCatching { sender.close() }
        jobs.forEach { if (it.isActive) it.cancel() }
    }

    /** Halves a 48 kHz stream with a windowed-sinc low-pass, keeping its state across chunks. */
    private class Halver {
        private val taps = DoubleArray(TAPS) { i ->
            val n = i - (TAPS - 1) / 2.0
            val sinc = if (n == 0.0) 1.0 else sin(PI * n / 2) / (PI * n / 2)
            0.5 * sinc * (0.54 - 0.46 * cos(2 * PI * i / (TAPS - 1)))
        }
        private val history = ShortArray(TAPS - 1)
        private var odd = false

        fun process(input: ShortArray): ShortArray {
            val all = history + input
            val out = ShortArray(input.size / 2 + 1)
            var n = 0
            for (i in input.indices) {
                odd = !odd
                if (!odd) continue
                var acc = 0.0
                for (k in taps.indices) acc += taps[k] * all[i + k]
                out[n++] = acc.coerceIn(Short.MIN_VALUE.toDouble(), Short.MAX_VALUE.toDouble()).toInt().toShort()
            }
            all.copyInto(history, 0, all.size - history.size)
            return out.copyOf(n)
        }

        private companion object {
            const val TAPS = 31
        }
    }

    companion object {
        private const val TAG = "RingPreroll"
        /** LiveKit's topic for pre-connect audio, which ring-media reads. */
        private const val TOPIC = "lk.agent.pre-connect-audio-buffer"
        private const val CAPTURE_RATE = 48_000
        /** After the track is first subscribed the pre-roll runs on this long, so it overlaps the
         * live audio ring-media lines it up with. */
        private const val TAIL_MS = 600L
        /** It ends this long after publishing in any case. */
        private const val LIMIT_MS = 4_000L
        private const val FIRST_AUDIO_TIMEOUT_MS = 1_000L
        private const val SEND_INTERVAL_MS = 100L
        /** The most kept while connecting: 15 s at 24 kHz. */
        private const val MAX_BYTES = 24_000 * 2 * 15

        /** Starts the room's microphone track now and listens to it; null when it cannot start. */
        fun start(room: Room, scope: CoroutineScope): Preroll? = try {
            val track = room.localParticipant.getOrCreateDefaultAudioTrack()
            track.prewarm()
            Preroll(track, scope).also { track.addSink(it) }
        } catch (e: Exception) {
            Log.w(TAG, "Could not start the microphone early", e)
            null
        }
    }
}
