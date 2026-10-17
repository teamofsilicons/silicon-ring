package com.teamofsilicons.ring.ui.components

import android.provider.Settings
import androidx.compose.animation.core.AnimationSpec
import androidx.compose.animation.core.animate
import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.animation.core.LinearEasing
import androidx.compose.animation.core.RepeatMode
import androidx.compose.animation.core.animateFloat
import androidx.compose.animation.core.infiniteRepeatable
import androidx.compose.animation.core.rememberInfiniteTransition
import androidx.compose.animation.core.spring
import androidx.compose.animation.core.tween
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.gestures.Orientation
import androidx.compose.foundation.gestures.draggable
import androidx.compose.foundation.gestures.rememberDraggableState
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.systemGestureExclusion
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.State
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.hapticfeedback.HapticFeedbackType
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.platform.LocalHapticFeedback
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.onClick
import androidx.compose.ui.semantics.role
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.IntOffset
import androidx.compose.ui.unit.dp
import com.teamofsilicons.ring.R
import com.teamofsilicons.ring.ui.theme.RingType
import kotlinx.coroutines.Job
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlin.math.roundToInt

/** True when the system asks for reduced motion (animator duration scale set to 0). */
@Composable
fun reduceMotion(): Boolean {
    val context = LocalContext.current
    return remember {
        Settings.Global.getFloat(context.contentResolver, Settings.Global.ANIMATOR_DURATION_SCALE, 1f) == 0f
    }
}

/**
 * The Ring signature: faint rings expanding from the avatar while a call rings or connects.
 * [content] (the avatar) is drawn on top, centred.
 */
@Composable
fun ExpandingRings(avatarSize: Dp, color: Color, active: Boolean, modifier: Modifier = Modifier, content: @Composable () -> Unit) {
    val still = reduceMotion()
    Box(modifier.size(avatarSize * 2.2f), contentAlignment = Alignment.Center) {
        // The infinite transition lives only while the rings show, so an idle screen requests no frames.
        if (active) {
            val phase = if (still) null else loopingPhase(3_000, RepeatMode.Restart)
            Canvas(Modifier.fillMaxSize()) {
                val base = avatarSize.toPx() / 2f
                val reach = size.minDimension / 2f - base
                val stroke = 1.dp.toPx()
                for (i in 0 until 3) {
                    val t = if (phase == null) (i + 1) / 4f else (phase.value + i / 3f) % 1f
                    val alpha = (1f - t) * 0.35f
                    drawCircle(color.copy(alpha = alpha), radius = base + reach * t, style = Stroke(stroke))
                }
            }
        }
        content()
    }
}

/** A soft concentric pulse around an avatar while that person speaks. */
@Composable
fun SpeakingRing(avatarSize: Dp, color: Color, speaking: Boolean, modifier: Modifier = Modifier, content: @Composable () -> Unit) {
    val still = reduceMotion()
    val ringAlpha by animateFloatAsState(if (speaking) 1f else 0f, tween(200), label = "ringAlpha")
    Box(modifier.size(avatarSize + 24.dp), contentAlignment = Alignment.Center) {
        if (ringAlpha > 0f) {
            val pulse = if (still) null else loopingPhase(1_100, RepeatMode.Reverse)
            Canvas(Modifier.fillMaxSize()) {
                val base = avatarSize.toPx() / 2f
                val grow = pulse?.value ?: 0.5f
                drawCircle(color.copy(alpha = 0.55f * ringAlpha), radius = base + 4.dp.toPx() + 2.dp.toPx() * grow, style = Stroke(2.dp.toPx()))
                drawCircle(color.copy(alpha = 0.22f * ringAlpha * (1f - grow * 0.5f)), radius = base + 9.dp.toPx() + 3.dp.toPx() * grow, style = Stroke(1.dp.toPx()))
            }
        }
        content()
    }
}

/** A 0..1 value that loops for as long as the caller stays in composition; read it while drawing. */
@Composable
private fun loopingPhase(durationMs: Int, mode: RepeatMode): State<Float> =
    rememberInfiniteTransition(label = "loop").animateFloat(
        initialValue = 0f,
        targetValue = 1f,
        animationSpec = infiniteRepeatable(tween(durationMs, easing = LinearEasing), mode),
        label = "phase",
    )

/** How far along the track (0..1) a release has to be to answer. */
private const val ANSWER_AT = 0.85f

