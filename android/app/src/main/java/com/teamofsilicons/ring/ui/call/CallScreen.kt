package com.teamofsilicons.ring.ui.call

import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.animation.core.tween
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.expandVertically
import androidx.compose.animation.shrinkVertically
import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.safeDrawingPadding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.OutlinedTextFieldDefaults
import androidx.compose.material3.Text
import androidx.compose.material3.rememberModalBottomSheetState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.drawWithContent
import androidx.compose.ui.graphics.BlendMode
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.CompositingStrategy
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.core.telecom.CallEndpointCompat
import com.teamofsilicons.ring.AppGraph
import com.teamofsilicons.ring.R
import com.teamofsilicons.ring.call.AudioRoute
import com.teamofsilicons.ring.call.CallPhase
import com.teamofsilicons.ring.call.CallUi
import com.teamofsilicons.ring.call.TranscriptLine
import com.teamofsilicons.ring.data.Account
import com.teamofsilicons.ring.ui.Format
import com.teamofsilicons.ring.ui.components.Avatar
import com.teamofsilicons.ring.ui.components.ExpandingRings
import com.teamofsilicons.ring.ui.components.IconAction
import com.teamofsilicons.ring.ui.components.PixelId
import com.teamofsilicons.ring.ui.components.PixelLabel
import com.teamofsilicons.ring.ui.components.PrimaryButton
import com.teamofsilicons.ring.ui.components.RingIcon
import com.teamofsilicons.ring.ui.components.RoundButton
import com.teamofsilicons.ring.ui.components.SiliconBadge
import com.teamofsilicons.ring.ui.components.SlideToAnswer
import com.teamofsilicons.ring.ui.components.SpeakingRing
import com.teamofsilicons.ring.ui.components.TextAction
import com.teamofsilicons.ring.ui.dialer.DialerContent
import com.teamofsilicons.ring.ui.theme.MarkColors
import com.teamofsilicons.ring.ui.theme.Ring
import com.teamofsilicons.ring.ui.theme.RingType
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import java.time.Instant

@Composable
fun CallScreen(ui: CallUi, graph: AppGraph, onAnswer: () -> Unit, onMinimize: () -> Unit) {
    if (ui.phase == CallPhase.Incoming) {
        IncomingCallScreen(
            ui = ui,
            onAnswer = onAnswer,
            onDecline = { reason -> graph.calls.decline(ui.ringid, reason) },
            onSilence = { graph.calls.silence(ui.ringid) },
        )
    } else {
        ActiveCallScreen(ui, graph, onMinimize)
    }
}

private val declineReasons = listOf("Can't talk right now", "In a meeting", "Call you back soon", "Text me instead")

