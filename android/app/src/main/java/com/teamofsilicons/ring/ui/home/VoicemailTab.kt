package com.teamofsilicons.ring.ui.home

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Text
import androidx.compose.material3.pulltorefresh.PullToRefreshBox
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import com.teamofsilicons.ring.AppGraph
import com.teamofsilicons.ring.R
import com.teamofsilicons.ring.call.CallNotifications
import com.teamofsilicons.ring.data.ApiException
import com.teamofsilicons.ring.data.Voicemail
import com.teamofsilicons.ring.ui.Format
import com.teamofsilicons.ring.ui.Navigator
import com.teamofsilicons.ring.ui.Route
import com.teamofsilicons.ring.ui.components.AudioPlayerState
import com.teamofsilicons.ring.ui.components.Avatar
import com.teamofsilicons.ring.ui.components.GroupRow
import com.teamofsilicons.ring.ui.components.IconAction
import com.teamofsilicons.ring.ui.components.PixelLabel
import com.teamofsilicons.ring.ui.components.SiliconBadge
import com.teamofsilicons.ring.ui.components.rememberAudioPlayer
import com.teamofsilicons.ring.ui.theme.Ring
import com.teamofsilicons.ring.ui.theme.RingType
import kotlinx.coroutines.launch

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun VoicemailTab(graph: AppGraph, nav: Navigator) {
    val voicemails by graph.repo.voicemails.collectAsStateWithLifecycle()
    val scope = rememberCoroutineScope()
    val player = rememberAudioPlayer()
    val context = androidx.compose.ui.platform.LocalContext.current

    fun play(vm: Voicemail) {
        if (player.isPlaying(vm.voicemailId)) return player.pause()
        scope.launch {
            try {
                val full = graph.api.voicemail(vm.voicemailId)
                val url = full.audioUrl ?: return@launch
                player.play(vm.voicemailId, url)
                if (!vm.read) {
                    graph.repo.upsertVoicemail(graph.api.markVoicemail(vm.voicemailId, true))
                    CallNotifications.cancelVoicemail(context, vm.voicemailId)
                }
            } catch (_: ApiException) {
                // The row stays as it was; the detail screen shows the error.
            }
        }
    }

    PullToRefreshBox(
        isRefreshing = voicemails.loading && voicemails.loaded,
        onRefresh = { scope.launch { graph.repo.refreshVoicemails() } },
        modifier = Modifier.fillMaxSize(),
    ) {
        LazyColumn(contentPadding = PaddingValues(start = 16.dp, end = 16.dp, bottom = 120.dp), modifier = Modifier.fillMaxSize()) {
            item { PageTitle("Voicemail") }
            voicemails.error?.let { message ->
                item { Text(message, style = RingType.bodySmall, color = Ring.colors.danger, modifier = Modifier.padding(bottom = 12.dp)) }
            }
            if (voicemails.loaded && voicemails.items.isEmpty()) {
                item { EmptyState("No voicemail") }
            } else {
                cardItems(voicemails.items, key = { it.voicemailId }) { vm ->
                    VoicemailRow(vm, player, onOpen = { nav.push(Route.VoicemailDetail(vm.voicemailId)) }, onPlay = { play(vm) })
                }
            }
        }
    }
}

@Composable
private fun VoicemailRow(vm: Voicemail, player: AudioPlayerState, onOpen: () -> Unit, onPlay: () -> Unit) {
    val c = Ring.colors
    GroupRow(onClick = onOpen) {
        Box(Modifier.width(8.dp), contentAlignment = Alignment.Center) {
            if (!vm.read) Box(Modifier.size(8.dp).clip(CircleShape).background(c.accent))
        }
        Avatar(vm.from, 40.dp)
        Column(Modifier.weight(1f)) {
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                Text(
                    vm.from.label,
                    style = RingType.title.copy(fontWeight = if (vm.read) androidx.compose.ui.text.font.FontWeight.Normal else androidx.compose.ui.text.font.FontWeight.Medium),
                    color = c.foreground,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                    modifier = Modifier.weight(1f, fill = false),
                )
                SiliconBadge(vm.from)
            }
            Spacer(Modifier.height(3.dp))
            val length = vm.durationMs?.let { " · ${Format.duration(it)}" }.orEmpty()
            PixelLabel("${Format.voicemailReason(vm.reason, vm.declineReason)} · ${Format.whenShort(vm.createdAt)}$length")
            val preview = when {
                !vm.transcript.isNullOrBlank() -> vm.transcript
                vm.transcriptStatus == "pending" -> "Transcribing…"
                else -> null
            }
            if (preview != null) {
                Spacer(Modifier.height(4.dp))
                Text(preview, style = RingType.bodySmall, color = c.textSecondary, maxLines = 2, overflow = TextOverflow.Ellipsis)
            }
        }
        IconAction(
            if (player.isPlaying(vm.voicemailId)) R.drawable.ms_pause_fill else R.drawable.ms_play_arrow_fill,
            if (player.isPlaying(vm.voicemailId)) "Pause" else "Play",
            onPlay,
            enabled = vm.audioStatus == "ready",
        )
    }
}