/**
 * Slide right to answer: a cream thumb with a phone glyph on a translucent track. Reaching the end
 * answers at once, without lifting the finger, as does releasing past [ANSWER_AT]; anything short
 * of it springs back. Accessibility users get a click action.
 *
 * The thumb follows the finger through plain state that the drag writes directly, and a release
 * past the end answers before any animation starts. (Each drag step used to launch a coroutine
 * that moved an Animatable. The last one ran after the release, cancelled the settle animation
 * and the answer waiting behind it, and the thumb sat at the end without answering.)
 *
 * When [resetKey] changes after an answer (the screen now shows why it failed) the thumb comes
 * back at once; otherwise it comes back after a few seconds if the screen is still up.
 */
@Composable
fun SlideToAnswer(onAnswer: () -> Unit, modifier: Modifier = Modifier, enabled: Boolean = true, resetKey: Any? = null) {
    val cream = Color(0xFFEDE8E0)
    val scope = rememberCoroutineScope()
    val haptics = LocalHapticFeedback.current
    var offset by remember { mutableFloatStateOf(0f) }
    var done by remember { mutableStateOf(false) }
    var armed by remember { mutableStateOf(false) }
    var keyAtAnswer by remember { mutableStateOf<Any?>(null) }
    val settling = remember { arrayOfNulls<Job>(1) }
    val density = LocalDensity.current
    val thumb = 60.dp
    val pad = 6.dp

    fun settle(target: Float, spec: AnimationSpec<Float>) {
        settling[0]?.cancel()
        settling[0] = scope.launch { animate(offset, target, animationSpec = spec) { value, _ -> offset = value } }
    }

    fun answer(): Boolean {
        if (!enabled || done) return false
        done = true
        keyAtAnswer = resetKey
        haptics.performHapticFeedback(HapticFeedbackType.Confirm)
        onAnswer()
        return true
    }

    // An answer that fails (no microphone, a network error) leaves this screen up: let the thumb
    // come back so the person can try again.
    LaunchedEffect(done, resetKey) {
        if (!done) return@LaunchedEffect
        if (resetKey == keyAtAnswer) delay(4_000)
        done = false
        armed = false
        settle(0f, spring(dampingRatio = 1f, stiffness = 500f))
    }
    BoxWithConstraints(
        modifier
            .fillMaxWidth()
            .height(thumb + pad * 2)
            // The thumb starts near the screen's left edge, where gesture navigation would take
            // a swipe as Back.
            .systemGestureExclusion()
            .clip(RoundedCornerShape(50))
            .background(cream.copy(alpha = 0.12f))
            .border(1.dp, cream.copy(alpha = 0.18f), RoundedCornerShape(50))
            .semantics {
                role = Role.Button
                contentDescription = "Answer"
                onClick("Answer") {
                    answer()
                    true
                }
            },
    ) {
        val maxOffset = with(density) { (maxWidth - thumb - pad * 2).toPx() }.coerceAtLeast(1f)
        val progress = (offset / maxOffset).coerceIn(0f, 1f)
        Text(
            if (done) "answering…" else "slide to answer",
            style = RingType.bodyLarge,
            color = cream.copy(alpha = if (done) 0.75f else 0.75f * (1f - progress)),
            modifier = Modifier.align(Alignment.Center).padding(start = thumb / 2),
        )
        Box(
            Modifier
                .padding(pad)
                .offset { IntOffset(offset.roundToInt(), 0) }
                .size(thumb)
                .clip(CircleShape)
                .background(cream)
                .draggable(
                    enabled = enabled && !done,
                    orientation = Orientation.Horizontal,
                    state = rememberDraggableState { delta ->
                        offset = (offset + delta).coerceIn(0f, maxOffset)
                        val past = offset >= maxOffset * ANSWER_AT
                        if (past && !armed) haptics.performHapticFeedback(HapticFeedbackType.GestureThresholdActivate)
                        armed = past
                        if (offset >= maxOffset) answer()
                    },
                    onDragStarted = { settling[0]?.cancel() },
                    onDragStopped = {
                        // Answering at the end disables the drag, which stops it here too.
                        if (done || (offset >= maxOffset * ANSWER_AT && answer())) {
                            settle(maxOffset, tween(120))
                        } else {
                            armed = false
                            settle(0f, spring(dampingRatio = 1f, stiffness = 500f))
                        }
                    },
                ),
            contentAlignment = Alignment.Center,
        ) {
            RingIcon(R.drawable.ms_call_fill, null, tint = Color(0xFF1F7A4D), size = 28.dp, modifier = Modifier.alpha(1f))
        }
    }
}
