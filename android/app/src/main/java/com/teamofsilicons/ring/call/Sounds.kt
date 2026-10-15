package com.teamofsilicons.ring.call

import android.app.NotificationManager
import android.content.Context
import android.media.AudioAttributes
import android.media.AudioManager
import android.media.MediaPlayer
import android.media.RingtoneManager
import android.media.ToneGenerator
import android.os.Build
import android.os.Handler
import android.os.Looper
import android.os.VibrationAttributes
import android.os.VibrationEffect
import android.os.Vibrator
import android.os.VibratorManager
import android.util.Log
import com.teamofsilicons.ring.R

private const val TAG = "RingSounds"

private val voiceAttributes: AudioAttributes = AudioAttributes.Builder()
    .setUsage(AudioAttributes.USAGE_VOICE_COMMUNICATION)
    .setContentType(AudioAttributes.CONTENT_TYPE_SONIFICATION)
    .build()

private val ringtoneAttributes: AudioAttributes = AudioAttributes.Builder()
    .setUsage(AudioAttributes.USAGE_NOTIFICATION_RINGTONE)
    .setContentType(AudioAttributes.CONTENT_TYPE_SONIFICATION)
    .build()

/** Call-progress sounds played on the voice stream: ringback while ringing and short ticks. */
class Tones(private val context: Context) {
    private var ringback: MediaPlayer? = null

    /** The caller's "tring tring, pause" (3 s cycle) from res/raw/ringback. */
    fun startRingback() {
        if (ringback != null) return
        ringback = create(R.raw.ringback)?.apply {
            isLooping = true
            start()
        }
    }

    fun stopRingback() {
        ringback?.let {
            runCatching { it.stop() }
            it.release()
        }
        ringback = null
    }

    fun connected() = oneShot(R.raw.connected)

    fun ended() = oneShot(R.raw.ended)

    private fun oneShot(res: Int) {
        create(res)?.apply {
            setOnCompletionListener { it.release() }
            start()
        }
    }

    private fun create(res: Int): MediaPlayer? = try {
        val audio = context.getSystemService(AudioManager::class.java)
        MediaPlayer.create(context, res, voiceAttributes, audio.generateAudioSessionId())
    } catch (e: Exception) {
        Log.w(TAG, "Could not play a call tone", e)
        null
    }
}

/**
 * Rings for incoming calls the way the phone app does: the default ringtone (falling back to
 * Ring's own) and vibration, honouring silent, vibrate and Do Not Disturb.
 */
class Ringer(private val context: Context) {
    private var player: MediaPlayer? = null
    private var vibrating = false
    private var waitingTone: ToneGenerator? = null
    private val handler = Handler(Looper.getMainLooper())
    private val waitingBeep = object : Runnable {
        override fun run() {
            waitingTone?.startTone(ToneGenerator.TONE_SUP_CALL_WAITING, 600)
            handler.postDelayed(this, 4_000)
        }
    }

    val ringing: Boolean get() = player != null || vibrating || waitingTone != null

    fun ring() {
        if (player != null || vibrating) return
        stopCallWaiting()
        val audio = context.getSystemService(AudioManager::class.java)
        val notifications = context.getSystemService(NotificationManager::class.java)
        val dndBlocks = when (notifications.currentInterruptionFilter) {
            NotificationManager.INTERRUPTION_FILTER_ALL, NotificationManager.INTERRUPTION_FILTER_UNKNOWN -> false
            // Priority-only Do Not Disturb still lets calls through when it allows calls from anyone.
            NotificationManager.INTERRUPTION_FILTER_PRIORITY -> runCatching {
                val policy = notifications.notificationPolicy
                policy.priorityCategories and NotificationManager.Policy.PRIORITY_CATEGORY_CALLS == 0 ||
                    policy.priorityCallSenders != NotificationManager.Policy.PRIORITY_SENDERS_ANY
            }.getOrDefault(true)
            else -> true
        }
        when (audio.ringerMode) {
            AudioManager.RINGER_MODE_SILENT -> return
            AudioManager.RINGER_MODE_VIBRATE -> if (!dndBlocks) vibrate()
            else -> if (!dndBlocks) {
                playRingtone()
                vibrate()
            }
        }
    }

    /** A soft beep every few seconds instead of a ringtone, while another call is live. */
    fun callWaiting() {
        if (waitingTone != null) return
        stop()
        waitingTone = runCatching { ToneGenerator(AudioManager.STREAM_VOICE_CALL, 60) }.getOrNull()
        handler.post(waitingBeep)
    }

    fun stop() {
        player?.let {
            runCatching { it.stop() }
            it.release()
        }
        player = null
        if (vibrating) vibrator()?.cancel()
        vibrating = false
        stopCallWaiting()
    }

    private fun stopCallWaiting() {
        handler.removeCallbacks(waitingBeep)
        waitingTone?.release()
        waitingTone = null
    }

    private fun playRingtone() {
        val uri = RingtoneManager.getActualDefaultRingtoneUri(context, RingtoneManager.TYPE_RINGTONE)
        player = startLooping(uri) ?: startLooping(null)
    }

    /**
     * Prepares asynchronously: a synchronous prepare() on the main thread can take seconds on a
     * busy device, which would hold up the call service. [uri] null plays Ring's own ringtone.
     */
    private fun startLooping(uri: android.net.Uri?): MediaPlayer? {
        val created = MediaPlayer()
        return try {
            created.apply {
                setAudioAttributes(ringtoneAttributes)
                if (uri != null) setDataSource(context, uri) else setBundledRingtone(this)
                isLooping = true
                setOnPreparedListener { if (player === it) it.start() }
                setOnErrorListener { failed, what, extra ->
                    Log.w(TAG, "Ringtone playback failed ($what/$extra)")
                    if (player === failed) {
                        failed.release()
                        // The system ringtone failed: fall back to Ring's own once.
                        player = if (uri != null) startLooping(null) else null
                    }
                    true
                }
                prepareAsync()
            }
        } catch (e: Exception) {
            Log.w(TAG, "Could not start the ringtone", e)
            created.release()
            null
        }
    }

    private fun setBundledRingtone(player: MediaPlayer) {
        context.resources.openRawResourceFd(R.raw.ringtone).use {
            player.setDataSource(it.fileDescriptor, it.startOffset, it.length)
        }
    }

    private fun vibrate() {
        val vibrator = vibrator() ?: return
        if (!vibrator.hasVibrator()) return
        val effect = VibrationEffect.createWaveform(longArrayOf(0, 700, 400, 700, 1400), 0)
        if (Build.VERSION.SDK_INT >= 33) {
            vibrator.vibrate(effect, VibrationAttributes.createForUsage(VibrationAttributes.USAGE_RINGTONE))
        } else {
            @Suppress("DEPRECATION")
            vibrator.vibrate(effect, ringtoneAttributes)
        }
        vibrating = true
    }

    private fun vibrator(): Vibrator? = if (Build.VERSION.SDK_INT >= 31) {
        context.getSystemService(VibratorManager::class.java)?.defaultVibrator
    } else {
        @Suppress("DEPRECATION")
        context.getSystemService(Vibrator::class.java)
    }
}
