package com.teamofsilicons.ring.ui.home

import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.core.animateDpAsState
import androidx.compose.animation.core.spring
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.windowInsetsPadding
import androidx.compose.foundation.layout.safeDrawing
import androidx.compose.foundation.layout.WindowInsetsSides
import androidx.compose.foundation.layout.only
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.shadow
import androidx.compose.ui.platform.LocalConfiguration
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import com.teamofsilicons.ring.AppGraph
import com.teamofsilicons.ring.R
import com.teamofsilicons.ring.call.CallPhase
import com.teamofsilicons.ring.data.Call
import com.teamofsilicons.ring.ui.CallPermissions
import com.teamofsilicons.ring.ui.Navigator
import com.teamofsilicons.ring.ui.Route
import com.teamofsilicons.ring.ui.components.PrimaryButton
import com.teamofsilicons.ring.ui.components.RingIcon
import com.teamofsilicons.ring.ui.components.TextAction
import com.teamofsilicons.ring.ui.components.hairline
import com.teamofsilicons.ring.ui.rememberCallPermissions
import com.teamofsilicons.ring.ui.theme.Ring
import com.teamofsilicons.ring.ui.theme.RingType
import kotlinx.coroutines.launch

enum class HomeTab(val title: String, val icon: Int) {
    Recents("Recents", R.drawable.ms_history),
    Voicemail("Voicemail", R.drawable.ms_voicemail),
    Settings("Settings", R.drawable.ms_settings),
}

@Composable
fun HomeScreen(graph: AppGraph, nav: Navigator) {
    var tab by rememberSaveable { mutableStateOf(HomeTab.Recents) }
    val permissions = rememberCallPermissions()
    val wide = LocalConfiguration.current.screenWidthDp >= 600
    val c = Ring.colors

    val content: @Composable (Modifier) -> Unit = { modifier ->
        Column(modifier) {
            Banners(graph, permissions)
            Box(Modifier.weight(1f)) {
                when (tab) {
                    HomeTab.Recents -> RecentsTab(graph, nav, permissions)
                    HomeTab.Voicemail -> VoicemailTab(graph, nav)
                    HomeTab.Settings -> SettingsTab(graph)
                }
            }
        }
    }

    if (wide) {
        Row(Modifier.fillMaxSize()) {
            Sidebar(tab, onSelect = { tab = it }, onNewCall = { nav.push(Route.Dialer()) })
            content(Modifier.weight(1f).fillMaxHeight().windowInsetsPadding(WindowInsets.safeDrawing.only(WindowInsetsSides.Top + WindowInsetsSides.End)))
        }
    } else {
        Box(Modifier.fillMaxSize()) {
            Column(Modifier.fillMaxSize()) {
                content(Modifier.weight(1f).statusBarsPadding())
                BottomTabs(tab, onSelect = { tab = it })
            }
            AnimatedVisibility(
                visible = tab != HomeTab.Settings,
                modifier = Modifier.align(Alignment.BottomEnd).navigationBarsPadding().padding(end = 20.dp, bottom = 84.dp),
            ) {
                NewCallButton(onClick = { nav.push(Route.Dialer()) })
            }
        }
    }
}

