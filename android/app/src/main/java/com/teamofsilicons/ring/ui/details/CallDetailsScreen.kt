package com.teamofsilicons.ring.ui.details

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.IntrinsicSize
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.safeDrawingPadding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.Text
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
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import com.teamofsilicons.ring.AppGraph
import com.teamofsilicons.ring.R
import com.teamofsilicons.ring.data.Account
import com.teamofsilicons.ring.data.ApiException
import com.teamofsilicons.ring.data.Call
import com.teamofsilicons.ring.data.EntryKind
import com.teamofsilicons.ring.data.InvitationState
import com.teamofsilicons.ring.data.Recording
import com.teamofsilicons.ring.data.RingJson
import com.teamofsilicons.ring.data.TranscriptEntry
import com.teamofsilicons.ring.ui.Format
import com.teamofsilicons.ring.ui.Navigator
import com.teamofsilicons.ring.ui.Route
import com.teamofsilicons.ring.ui.components.Avatar
import com.teamofsilicons.ring.ui.components.CardGroup
import com.teamofsilicons.ring.ui.components.Divider
import com.teamofsilicons.ring.ui.components.GroupRow
import com.teamofsilicons.ring.ui.components.IconAction
import com.teamofsilicons.ring.ui.components.PixelId
import com.teamofsilicons.ring.ui.components.PixelLabel
import com.teamofsilicons.ring.ui.components.PlayerBar
import com.teamofsilicons.ring.ui.components.PrimaryButton
import com.teamofsilicons.ring.ui.components.SectionHeader
import com.teamofsilicons.ring.ui.components.SiliconBadge
import com.teamofsilicons.ring.ui.components.hairline
import com.teamofsilicons.ring.ui.components.rememberAudioPlayer
import com.teamofsilicons.ring.ui.rememberCallPermissions
import com.teamofsilicons.ring.ui.theme.Ring
import com.teamofsilicons.ring.ui.theme.RingType
import kotlinx.coroutines.launch

@Composable
fun CallDetailsScreen(graph: AppGraph, nav: Navigator, ringid: String) {
    val c = Ring.colors
    val scope = rememberCoroutineScope()
    val permissions = rememberCallPermissions()
    val player = rememberAudioPlayer()
    val me = graph.repo.myUuid.orEmpty()
    var call by remember { mutableStateOf(graph.repo.calls.value.items.firstOrNull { it.ringid == ringid }) }
    var entries by remember { mutableStateOf<List<TranscriptEntry>>(emptyList()) }
    var recording by remember { mutableStateOf<Recording?>(null) }
    var error by remember { mutableStateOf<String?>(null) }
    var loading by remember { mutableStateOf(true) }

    LaunchedEffect(ringid) {
        try {
            val fresh = graph.api.call(ringid)
            call = fresh
            graph.repo.upsertCall(fresh)
            entries = graph.api.fullTranscript(ringid)
            if (fresh.recording.status != "none") recording = runCatching { graph.api.recording(ringid) }.getOrNull()
        } catch (e: ApiException) {
            error = e.userMessage
        }
        loading = false
    }

    Column(Modifier.fillMaxSize().safeDrawingPadding()) {
        Row(Modifier.fillMaxWidth().padding(horizontal = 8.dp, vertical = 4.dp), verticalAlignment = Alignment.CenterVertically) {
            IconAction(R.drawable.ms_arrow_back, "Back", { nav.pop() }, tint = c.foreground)
            Text("Call details", style = RingType.heading, color = c.foreground)
        }
        val current = call
        if (current == null) {
            Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) {
                if (loading) CircularProgressIndicator(color = c.accent) else Text(error ?: "Call not found", style = RingType.body, color = c.textMuted)
            }
            return@Column
        }
        val people = Format.counterpart(current, me)
        val outcome = Format.outcome(current, me)
        Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(horizontal = 16.dp).padding(bottom = 40.dp)) {
            Spacer(Modifier.height(8.dp))
            Text(
                people.joinToString(", ") { it.label },
                style = RingType.pageTitle,
                color = c.foreground,
                maxLines = 2,
                overflow = TextOverflow.Ellipsis,
            )
            Spacer(Modifier.height(6.dp))
            PixelLabel("${Format.whenFull(current.createdAt)} · ${outcome.label}", color = if (outcome.missed) c.danger else c.textMuted)
            Spacer(Modifier.height(16.dp))
            PrimaryButton(
                "Call ${people.first().label}",
                icon = R.drawable.ms_call_fill,
                onClick = {
                    permissions.withMicrophone { ok ->
                        if (!ok) {
                            error = "Ring needs the microphone to place calls."
                            return@withMicrophone
                        }
                        scope.launch {
                            error = graph.calls.placeCall(people.first().id)
                            if (error == null) graph.calls.openCallScreen()
                        }
                    }
                },
            )
            error?.let { Text(it, style = RingType.bodySmall, color = c.danger, modifier = Modifier.padding(top = 10.dp)) }

            SectionHeader("Participants")
            CardGroup {
                val everyone = (listOf(current.createdBy) + current.participants.map { it.account } + current.invitations.map { it.to }).distinctBy { it.uuid }
                everyone.forEachIndexed { index, account ->
                    if (index > 0) Divider(inset = 64.dp)
                    ParticipantRow(current, account, me)
                }
            }

            current.invitations.filter { it.state == InvitationState.DECLINED && !it.reason.isNullOrBlank() }.takeIf { it.isNotEmpty() }?.let { declined ->
                SectionHeader("Decline reasons")
                CardGroup {
                    declined.forEachIndexed { index, invitation ->
                        if (index > 0) Divider(inset = 16.dp)
                        GroupRow {
                            Column {
                                PixelLabel(invitation.to.label)
                                Text(invitation.reason.orEmpty(), style = RingType.body, color = c.foreground)
                            }
                        }
                    }
                }
            }

            recording?.takeIf { it.status == "ready" && it.url != null }?.let { rec ->
                SectionHeader("Recording")
                CardGroup {
                    Box(Modifier.padding(16.dp)) {
                        PlayerBar(player, "recording:$ringid", rec.durationMs, onPlay = { player.play("recording:$ringid", rec.url!!) })
                    }
                }
            } ?: recording?.takeIf { it.status == "processing" || it.status == "recording" }?.let {
                SectionHeader("Recording")
                Text("The recording is being prepared.", style = RingType.bodySmall, color = c.textMuted)
            }

            current.voicemailId?.let { id ->
                SectionHeader("Voicemail")
                CardGroup {
                    GroupRow(onClick = { nav.push(Route.VoicemailDetail(id)) }) {
                        Text("Open the message", style = RingType.body, color = c.accentText, modifier = Modifier.weight(1f))
                    }
                }
            }

            val timeline = entries.filter { it.kind != EntryKind.SPEECH && it.kind != EntryKind.NOTE }
            if (timeline.isNotEmpty()) {
                SectionHeader("Timeline")
                Timeline(timeline)
            }

            val speech = entries.filter { it.kind == EntryKind.SPEECH && !it.text.isNullOrBlank() }
            SectionHeader("Transcript")
            if (speech.isEmpty()) {
                Text(if (loading) "Loading…" else "Nothing was said on this call.", style = RingType.bodySmall, color = c.textMuted)
            } else {
                CardGroup {
                    Column(Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(14.dp)) {
                        speech.forEach { entry ->
                            Column {
                                PixelLabel("${entry.actor?.label ?: "Someone"} · ${entry.offsetMs?.let { Format.duration(it) } ?: Format.clock(entry.at)}")
                                Spacer(Modifier.height(2.dp))
                                Text(entry.text.orEmpty(), style = RingType.bodyLarge, color = c.foreground)
                            }
                        }
                    }
                }
            }
        }
    }
}