/** Incoming call: ink in both themes, slide to answer, decline with optional reasons, silence. */
@OptIn(ExperimentalLayoutApi::class)
@Composable
fun IncomingCallScreen(
    ui: CallUi,
    onAnswer: () -> Unit,
    onDecline: (String?) -> Unit,
    onSilence: () -> Unit,
    startWithReasons: Boolean = false,
) {
    val cream = MarkColors.cream
    val muted = MarkColors.creamMuted
    val danger = Color(0xFFF2706A)
    var declineOpen by remember { mutableStateOf(startWithReasons) }
    var custom by remember { mutableStateOf("") }

    Column(
        Modifier
            .fillMaxSize()
            .background(MarkColors.ink)
            .safeDrawingPadding()
            .imePadding()
            .padding(horizontal = 24.dp),
        horizontalAlignment = Alignment.CenterHorizontally,
    ) {
        Spacer(Modifier.height(24.dp))
        PixelLabel("Ring", color = muted)
        Spacer(Modifier.weight(1f))
        ExpandingRings(120.dp, cream, active = !declineOpen) { Avatar(ui.peer, 120.dp) }
        Spacer(Modifier.height(8.dp))
        CallerIdentity(ui.peer, cream, muted, 36)
        Spacer(Modifier.height(18.dp))
        PixelLabel(if (ui.silenced) "Silenced · is calling" else "Is calling", color = cream, style = RingType.pixelLarge)
        if (ui.alreadyOn.isNotEmpty()) {
            Spacer(Modifier.height(6.dp))
            PixelLabel("With " + ui.alreadyOn.joinToString(", ") { it.label }, color = muted)
        }
        ui.notice?.let {
            Spacer(Modifier.height(12.dp))
            Text(it, style = RingType.bodySmall, color = danger, textAlign = TextAlign.Center)
        }
        Spacer(Modifier.weight(1f))

        AnimatedVisibility(declineOpen, enter = fadeIn() + expandVertically(), exit = fadeOut() + shrinkVertically()) {
            Column(Modifier.fillMaxWidth().padding(bottom = 20.dp)) {
                PixelLabel("Decline with a reason", color = muted)
                Spacer(Modifier.height(10.dp))
                FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                    declineReasons.forEach { reason ->
                        Text(
                            reason,
                            style = RingType.bodySmall,
                            color = cream,
                            modifier = Modifier
                                .clip(RoundedCornerShape(16.dp))
                                .background(cream.copy(alpha = 0.10f))
                                .border(1.dp, cream.copy(alpha = 0.18f), RoundedCornerShape(16.dp))
                                .clickable { onDecline(reason) }
                                .padding(horizontal = 14.dp, vertical = 9.dp),
                        )
                    }
                }
                Spacer(Modifier.height(12.dp))
                OutlinedTextField(
                    value = custom,
                    onValueChange = { if (it.codePointCount(0, it.length) <= 200) custom = it },
                    singleLine = true,
                    placeholder = { Text("Something else…", style = RingType.body, color = muted) },
                    textStyle = RingType.body.copy(color = cream),
                    shape = RoundedCornerShape(8.dp),
                    keyboardOptions = KeyboardOptions(imeAction = ImeAction.Send),
                    keyboardActions = KeyboardActions(onSend = { if (custom.isNotBlank()) onDecline(custom.trim()) }),
                    trailingIcon = {
                        IconAction(R.drawable.ms_send, "Decline with this reason", { if (custom.isNotBlank()) onDecline(custom.trim()) }, tint = if (custom.isBlank()) muted else cream, enabled = custom.isNotBlank())
                    },
                    colors = OutlinedTextFieldDefaults.colors(
                        focusedBorderColor = cream.copy(alpha = 0.5f),
                        unfocusedBorderColor = cream.copy(alpha = 0.2f),
                        cursorColor = cream,
                        focusedContainerColor = Color.Transparent,
                        unfocusedContainerColor = Color.Transparent,
                    ),
                    modifier = Modifier.fillMaxWidth(),
                )
            }
        }

        SlideToAnswer(onAnswer = onAnswer, enabled = !ui.working, modifier = Modifier.widthIn(max = 420.dp))
        Spacer(Modifier.height(20.dp))
        Row(Modifier.fillMaxWidth().widthIn(max = 420.dp), verticalAlignment = Alignment.CenterVertically) {
            if (!ui.silenced) TextAction("Silence", onSilence, color = cream) else Spacer(Modifier.size(1.dp))
            Spacer(Modifier.weight(1f))
            Column(horizontalAlignment = Alignment.CenterHorizontally) {
                RoundButton(
                    icon = R.drawable.ms_call_end_fill,
                    contentDescription = if (declineOpen) "Decline without a reason" else "Decline",
                    onClick = { if (declineOpen) onDecline(null) else declineOpen = true },
                    size = 64.dp,
                    background = Color(0xFFC8322B),
                    tint = Color.White,
                )
                Spacer(Modifier.height(6.dp))
                PixelLabel(if (declineOpen) "No reason" else "Decline", color = muted, style = RingType.pixelSmall)
            }
        }
        Spacer(Modifier.height(24.dp))
    }
}

@Composable
private fun CallerIdentity(account: Account, nameColor: Color, idColor: Color, nameSize: Int) {
    Column(horizontalAlignment = Alignment.CenterHorizontally) {
        Text(
            account.label,
            style = RingType.displayName.copy(fontSize = nameSize.sp, lineHeight = (nameSize * 1.18f).sp),
            color = nameColor,
            textAlign = TextAlign.Center,
            maxLines = 2,
            overflow = TextOverflow.Ellipsis,
        )
        Spacer(Modifier.height(6.dp))
        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            if (account.displayName?.isNotBlank() == true) PixelId(account.id, color = idColor, style = RingType.pixelLarge)
            SiliconBadge(account)
        }
    }
}

