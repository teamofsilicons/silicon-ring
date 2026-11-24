package com.teamofsilicons.ring.ui.call

import android.content.Context
import androidx.annotation.DrawableRes
import androidx.browser.customtabs.CustomTabsIntent
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.shadow
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.clearAndSetSemantics
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.heading
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.compose.ui.window.Dialog
import androidx.compose.ui.window.DialogProperties
import androidx.core.net.toUri
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import com.teamofsilicons.ring.AppGraph
import com.teamofsilicons.ring.BuildConfig
import com.teamofsilicons.ring.R
import com.teamofsilicons.ring.call.RecordingNotice
import com.teamofsilicons.ring.ui.components.PixelLabel
import com.teamofsilicons.ring.ui.components.PrimaryButton
import com.teamofsilicons.ring.ui.components.RingIcon
import com.teamofsilicons.ring.ui.components.TextAction
import com.teamofsilicons.ring.ui.components.hairline
import com.teamofsilicons.ring.ui.theme.Ring
import com.teamofsilicons.ring.ui.theme.RingType

/** The words of the recording notice and its indicator, the same on every Ring client. */
private object RecordingCopy {
    const val TITLE = "Ring records calls"
    const val INDICATOR = "Recorded · Transcribed"
    const val AGREE = "Agree and continue"
    const val NOT_NOW = "Not now"
    const val PRIVACY_POLICY = "Privacy Policy"
    const val BODY = "Every call is recorded and transcribed, so the people on it can read it and play it back later. Everyone on a call sees the same transcript."
    const val DEEPGRAM = "Deepgram transcribes what people say."
    const val OPENAI = "When a silicon is on the call, OpenAI produces its voice and hears the call."
    const val CONSENT = "Some places require everyone's consent to record a call. Let the people you call know."
    const val OVER_CALL = "This call is recorded and transcribed, like every Ring call. Your microphone stays off until you agree; Not now ends the call."
    const val AFTER_CALL = "Your last call was recorded and transcribed, like every Ring call."
}

/**
 * Runs a call action once this device has agreed to the recording notice, showing the notice
 * first until it has (RingRoot shows it, or CallActivity for answering a ring). "Not now" drops
 * the action. A notice still up when this screen goes away is withdrawn with it, since the action
 * belongs to this screen.
 */
@Composable
fun rememberRecordingNotice(graph: AppGraph): (RecordingNotice.Action, () -> Unit) -> Unit {
    val notice = graph.recordingNotice
    val owner = remember { Any() }
    DisposableEffect(notice, owner) { onDispose { notice.withdraw(owner) } }
    return remember(notice, owner) { { action, then -> notice.before(action, owner, then) } }
}

/**
 * The recording notice over the app: after sign-in, and before placing a call or moving one here
 * until this device agrees. Answering a ring shows it on the call screen instead, and so does a
 * call that went on before this device agreed, while it is live.
 */
@Composable
fun RecordingNoticeHost(graph: AppGraph) {
    val request by graph.recordingNotice.request.collectAsStateWithLifecycle()
    val calls by graph.calls.calls.collectAsStateWithLifecycle()
    val action = request?.action ?: return
    if (action is RecordingNotice.Action.Answer) return
    if (action == RecordingNotice.Action.Answered && calls.any { it.live && it.waitsForNotice }) return
    RecordingNoticeDialog(action, onAgree = graph.recordingNotice::agree, onNotNow = graph.recordingNotice::notNow)
}

/** The notice in its own window; nothing underneath responds until it is answered. Back is "Not now". */
@Composable
fun RecordingNoticeDialog(action: RecordingNotice.Action, onAgree: () -> Unit, onNotNow: () -> Unit) {
    Dialog(
        onDismissRequest = onNotNow,
        properties = DialogProperties(dismissOnClickOutside = false, usePlatformDefaultWidth = false),
    ) {
        Box(Modifier.fillMaxWidth().padding(16.dp), contentAlignment = Alignment.Center) {
            NoticeCard(
                context = if (action == RecordingNotice.Action.Answered) RecordingCopy.AFTER_CALL else null,
                onAgree = onAgree,
                onNotNow = onNotNow,
            )
        }
    }
}

/**
 * Over a call that went on before this device agreed, with its microphone held: covers the people
 * and the transcript and leaves the controls below it usable. "Agree and continue" turns the
 * microphone on; "Not now" ends the call.
 */
