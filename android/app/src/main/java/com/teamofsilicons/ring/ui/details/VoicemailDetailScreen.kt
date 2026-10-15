package com.teamofsilicons.ring.ui.details

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.safeDrawingPadding
import androidx.compose.foundation.rememberScrollState
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
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.unit.dp
import com.teamofsilicons.ring.AppGraph
import com.teamofsilicons.ring.R
import com.teamofsilicons.ring.call.CallNotifications
import com.teamofsilicons.ring.data.ApiException
import com.teamofsilicons.ring.data.Voicemail
import com.teamofsilicons.ring.ui.Format
import com.teamofsilicons.ring.ui.Navigator
import com.teamofsilicons.ring.ui.Route
import com.teamofsilicons.ring.ui.components.Avatar
import com.teamofsilicons.ring.ui.components.CardGroup
import com.teamofsilicons.ring.ui.components.IconAction
import com.teamofsilicons.ring.ui.components.Identity
import com.teamofsilicons.ring.ui.components.PixelLabel
import com.teamofsilicons.ring.ui.components.PlayerBar
import com.teamofsilicons.ring.ui.components.PrimaryButton
import com.teamofsilicons.ring.ui.components.SecondaryButton
import com.teamofsilicons.ring.ui.components.SectionHeader
import com.teamofsilicons.ring.ui.components.rememberAudioPlayer
import com.teamofsilicons.ring.ui.rememberCallPermissions
import com.teamofsilicons.ring.ui.settings.ConfirmDialog
import com.teamofsilicons.ring.ui.theme.Ring
import com.teamofsilicons.ring.ui.theme.RingType
import kotlinx.coroutines.launch

@Composable
fun VoicemailDetailScreen(graph: AppGraph, nav: Navigator, voicemailId: String) {
    val c = Ring.colors
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    val player = rememberAudioPlayer()
    val permissions = rememberCallPermissions()
    var voicemail by remember { mutableStateOf<Voicemail?>(graph.repo.voicemails.value.items.firstOrNull { it.voicemailId == voicemailId }) }
    var error by remember { mutableStateOf<String?>(null) }
    var loading by remember { mutableStateOf(true) }
    var confirmDelete by remember { mutableStateOf(false) }

    LaunchedEffect(voicemailId) {
        try {
            var fresh = graph.api.voicemail(voicemailId)
            if (!fresh.read) fresh = graph.api.markVoicemail(voicemailId, true).copy(audioUrl = fresh.audioUrl)
            voicemail = fresh
            graph.repo.upsertVoicemail(fresh.copy(audioUrl = null))
            CallNotifications.cancelVoicemail(context, voicemailId)
        } catch (e: ApiException) {
            error = e.userMessage
        }
        loading = false
    }

    Column(Modifier.fillMaxSize().safeDrawingPadding()) {
        Row(Modifier.fillMaxWidth().padding(horizontal = 8.dp, vertical = 4.dp), verticalAlignment = Alignment.CenterVertically) {
            IconAction(R.drawable.ms_arrow_back, "Back", { nav.pop() }, tint = c.foreground)
            Text("Voicemail", style = RingType.heading, color = c.foreground, modifier = Modifier.weight(1f))
            IconAction(R.drawable.ms_delete, "Delete", { confirmDelete = true }, tint = c.textSecondary)
        }
        val vm = voicemail
        if (vm == null) {
            Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) {
                if (loading) CircularProgressIndicator(color = c.accent) else Text(error ?: "Voicemail not found", style = RingType.body, color = c.textMuted)
            }
            return@Column
        }
        Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(horizontal = 16.dp).padding(bottom = 40.dp)) {
            Spacer(Modifier.height(16.dp))
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(16.dp)) {
                Avatar(vm.from, 64.dp)
                Identity(vm.from, RingType.pageTitle, c.foreground, c.textMuted, center = false)
            }
            Spacer(Modifier.height(16.dp))
            PixelLabel(Format.voicemailReason(vm.reason, vm.declineReason), color = c.foreground)
            Spacer(Modifier.height(4.dp))
            PixelLabel(Format.whenFull(vm.createdAt))

            SectionHeader("Message")
            CardGroup {
                Box(Modifier.padding(16.dp)) {
                    when {
                        vm.audioStatus == "ready" && vm.audioUrl != null ->
                            PlayerBar(player, vm.voicemailId, vm.durationMs, onPlay = { player.play(vm.voicemailId, vm.audioUrl) })
                        vm.audioStatus == "failed" -> Text("The audio could not be saved.", style = RingType.bodySmall, color = c.danger)
                        else -> Text("The audio is still processing.", style = RingType.bodySmall, color = c.textMuted)
                    }
                }
            }

            SectionHeader("Transcript")
            Text(
                when {
                    !vm.transcript.isNullOrBlank() -> vm.transcript
                    vm.transcriptStatus == "pending" -> "Transcribing…"
                    vm.transcriptStatus == "failed" -> "No transcript is available."
                    else -> "No words were left."
                },
                style = RingType.bodyLarge,
                color = if (vm.transcript.isNullOrBlank()) c.textMuted else c.foreground,
            )

            Spacer(Modifier.height(28.dp))
            Row(horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                PrimaryButton("Call back", icon = R.drawable.ms_call_fill, onClick = {
                    permissions.withMicrophone { ok ->
                        if (!ok) {
                            error = "Ring needs the microphone to place calls."
                            return@withMicrophone
                        }
                        scope.launch {
                            error = graph.calls.placeCall(vm.from.id)
                            if (error == null) graph.calls.openCallScreen()
                        }
                    }
                })
                SecondaryButton("Call details", onClick = { nav.push(Route.CallDetails(vm.ringid)) })
            }
            error?.let { Text(it, style = RingType.bodySmall, color = c.danger, modifier = Modifier.padding(top = 12.dp)) }
        }
    }

    if (confirmDelete) {
        ConfirmDialog(
            title = "Delete this voicemail?",
            text = "It is removed from Ring for good.",
            confirm = "Delete",
            onDismiss = { confirmDelete = false },
            onConfirm = {
                confirmDelete = false
                scope.launch {
                    try {
                        graph.api.deleteVoicemail(voicemailId)
                        graph.repo.removeVoicemail(voicemailId)
                        nav.pop()
                    } catch (e: ApiException) {
                        error = e.userMessage
                    }
                }
            },
        )
    }
}
