package com.teamofsilicons.ring.ui.components

import android.provider.Settings
import androidx.compose.animation.core.Animatable
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
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.State
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
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
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalDensity
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

/**
 * Slide right to answer: a cream thumb with a phone glyph on a translucent track. Releasing past
 * the end answers; anything short of it springs back. Accessibility users get a click action.
 */
@Composable
fun SlideToAnswer(onAnswer: () -> Unit, modifier: Modifier = Modifier, enabled: Boolean = true) {
    val cream = Color(0xFFEDE8E0)
    val scope = rememberCoroutineScope()
    val offset = remember { Animatable(0f) }
    var done by remember { mutableStateOf(false) }
    val density = LocalDensity.current
    val thumb = 60.dp
    val pad = 6.dp
    // An answer that fails (no microphone, a network error) leaves this screen up: let the thumb
    // come back so the person can try again.
    LaunchedEffect(done) {
        if (!done) return@LaunchedEffect
        delay(4_000)
        done = false
        offset.animateTo(0f, spring(dampingRatio = 1f, stiffness = 500f))
    }
    BoxWithConstraints(
        modifier
            .fillMaxWidth()
            .height(thumb + pad * 2)
            .clip(RoundedCornerShape(50))
            .background(cream.copy(alpha = 0.12f))
            .border(1.dp, cream.copy(alpha = 0.18f), RoundedCornerShape(50))
            .semantics {
                role = Role.Button
                contentDescription = "Answer"
                onClick("Answer") {
                    if (enabled && !done) {
                        done = true
                        onAnswer()
                    }
                    true
                }
            },
    ) {
        val maxOffset = with(density) { (maxWidth - thumb - pad * 2).toPx() }.coerceAtLeast(1f)
        val progress = (offset.value / maxOffset).coerceIn(0f, 1f)
        Text(
            "slide to answer",
            style = RingType.bodyLarge,
            color = cream.copy(alpha = 0.75f * (1f - progress)),
            modifier = Modifier.align(Alignment.Center).padding(start = thumb / 2),
        )
        Box(
            Modifier
                .padding(pad)
                .offset { IntOffset(offset.value.roundToInt(), 0) }
                .size(thumb)
                .clip(CircleShape)
                .background(cream)
                .draggable(
                    enabled = enabled && !done,
                    orientation = Orientation.Horizontal,
                    state = rememberDraggableState { delta ->
                        scope.launch { offset.snapTo((offset.value + delta).coerceIn(0f, maxOffset)) }
                    },
                    onDragStopped = {
                        if (offset.value >= maxOffset * 0.85f) {
                            offset.animateTo(maxOffset, tween(120))
                            done = true
                            onAnswer()
                        } else {
                            offset.animateTo(0f, spring(dampingRatio = 1f, stiffness = 500f))
                        }
                    },
                ),
            contentAlignment = Alignment.Center,
        ) {
            RingIcon(R.drawable.ms_call_fill, null, tint = Color(0xFF1F7A4D), size = 28.dp, modifier = Modifier.alpha(1f))
        }
    }
}
