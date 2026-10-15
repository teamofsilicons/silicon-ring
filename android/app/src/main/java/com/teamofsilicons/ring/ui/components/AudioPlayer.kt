package com.teamofsilicons.ring.ui.components

import android.media.AudioAttributes
import android.media.MediaPlayer
import android.util.Log
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.material3.Slider
import androidx.compose.material3.SliderDefaults
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.Stable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import com.teamofsilicons.ring.R
import com.teamofsilicons.ring.ui.Format
import com.teamofsilicons.ring.ui.theme.Ring
import kotlinx.coroutines.delay

/**
 * Streams one voicemail or recording at a time from a presigned URL with [MediaPlayer].
 * [key] identifies what is loaded so list rows can show which item is playing.
 */
@Stable
class AudioPlayerState {
    var key by mutableStateOf<String?>(null)
        private set
    var playing by mutableStateOf(false)
        private set
    var preparing by mutableStateOf(false)
        private set
    var positionMs by mutableIntStateOf(0)
        private set
    var durationMs by mutableIntStateOf(0)
        private set
    var error by mutableStateOf<String?>(null)
        private set

    private var player: MediaPlayer? = null

    fun isPlaying(key: String) = this.key == key && (playing || preparing)

    fun play(key: String, url: String) {
        if (this.key == key && player != null && !preparing) {
            player?.start()
            playing = true
            return
        }
        release()
        this.key = key
        error = null
        preparing = true
        player = MediaPlayer().apply {
            setAudioAttributes(
                AudioAttributes.Builder()
                    .setUsage(AudioAttributes.USAGE_MEDIA)
                    .setContentType(AudioAttributes.CONTENT_TYPE_SPEECH)
                    .build(),
            )
            setOnPreparedListener {
                preparing = false
                durationMs = it.duration.coerceAtLeast(0)
                it.start()
                playing = true
            }
            setOnCompletionListener {
                playing = false
                positionMs = 0
                runCatching { it.seekTo(0) }
            }
            setOnErrorListener { _, what, extra ->
                Log.w("RingPlayer", "Playback error $what/$extra")
                error = "Could not play this audio."
                preparing = false
                playing = false
                true
            }
            try {
                setDataSource(url)
                prepareAsync()
            } catch (e: Exception) {
                error = "Could not play this audio."
                preparing = false
            }
        }
    }

    fun pause() {
        runCatching { player?.pause() }
        playing = false
    }

    fun toggle(key: String, url: () -> String?) {
        if (isPlaying(key)) pause() else url()?.let { play(key, it) }
    }

    fun seekTo(fraction: Float) {
        val p = player ?: return
        if (durationMs <= 0) return
        val target = (durationMs * fraction).toInt()
        runCatching { p.seekTo(target) }
        positionMs = target
    }

    internal fun tick() {
        val p = player ?: return
        if (playing) positionMs = runCatching { p.currentPosition }.getOrDefault(positionMs)
    }

    fun release() {
        player?.let { runCatching { it.release() } }
        player = null
        key = null
        playing = false
        preparing = false
        positionMs = 0
        durationMs = 0
    }
}

@Composable
fun rememberAudioPlayer(): AudioPlayerState {
    val state = remember { AudioPlayerState() }
    DisposableEffect(state) { onDispose { state.release() } }
    LaunchedEffect(state) {
        while (true) {
            state.tick()
            delay(200)
        }
    }
    return state
}

/** Play button, scrubber and times for the loaded item. */
@Composable
fun PlayerBar(state: AudioPlayerState, key: String, fallbackDurationMs: Long?, onPlay: () -> Unit, modifier: Modifier = Modifier) {
    val c = Ring.colors
    val active = state.key == key
    var scrub by remember { mutableFloatStateOf(-1f) }
    val duration = if (active && state.durationMs > 0) state.durationMs.toLong() else fallbackDurationMs ?: 0L
    val position = if (active) state.positionMs.toLong() else 0L
    val fraction = if (scrub >= 0f) scrub else if (duration > 0) (position.toFloat() / duration).coerceIn(0f, 1f) else 0f
    Column(modifier.fillMaxWidth()) {
        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            RoundButton(
                icon = if (state.isPlaying(key)) R.drawable.ms_pause_fill else R.drawable.ms_play_arrow_fill,
                contentDescription = if (state.isPlaying(key)) "Pause" else "Play",
                onClick = { if (state.isPlaying(key)) state.pause() else onPlay() },
                size = 44.dp,
                background = c.accent,
                tint = c.onPrimary,
            )
            Slider(
                value = fraction,
                onValueChange = { scrub = it },
                onValueChangeFinished = {
                    if (active) state.seekTo(scrub)
                    scrub = -1f
                },
                enabled = active && duration > 0,
                colors = SliderDefaults.colors(
                    thumbColor = c.accent,
                    activeTrackColor = c.accent,
                    inactiveTrackColor = c.beige,
                    disabledThumbColor = c.borderStrong,
                    disabledInactiveTrackColor = c.beige,
                ),
                modifier = Modifier.weight(1f),
            )
        }
        Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.SpaceBetween) {
            PixelLabel(Format.duration(if (scrub >= 0) (duration * scrub).toLong() else position))
            PixelLabel(if (duration > 0) Format.duration(duration) else "--:--")
        }
        state.error?.takeIf { active }?.let { PixelLabel(it, color = c.danger, uppercase = false) }
    }
}