@Composable
private fun ParticipantRow(call: Call, account: Account, me: String) {
    val c = Ring.colors
    val participant = call.participant(account.uuid)
    val invitation = call.invitations.lastOrNull { it.to.uuid == account.uuid }
    val status = when {
        account.uuid == call.createdBy.uuid -> "Caller"
        participant != null -> "Joined" + (participant.invitedBy?.let { " · invited by ${it.label}" } ?: "")
        invitation != null -> invitation.state.replaceFirstChar { it.uppercase() }
        else -> ""
    }
    GroupRow {
        Avatar(account, 40.dp)
        Column(Modifier.weight(1f)) {
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                Text(if (account.uuid == me) "${account.label} (you)" else account.label, style = RingType.title, color = c.foreground, maxLines = 1, overflow = TextOverflow.Ellipsis, modifier = Modifier.weight(1f, fill = false))
                SiliconBadge(account)
            }
            PixelId(account.id)
        }
        PixelLabel(status)
    }
}

/** The teamofsilicons.com timeline: a hairline rail with dots and pixel-font times. */
@Composable
private fun Timeline(entries: List<TranscriptEntry>) {
    val c = Ring.colors
    Column {
        entries.forEachIndexed { index, entry ->
            Row(Modifier.fillMaxWidth().height(IntrinsicSize.Min)) {
                Box(Modifier.width(20.dp).fillMaxHeight(), contentAlignment = Alignment.TopCenter) {
                    Box(
                        Modifier
                            .padding(top = if (index == 0) 8.dp else 0.dp, bottom = if (index == entries.lastIndex) 0.dp else 0.dp)
                            .width(hairline)
                            .fillMaxHeight(if (index == entries.lastIndex) 0.3f else 1f)
                            .background(c.borderStrong),
                    )
                    Box(Modifier.padding(top = 6.dp).size(7.dp).clip(CircleShape).background(if (entry.kind == EntryKind.MISSED || entry.kind == EntryKind.DECLINED) c.danger else c.accent))
                }
                Column(Modifier.padding(start = 10.dp, bottom = 14.dp)) {
                    PixelLabel(Format.clock(entry.at))
                    Text(describe(entry), style = RingType.body, color = c.foreground)
                }
            }
        }
    }
}

private fun describe(entry: TranscriptEntry): String {
    val who = entry.actor?.label ?: "Someone"
    return when (entry.kind) {
        EntryKind.CALL_STARTED -> "$who started the call"
        EntryKind.ANSWERED -> entry.dataString("after_ms")?.toLongOrNull()?.let { "$who answered after ${it / 1000} s" } ?: "$who answered"
        EntryKind.JOINED -> "$who joined"
        EntryKind.LEFT -> "$who left"
        EntryKind.INVITED -> {
            val invitee = entry.dataElement("invitee")?.let { runCatching { RingJson.decodeFromJsonElement(Account.serializer(), it).label }.getOrNull() }
            "$who invited ${invitee ?: "someone"}"
        }
        EntryKind.DECLINED -> entry.dataString("reason")?.let { "$who declined: $it" } ?: "$who declined"
        EntryKind.MISSED -> "$who missed the call"
        EntryKind.BUSY -> "$who was busy"
        EntryKind.VOICEMAIL -> "$who left a voicemail"
        EntryKind.CALL_ENDED -> "Call ended" + (entry.dataString("reason")?.let { " ($it)" } ?: "")
        else -> entry.text ?: entry.kind
    }
}