@Composable
private fun NewCallButton(onClick: () -> Unit) {
    val c = Ring.colors
    Row(
        Modifier
            .shadow(6.dp, RoundedCornerShape(16.dp))
            .clip(RoundedCornerShape(16.dp))
            .background(c.accent)
            .clickable(onClick = onClick)
            .padding(horizontal = 18.dp, vertical = 16.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        RingIcon(R.drawable.ms_add_call, null, tint = c.onPrimary, size = 22.dp)
        Spacer(Modifier.width(10.dp))
        Text("New call", style = RingType.button, color = c.onPrimary)
    }
}

/** Phone tab bar with the site's sliding beige pill. */
@Composable
private fun BottomTabs(selected: HomeTab, onSelect: (HomeTab) -> Unit) {
    val c = Ring.colors
    Column(Modifier.fillMaxWidth().background(c.surface)) {
        Box(Modifier.fillMaxWidth().height(hairline).background(c.border))
        BoxWithConstraints(Modifier.fillMaxWidth().navigationBarsPadding().height(64.dp)) {
            val itemWidth = maxWidth / HomeTab.entries.size
            val pillWidth = 64.dp
            val x by animateDpAsState(itemWidth * selected.ordinal + (itemWidth - pillWidth) / 2, spring(dampingRatio = 1f, stiffness = 600f), label = "pill")
            Box(
                Modifier
                    .offset(x = x, y = 8.dp)
                    .size(pillWidth, 30.dp)
                    .clip(RoundedCornerShape(15.dp))
                    .background(c.beige),
            )
            Row(Modifier.fillMaxSize()) {
                HomeTab.entries.forEach { item ->
                    val active = item == selected
                    Column(
                        Modifier
                            .weight(1f)
                            .fillMaxHeight()
                            .clickable { onSelect(item) }
                            .padding(top = 11.dp),
                        horizontalAlignment = Alignment.CenterHorizontally,
                    ) {
                        RingIcon(item.icon, null, tint = if (active) c.ink else c.textMuted, size = 22.dp)
                        Spacer(Modifier.height(10.dp))
                        Text(item.title, style = RingType.pixelSmall, color = if (active) c.ink else c.textMuted)
                    }
                }
            }
        }
    }
}

/** Tablet sidebar on surfaceMuted with the beige selected pill. */
@Composable
private fun Sidebar(selected: HomeTab, onSelect: (HomeTab) -> Unit, onNewCall: () -> Unit) {
    val c = Ring.colors
    Row(Modifier.fillMaxHeight()) {
        Column(
            Modifier
                .width(232.dp)
                .fillMaxHeight()
                .background(c.surfaceMuted)
                .windowInsetsPadding(WindowInsets.safeDrawing.only(WindowInsetsSides.Top + WindowInsetsSides.Start + WindowInsetsSides.Bottom))
                .padding(16.dp),
            verticalArrangement = Arrangement.spacedBy(4.dp),
        ) {
            Row(verticalAlignment = Alignment.CenterVertically, modifier = Modifier.padding(bottom = 20.dp, start = 6.dp, top = 8.dp)) {
                com.teamofsilicons.ring.ui.components.RingMark(28.dp)
                Spacer(Modifier.width(10.dp))
                Text("RING", style = RingType.pixelLarge, color = c.ink)
            }
            PrimaryButton("New call", onClick = onNewCall, icon = R.drawable.ms_add_call, modifier = Modifier.fillMaxWidth())
            Spacer(Modifier.height(16.dp))
            HomeTab.entries.forEach { item ->
                val active = item == selected
                Row(
                    Modifier
                        .fillMaxWidth()
                        .clip(RoundedCornerShape(8.dp))
                        .background(if (active) c.beige else androidx.compose.ui.graphics.Color.Transparent)
                        .clickable { onSelect(item) }
                        .padding(horizontal = 12.dp, vertical = 10.dp),
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    RingIcon(item.icon, null, tint = if (active) c.ink else c.textSecondary, size = 20.dp)
                    Spacer(Modifier.width(12.dp))
                    Text(item.title, style = RingType.title, color = if (active) c.ink else c.textSecondary)
                }
            }
        }
        Box(Modifier.width(hairline).fillMaxHeight().background(c.border))
    }
}

/** Handoff, "return to call" and permission banners above every tab. */
@Composable
private fun Banners(graph: AppGraph, permissions: CallPermissions) {
    val c = Ring.colors
    val scope = rememberCoroutineScope()
    val elsewhere by graph.calls.elsewhere.collectAsStateWithLifecycle()
    val primary by graph.calls.primary.collectAsStateWithLifecycle()
    val moved by graph.calls.movedAway.collectAsStateWithLifecycle()
    var switchError by rememberSaveable { mutableStateOf<String?>(null) }
    val me = graph.repo.myUuid.orEmpty()
    val thisDevice = if (LocalConfiguration.current.smallestScreenWidthDp >= 600) "this tablet" else "this phone"

    Column(Modifier.fillMaxWidth().padding(horizontal = 16.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
        primary?.takeIf { it.joined || it.phase == CallPhase.Incoming }?.let { call ->
            Banner(
                text = if (call.phase == CallPhase.Incoming) "${call.peer.label} is calling" else "On a call with ${call.peer.label}",
                action = "Open",
                onAction = { graph.calls.openCallScreen() },
                accent = true,
            )
        }
        elsewhere.forEach { call: Call ->
            val device = call.participant(me)?.device?.label ?: "another device"
            val with = call.others(me).firstOrNull()?.label ?: "someone"
            Banner(
                text = "On a call with $with on $device",
                action = "Switch to $thisDevice",
                onAction = {
                    permissions.withMicrophone { ok ->
                        if (!ok) {
                            switchError = "Allow the microphone to take the call here."
                            return@withMicrophone
                        }
                        scope.launch {
                            switchError = graph.calls.switchHere(call.ringid)
                            if (switchError == null) graph.calls.openCallScreen()
                        }
                    }
                },
            )
        }
        moved?.let { Banner(text = it, action = null, onAction = {}) }
        switchError?.let { Text(it, style = RingType.bodySmall, color = c.danger) }
        if (!permissions.allGranted) PermissionCard(permissions)
    }
}

@Composable
private fun Banner(text: String, action: String?, onAction: () -> Unit, accent: Boolean = false) {
    val c = Ring.colors
    Row(
        Modifier
            .padding(top = 8.dp)
            .fillMaxWidth()
            .clip(RoundedCornerShape(10.dp))
            .background(if (accent) c.accentSubtle else c.beige)
            .border(hairline, if (accent) c.accentBorder else c.borderStrong, RoundedCornerShape(10.dp))
            .padding(start = 14.dp, end = 4.dp, top = 4.dp, bottom = 4.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Text(text, style = RingType.bodySmall, color = c.foreground, modifier = Modifier.weight(1f).padding(vertical = 8.dp), maxLines = 2, overflow = TextOverflow.Ellipsis)
        if (action != null) TextAction(action, onAction)
    }
}

/** Explains and requests what incoming calls need: notifications, microphone, full screen. */
@Composable
private fun PermissionCard(permissions: CallPermissions) {
    val c = Ring.colors
    Column(
        Modifier
            .padding(top = 8.dp)
            .fillMaxWidth()
            .clip(RoundedCornerShape(12.dp))
            .background(c.surface)
            .border(hairline, c.border, RoundedCornerShape(12.dp))
            .padding(16.dp),
        verticalArrangement = Arrangement.spacedBy(10.dp),
    ) {
        com.teamofsilicons.ring.ui.components.PixelLabel("Set up calling")
        if (!permissions.notifications) {
            PermissionRow("Allow notifications so calls can ring this phone.", "Allow") { permissions.requestNotifications() }
        }
        if (!permissions.microphone) {
            PermissionRow("Allow the microphone so people can hear you.", "Allow") { permissions.withMicrophone {} }
        }
        if (!permissions.fullScreen) {
            PermissionRow("Let Ring show incoming calls on the lock screen.", "Open settings") { permissions.openFullScreenSettings() }
        }
    }
}

@Composable
private fun PermissionRow(text: String, action: String, onClick: () -> Unit) {
    val c = Ring.colors
    Row(verticalAlignment = Alignment.CenterVertically) {
        Text(text, style = RingType.bodySmall, color = c.textSecondary, modifier = Modifier.weight(1f))
        TextAction(action, onClick)
    }
}