/** Outgoing, voicemail and in-call screen. Paper in light mode, ink in dark mode. */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun ActiveCallScreen(ui: CallUi, graph: AppGraph, onMinimize: () -> Unit) {
    val c = Ring.colors
    var now by remember { mutableStateOf(Instant.now()) }
    var inviting by remember { mutableStateOf(false) }
    LaunchedEffect(ui.phase) {
        while (ui.phase == CallPhase.Active) {
            now = Instant.now()
            delay(1_000)
        }
    }
    val status = when (ui.phase) {
        CallPhase.Calling -> "Calling…"
        CallPhase.Ringing -> "Ringing"
        CallPhase.Connecting -> "Connecting…"
        CallPhase.Active -> ui.notice ?: Format.elapsed(ui.answeredAt, now)
        CallPhase.Voicemail -> "Voicemail"
        CallPhase.Ended -> ui.endedText ?: "Call ended"
        CallPhase.Incoming -> ""
    }

    Column(
        Modifier
            .fillMaxSize()
            .background(c.background)
            .safeDrawingPadding()
            .padding(horizontal = 20.dp),
        horizontalAlignment = Alignment.CenterHorizontally,
    ) {
        Row(Modifier.fillMaxWidth().padding(top = 8.dp), verticalAlignment = Alignment.CenterVertically) {
            IconAction(R.drawable.ms_arrow_back, "Back to Ring", onMinimize, tint = c.textSecondary)
            Spacer(Modifier.weight(1f))
            PixelLabel(status, color = if (ui.phase == CallPhase.Ended) c.textMuted else c.foreground, style = RingType.pixelLarge)
            Spacer(Modifier.weight(1f))
            Spacer(Modifier.size(40.dp))
        }

        if (ui.phase == CallPhase.Active) {
            Spacer(Modifier.height(20.dp))
            Participants(ui)
            Spacer(Modifier.height(12.dp))
            Transcript(ui.transcript, Modifier.weight(1f).fillMaxWidth())
        } else {
            Spacer(Modifier.weight(1f))
            val ringing = ui.phase == CallPhase.Calling || ui.phase == CallPhase.Ringing || ui.phase == CallPhase.Connecting
            ExpandingRings(120.dp, if (ui.phase == CallPhase.Voicemail) c.danger else c.ink, active = ringing || ui.phase == CallPhase.Voicemail) {
                Avatar(ui.peer, 120.dp)
            }
            Spacer(Modifier.height(8.dp))
            CallerIdentity(ui.peer, c.foreground, c.textMuted, 36)
            if (ui.phase == CallPhase.Voicemail) {
                Spacer(Modifier.height(18.dp))
                ui.notice?.let { PixelLabel(it, color = c.textMuted) }
                Spacer(Modifier.height(6.dp))
                PixelLabel("Leave a message after the beep", color = c.danger, style = RingType.pixelLarge)
            } else if (ui.notice != null && ui.phase != CallPhase.Ended) {
                Spacer(Modifier.height(14.dp))
                PixelLabel(ui.notice, color = c.textMuted)
            }
            Spacer(Modifier.weight(1f))
        }

        if (ui.phase != CallPhase.Ended) {
            Spacer(Modifier.height(16.dp))
            Controls(ui, graph, onInvite = { inviting = true })
        } else {
            Spacer(Modifier.height(96.dp))
        }
        Spacer(Modifier.height(28.dp))
    }

    if (inviting) {
        val sheet = rememberModalBottomSheetState(skipPartiallyExpanded = true)
        val scope = rememberCoroutineScope()
        var error by remember { mutableStateOf<String?>(null) }
        var busy by remember { mutableStateOf(false) }
        ModalBottomSheet(
            onDismissRequest = { inviting = false },
            sheetState = sheet,
            containerColor = c.surfaceRaised,
            shape = RoundedCornerShape(topStart = 12.dp, topEnd = 12.dp),
        ) {
            Box(Modifier.fillMaxWidth().height(560.dp)) {
                DialerContent(
                    graph = graph,
                    title = "Add person",
                    actionLabel = "Add",
                    prefill = null,
                    externalError = error,
                    busy = busy,
                    onClose = { inviting = false },
                    exclude = ui.others.map { it.uuid }.toSet() + setOfNotNull(graph.repo.myUuid),
                    onPick = { account ->
                        busy = true
                        scope.launch {
                            error = graph.calls.invite(ui.ringid, account.id)
                            busy = false
                            if (error == null) inviting = false
                        }
                    },
                )
            }
        }
    }
}