@Composable
fun RecordingNoticeOverCall(onAgree: () -> Unit, onNotNow: () -> Unit, modifier: Modifier = Modifier) {
    Box(
        modifier
            .fillMaxSize()
            .background(Color.Black.copy(alpha = 0.4f))
            // Taps outside the card don't reach the call underneath.
            .pointerInput(Unit) { detectTapGestures {} },
        contentAlignment = Alignment.Center,
    ) {
        NoticeCard(context = RecordingCopy.OVER_CALL, onAgree = onAgree, onNotNow = onNotNow, modifier = Modifier.padding(vertical = 16.dp))
    }
}

/** On the outgoing and in-call screens: the call is recorded and transcribed. */
@Composable
fun RecordingIndicator(modifier: Modifier = Modifier) {
    val c = Ring.colors
    Row(
        modifier.clearAndSetSemantics { contentDescription = "This call is recorded and transcribed" },
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Box(Modifier.size(6.dp).clip(CircleShape).background(c.danger))
        Spacer(Modifier.width(6.dp))
        PixelLabel(RecordingCopy.INDICATOR, color = c.textMuted, style = RingType.pixelSmall)
    }
}

@Composable
private fun NoticeCard(context: String?, onAgree: () -> Unit, onNotNow: () -> Unit, modifier: Modifier = Modifier) {
    val c = Ring.colors
    val android = LocalContext.current
    val shape = RoundedCornerShape(16.dp)
    Column(
        modifier
            .widthIn(max = 480.dp)
            .fillMaxWidth()
            .shadow(if (c.dark) 0.dp else 8.dp, shape, ambientColor = Color.Black.copy(alpha = 0.12f), spotColor = Color.Black.copy(alpha = 0.12f))
            .clip(shape)
            .background(c.surfaceRaised)
            .border(hairline, c.border, shape)
            .verticalScroll(rememberScrollState())
            .padding(20.dp),
        verticalArrangement = Arrangement.spacedBy(16.dp),
    ) {
        Row(Modifier.clearAndSetSemantics {}, verticalAlignment = Alignment.CenterVertically) {
            Box(Modifier.size(8.dp).clip(CircleShape).background(c.danger))
            Spacer(Modifier.width(8.dp))
            PixelLabel(RecordingCopy.INDICATOR, style = RingType.pixelSmall)
        }
        Text(RecordingCopy.TITLE, style = RingType.pageTitle, color = c.ink, modifier = Modifier.semantics { heading() })
        context?.let { Text(it, style = RingType.body.copy(fontWeight = FontWeight.Medium), color = c.foreground) }
        Text(RecordingCopy.BODY, style = RingType.body, color = c.foreground)
        Column(verticalArrangement = Arrangement.spacedBy(10.dp)) {
            NoticeItem(R.drawable.ms_mic, RecordingCopy.DEEPGRAM)
            NoticeItem(R.drawable.ms_volume_up, RecordingCopy.OPENAI)
        }
        Text(RecordingCopy.CONSENT, style = RingType.bodySmall.copy(fontSize = 13.sp, lineHeight = 18.sp), color = c.textMuted)
        Row(
            Modifier
                .offset(x = (-10).dp)
                .clip(RoundedCornerShape(8.dp))
                .clickable(role = Role.Button) { openUrl(android, BuildConfig.PRIVACY_URL) }
                .padding(horizontal = 10.dp, vertical = 8.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Text(RecordingCopy.PRIVACY_POLICY, style = RingType.button, color = c.accentText)
            Spacer(Modifier.width(6.dp))
            RingIcon(R.drawable.ms_open_in_new, null, tint = c.accentText, size = 16.dp)
        }
        Column(Modifier.padding(top = 4.dp), verticalArrangement = Arrangement.spacedBy(10.dp)) {
            PrimaryButton(RecordingCopy.AGREE, onClick = onAgree, modifier = Modifier.fillMaxWidth())
            Box(Modifier.fillMaxWidth(), contentAlignment = Alignment.Center) {
                TextAction(RecordingCopy.NOT_NOW, onNotNow, color = c.textSecondary)
            }
        }
    }
}

@Composable
private fun NoticeItem(@DrawableRes icon: Int, text: String) {
    val c = Ring.colors
    Row(horizontalArrangement = Arrangement.spacedBy(10.dp)) {
        RingIcon(icon, null, Modifier.padding(top = 2.dp), tint = c.accentText, size = 18.dp)
        Text(text, style = RingType.body, color = c.textSecondary, modifier = Modifier.weight(1f))
    }
}

private fun openUrl(context: Context, url: String) {
    runCatching { CustomTabsIntent.Builder().setShowTitle(true).build().launchUrl(context, url.toUri()) }
}