@Composable
private fun Participants(ui: CallUi) {
    val c = Ring.colors
    val people = ui.others.filter { other -> ui.call?.participants?.any { it.account.uuid == other.uuid && it.joined } ?: (other.uuid == ui.peer.uuid) }
        .ifEmpty { listOf(ui.peer) }
    val size: Dp = when (people.size) {
        1 -> 88.dp
        2 -> 72.dp
        else -> 56.dp
    }
    Row(horizontalArrangement = Arrangement.spacedBy(12.dp, Alignment.CenterHorizontally), modifier = Modifier.fillMaxWidth()) {
        people.take(4).forEach { person ->
            Column(horizontalAlignment = Alignment.CenterHorizontally, modifier = Modifier.weight(1f, fill = false).widthIn(max = 160.dp)) {
                SpeakingRing(size, c.accent, speaking = person.uuid in ui.speaking) { Avatar(person, size) }
                Text(
                    person.label,
                    style = RingType.serifName.copy(fontSize = if (people.size == 1) 26.sp else 18.sp, lineHeight = if (people.size == 1) 31.sp else 21.sp),
                    color = c.foreground,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                    textAlign = TextAlign.Center,
                )
                if (people.size == 1) {
                    Spacer(Modifier.height(4.dp))
                    Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                        if (person.displayName?.isNotBlank() == true) PixelId(person.id)
                        SiliconBadge(person)
                    }
                }
            }
        }
    }
    val invited = ui.call?.invitations?.filter { it.ringing }.orEmpty()
    if (invited.isNotEmpty()) {
        Spacer(Modifier.height(8.dp))
        PixelLabel("Ringing " + invited.joinToString(", ") { it.to.label })
    }
}

/** The live transcript: new lines enter at the bottom, move up and fade toward the top. */
@Composable
private fun Transcript(lines: List<TranscriptLine>, modifier: Modifier) {
    val c = Ring.colors
    val state = rememberLazyListState()
    LaunchedEffect(lines.size, lines.lastOrNull()?.text) {
        // Item 0 is the top spacer, so the newest line sits at index lines.size.
        if (lines.isNotEmpty()) state.animateScrollToItem(lines.size)
    }
    if (lines.isEmpty()) {
        Box(modifier, contentAlignment = Alignment.Center) {
            Text("The transcript appears here as people speak.", style = RingType.bodySmall, color = c.textMuted, textAlign = TextAlign.Center)
        }
        return
    }
    LazyColumn(
        state = state,
        verticalArrangement = Arrangement.spacedBy(14.dp, Alignment.Bottom),
        modifier = modifier
            .graphicsLayer(compositingStrategy = CompositingStrategy.Offscreen)
            .drawWithContent {
                drawContent()
                drawRect(
                    Brush.verticalGradient(0f to Color.Transparent, 0.45f to Color.Black, 1f to Color.Black),
                    blendMode = BlendMode.DstIn,
                )
            },
    ) {
        item { Spacer(Modifier.height(120.dp)) }
        items(lines, key = { it.key }) { line ->
            val alpha by animateFloatAsState(if (line.final) 1f else 0.6f, tween(150), label = "interim")
            Column(Modifier.fillMaxWidth().animateItem()) {
                if (line.system) {
                    PixelLabel(line.text, color = c.textMuted)
                } else {
                    PixelLabel(line.speaker?.label ?: "Someone", color = c.textMuted)
                    Spacer(Modifier.height(2.dp))
                    Text(line.text, style = RingType.transcript, color = if (line.final) c.foreground else c.textMuted.copy(alpha = alpha))
                }
            }
        }
    }
}

@Composable
private fun Controls(ui: CallUi, graph: AppGraph, onInvite: () -> Unit) {
    val c = Ring.colors
    val idle = if (c.dark) c.beige else c.surface
    val outline = BorderStroke(1.dp, c.border)
    if (ui.phase == CallPhase.Voicemail) {
        Row(horizontalArrangement = Arrangement.spacedBy(20.dp), verticalAlignment = Alignment.CenterVertically) {
            RoundButton(
                icon = if (ui.muted) R.drawable.ms_mic_off_fill else R.drawable.ms_mic,
                contentDescription = if (ui.muted) "Unmute" else "Mute",
                onClick = { graph.calls.setMuted(ui.ringid, !ui.muted) },
                background = if (ui.muted) c.ink else idle,
                tint = if (ui.muted) c.background else c.foreground,
                border = if (ui.muted) null else outline,
            )
            PrimaryButton("Done", onClick = { graph.calls.hangUp(ui.ringid) }, icon = R.drawable.ms_check, modifier = Modifier.widthIn(min = 160.dp))
        }
        return
    }
    Row(horizontalArrangement = Arrangement.spacedBy(18.dp), verticalAlignment = Alignment.CenterVertically) {
        RoundButton(
            icon = if (ui.muted) R.drawable.ms_mic_off_fill else R.drawable.ms_mic,
            contentDescription = if (ui.muted) "Unmute" else "Mute",
            onClick = { graph.calls.setMuted(ui.ringid, !ui.muted) },
            background = if (ui.muted) c.ink else idle,
            tint = if (ui.muted) c.background else c.foreground,
            border = if (ui.muted) null else outline,
        )
        RouteButton(ui, idle, outline) { graph.calls.setRoute(ui.ringid, it) }
        if (ui.phase == CallPhase.Active) {
            RoundButton(R.drawable.ms_person_add, "Add person", onInvite, background = idle, tint = c.foreground, border = outline)
        }
        RoundButton(
            icon = R.drawable.ms_call_end_fill,
            contentDescription = "Hang up",
            onClick = { graph.calls.hangUp(ui.ringid) },
            size = 64.dp,
            background = Color(0xFFC8322B),
            tint = Color.White,
        )
    }
}

/** Speaker toggle, or a list of routes when a headset or Bluetooth device is connected. */
@Composable
private fun RouteButton(ui: CallUi, idle: Color, outline: BorderStroke, onSelect: (AudioRoute) -> Unit) {
    val c = Ring.colors
    var open by remember { mutableStateOf(false) }
    val type = ui.route?.type ?: CallEndpointCompat.TYPE_EARPIECE
    val speaker = type == CallEndpointCompat.TYPE_SPEAKER
    val icon = when (type) {
        CallEndpointCompat.TYPE_BLUETOOTH -> R.drawable.ms_bluetooth
        CallEndpointCompat.TYPE_WIRED_HEADSET -> R.drawable.ms_headphones
        CallEndpointCompat.TYPE_SPEAKER -> R.drawable.ms_volume_up
        else -> R.drawable.ms_phone_in_talk
    }
    val rich = ui.routes.any { it.type == CallEndpointCompat.TYPE_BLUETOOTH || it.type == CallEndpointCompat.TYPE_WIRED_HEADSET } || ui.routes.size > 2
    Box {
        RoundButton(
            icon = icon,
            contentDescription = "Audio route: ${ui.route?.name ?: "phone"}",
            enabled = ui.routes.isNotEmpty(),
            onClick = {
                if (rich) {
                    open = true
                } else {
                    val target = ui.routes.firstOrNull { if (speaker) it.type != CallEndpointCompat.TYPE_SPEAKER else it.type == CallEndpointCompat.TYPE_SPEAKER }
                    target?.let(onSelect)
                }
            },
            background = if (speaker) c.ink else idle,
            tint = if (speaker) c.background else c.foreground,
            border = if (speaker) null else outline,
        )
        DropdownMenu(expanded = open, onDismissRequest = { open = false }, containerColor = c.surfaceRaised) {
            ui.routes.forEach { route ->
                DropdownMenuItem(
                    text = { Text(route.name, style = RingType.body, color = c.foreground) },
                    leadingIcon = {
                        RingIcon(
                            when (route.type) {
                                CallEndpointCompat.TYPE_BLUETOOTH -> R.drawable.ms_bluetooth
                                CallEndpointCompat.TYPE_WIRED_HEADSET -> R.drawable.ms_headphones
                                CallEndpointCompat.TYPE_SPEAKER -> R.drawable.ms_volume_up
                                else -> R.drawable.ms_phone_in_talk
                            },
                            null,
                            size = 20.dp,
                        )
                    },
                    trailingIcon = { if (route.endpoint.identifier == ui.route?.endpoint?.identifier) RingIcon(R.drawable.ms_check, "Current", tint = c.accent, size = 18.dp) },
                    onClick = {
                        open = false
                        onSelect(route)
                    },
                )
            }
        }
    }
}
