package com.teamofsilicons.ring.ui.home

import android.net.Uri
import androidx.browser.customtabs.CustomTabsIntent
import androidx.compose.animation.core.animateDpAsState
import androidx.compose.animation.core.spring
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Switch
import androidx.compose.material3.SwitchDefaults
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
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import com.teamofsilicons.ring.AppGraph
import com.teamofsilicons.ring.BuildConfig
import com.teamofsilicons.ring.R
import com.teamofsilicons.ring.data.ApiException
import com.teamofsilicons.ring.data.Appearance
import com.teamofsilicons.ring.data.Device
import com.teamofsilicons.ring.ui.Format
import com.teamofsilicons.ring.ui.components.Avatar
import com.teamofsilicons.ring.ui.components.Badge
import com.teamofsilicons.ring.ui.components.CardGroup
import com.teamofsilicons.ring.ui.components.Divider
import com.teamofsilicons.ring.ui.components.GroupRow
import com.teamofsilicons.ring.ui.components.PixelId
import com.teamofsilicons.ring.ui.components.PixelLabel
import com.teamofsilicons.ring.ui.components.RingIcon
import com.teamofsilicons.ring.ui.components.SectionHeader
import com.teamofsilicons.ring.ui.components.SecondaryButton
import com.teamofsilicons.ring.ui.components.TextAction
import com.teamofsilicons.ring.ui.settings.ConfirmDialog
import com.teamofsilicons.ring.ui.settings.ServerDialog
import com.teamofsilicons.ring.ui.settings.TextEditDialog
import com.teamofsilicons.ring.ui.theme.Ring
import com.teamofsilicons.ring.ui.theme.RingType
import kotlinx.coroutines.launch
import kotlinx.serialization.json.JsonNull
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put
import kotlinx.serialization.json.putJsonObject

private sealed interface SettingsDialog {
    data object DeviceName : SettingsDialog
    data class Greeting(val reason: String) : SettingsDialog
    data class Revoke(val device: Device) : SettingsDialog
    data object Server : SettingsDialog
    data object SignOut : SettingsDialog
}

@Composable
fun SettingsTab(graph: AppGraph) {
    val c = Ring.colors
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    val session by graph.sessions.session.collectAsStateWithLifecycle()
    val device by graph.repo.currentDevice.collectAsStateWithLifecycle()
    val devices by graph.repo.devices.collectAsStateWithLifecycle()
    val settings by graph.repo.settings.collectAsStateWithLifecycle()
    val meta by graph.repo.meta.collectAsStateWithLifecycle()
    val appearance by graph.prefs.appearance.collectAsStateWithLifecycle()
    val apiUrl by graph.prefs.apiUrl.collectAsStateWithLifecycle()
    var dialog by remember { mutableStateOf<SettingsDialog?>(null) }
    var error by remember { mutableStateOf<String?>(null) }

    LaunchedEffect(Unit) {
        graph.repo.refreshDevices()
        graph.repo.refreshSettings()
        graph.repo.refreshMe()
    }

    fun run(block: suspend () -> Unit) {
        scope.launch {
            error = null
            try {
                block()
            } catch (e: ApiException) {
                error = e.userMessage
            }
        }
    }

    fun patchSettings(body: kotlinx.serialization.json.JsonObject) = run { graph.repo.setSettings(graph.api.patchSettings(body)) }

    val account = session?.account ?: return
    Column(
        Modifier
            .fillMaxSize()
            .verticalScroll(rememberScrollState())
            .padding(start = 16.dp, end = 16.dp, bottom = 40.dp),
    ) {
        PageTitle("Settings")
        error?.let { Text(it, style = RingType.bodySmall, color = c.danger, modifier = Modifier.padding(bottom = 8.dp)) }

        SectionHeader("Account")
        CardGroup {
            GroupRow {
                Avatar(account, 48.dp)
                Column(Modifier.weight(1f)) {
                    Text(account.label, style = RingType.serifName.copy(fontSize = RingType.heading.fontSize), color = c.foreground, maxLines = 1, overflow = TextOverflow.Ellipsis)
                    Spacer(Modifier.height(4.dp))
                    Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                        PixelId(account.id)
                        Badge(if (account.isSilicon) "Silicon" else "Carbon", accent = account.isSilicon)
                    }
                }
            }
            Divider(inset = 16.dp)
            LinkRow("Edit profile on Silicon Accounts") {
                openUrl(context, meta?.accountsUrl ?: BuildConfig.ACCOUNTS_URL)
            }
        }

        SectionHeader("This device")
        CardGroup {
            GroupRow(onClick = { dialog = SettingsDialog.DeviceName }) {
                Column(Modifier.weight(1f)) {
                    Text("Name", style = RingType.body, color = c.foreground)
                    Text(device?.name ?: graph.repo.deviceName, style = RingType.bodySmall, color = c.textMuted, maxLines = 1, overflow = TextOverflow.Ellipsis)
                }
                RingIcon(R.drawable.ms_edit, "Rename", tint = c.textMuted, size = 18.dp)
            }
            Divider(inset = 16.dp)
            ToggleRow(
                title = "Ring on this device",
                subtitle = "Incoming calls ring here as well as on your other devices.",
                checked = device?.ringEnabled ?: true,
            ) { enabled -> run { graph.repo.updateThisDevice(ringEnabled = enabled) } }
        }

        SectionHeader("Devices")
        CardGroup {
            val others = devices.items
            if (others.isEmpty()) {
                GroupRow { Text(if (devices.loading) "Loading…" else "No devices", style = RingType.body, color = c.textMuted) }
            }
            others.forEachIndexed { index, item ->
                if (index > 0) Divider(inset = 16.dp)
                GroupRow {
                    RingIcon(R.drawable.ms_smartphone, null, tint = c.textSecondary, size = 20.dp)
                    Column(Modifier.weight(1f)) {
                        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                            Text(item.name ?: item.platform ?: "Device", style = RingType.body, color = c.foreground, maxLines = 1, overflow = TextOverflow.Ellipsis, modifier = Modifier.weight(1f, fill = false))
                            if (item.current) Badge("This device")
                        }
                        val seen = if (item.online) "Online" else item.lastSeenAt?.let { "Seen ${Format.whenShort(it)}" } ?: ""
                        PixelLabel(listOfNotNull(item.platform, item.appVersion, seen.takeIf { it.isNotEmpty() }).joinToString(" · "))
                    }
                    if (!item.current) TextAction("Revoke", { dialog = SettingsDialog.Revoke(item) }, color = c.danger)
                }
            }
        }

        SectionHeader("Voicemail")
        val voicemail = settings?.voicemail
        CardGroup {
            ToggleRow(
                title = "Voicemail",
                subtitle = "Callers can leave a message when you are busy, decline or miss a call.",
                checked = voicemail?.enabled ?: true,
                enabled = settings != null,
            ) { enabled -> patchSettings(buildJsonObject { putJsonObject("voicemail") { put("enabled", enabled) } }) }
            listOf("busy" to "When busy", "declined" to "When declined", "missed" to "When missed").forEach { (reason, title) ->
                Divider(inset = 16.dp)
                val custom = voicemail?.greetings?.get(reason)
                GroupRow(onClick = { dialog = SettingsDialog.Greeting(reason) }) {
                    Column(Modifier.weight(1f)) {
                        Text(title, style = RingType.body, color = c.foreground)
                        Text(custom ?: "Default greeting", style = RingType.bodySmall, color = c.textMuted, maxLines = 2, overflow = TextOverflow.Ellipsis)
                    }
                    if (custom != null) {
                        TextAction("Reset", {
                            patchSettings(buildJsonObject { putJsonObject("voicemail") { putJsonObject("greetings") { put(reason, JsonNull) } } })
                        }, color = c.textSecondary)
                    }
                }
            }
        }

        SectionHeader("Appearance")
        Segmented(
            options = Appearance.entries.map { it to it.name },
            selected = appearance,
            onSelect = { graph.prefs.setAppearance(it) },
        )

        SectionHeader("About")
        CardGroup {
            InfoRow("Version", "${BuildConfig.VERSION_NAME} (${BuildConfig.VERSION_CODE})")
            Divider(inset = 16.dp)
            LinkRow("Source") { openUrl(context, BuildConfig.SOURCE_URL) }
            Divider(inset = 16.dp)
            InfoRow("Privacy", "Ring sends no telemetry")
            Divider(inset = 16.dp)
            GroupRow(onClick = { dialog = SettingsDialog.Server }) {
                Column(Modifier.weight(1f)) {
                    Text("Server", style = RingType.body, color = c.foreground)
                    PixelId(apiUrl)
                }
                RingIcon(R.drawable.ms_chevron_right, null, tint = c.textMuted, size = 18.dp)
            }
        }

        Spacer(Modifier.height(28.dp))
        SecondaryButton("Sign out", onClick = { dialog = SettingsDialog.SignOut }, icon = R.drawable.ms_logout, contentColor = c.danger, modifier = Modifier.fillMaxWidth())
    }

    when (val d = dialog) {
        SettingsDialog.DeviceName -> TextEditDialog(
            title = "Device name",
            initial = device?.name ?: graph.repo.deviceName,
            limit = 80,
            placeholder = "Shubham's Pixel",
            singleLine = true,
            onDismiss = { dialog = null },
            onSave = {
                dialog = null
                run { graph.repo.updateThisDevice(name = it) }
            },
        )
        is SettingsDialog.Greeting -> TextEditDialog(
            title = when (d.reason) {
                "busy" -> "Greeting when busy"
                "declined" -> "Greeting when declined"
                else -> "Greeting when missed"
            },
            initial = settings?.voicemail?.greetings?.get(d.reason).orEmpty(),
            limit = 300,
            placeholder = "Hi, it's me. Leave a message after the beep.",
            singleLine = false,
            help = "Callers hear this, read in your voice, before the beep.",
            onDismiss = { dialog = null },
            onSave = { text ->
                dialog = null
                patchSettings(buildJsonObject { putJsonObject("voicemail") { putJsonObject("greetings") { put(d.reason, JsonPrimitive(text)) } } })
            },
        )
        is SettingsDialog.Revoke -> ConfirmDialog(
            title = "Revoke ${d.device.name ?: "device"}?",
            text = "That device is signed out of Ring and stops ringing.",
            confirm = "Revoke",
            onDismiss = { dialog = null },
            onConfirm = {
                dialog = null
                run {
                    graph.api.deleteDevice(d.device.deviceId)
                    graph.repo.removeDevice(d.device.deviceId)
                }
            },
        )
        SettingsDialog.Server -> ServerDialog(
            current = apiUrl,
            onDismiss = { dialog = null },
            onSave = {
                dialog = null
                graph.changeServer(it)
            },
        )
        SettingsDialog.SignOut -> ConfirmDialog(
            title = "Sign out of Ring?",
            text = "Calls stop ringing on this device until you sign in again.",
            confirm = "Sign out",
            onDismiss = { dialog = null },
            onConfirm = {
                dialog = null
                scope.launch { graph.signOut() }
            },
        )
        null -> Unit
    }
}

@Composable
private fun ToggleRow(title: String, subtitle: String?, checked: Boolean, enabled: Boolean = true, onChange: (Boolean) -> Unit) {
    val c = Ring.colors
    GroupRow(onClick = if (enabled) ({ onChange(!checked) }) else null) {
        Column(Modifier.weight(1f)) {
            Text(title, style = RingType.body, color = c.foreground)
            subtitle?.let { Text(it, style = RingType.bodySmall, color = c.textMuted) }
        }
        Switch(
            checked = checked,
            onCheckedChange = onChange,
            enabled = enabled,
            colors = SwitchDefaults.colors(
                checkedThumbColor = c.onPrimary,
                checkedTrackColor = c.accent,
                checkedBorderColor = c.accent,
                uncheckedThumbColor = c.textMuted,
                uncheckedTrackColor = c.beige,
                uncheckedBorderColor = c.borderStrong,
            ),
        )
    }
}

@Composable
private fun InfoRow(title: String, value: String) {
    val c = Ring.colors
    GroupRow {
        Text(title, style = RingType.body, color = c.foreground, modifier = Modifier.weight(1f))
        Text(value, style = RingType.bodySmall, color = c.textMuted)
    }
}

@Composable
private fun LinkRow(title: String, onClick: () -> Unit) {
    val c = Ring.colors
    GroupRow(onClick = onClick) {
        Text(title, style = RingType.body, color = c.accentText, modifier = Modifier.weight(1f))
        RingIcon(R.drawable.ms_open_in_new, null, tint = c.accentText, size = 18.dp)
    }
}

/** A segmented control with the beige pill that slides between options. */
@Composable
private fun <T> Segmented(options: List<Pair<T, String>>, selected: T, onSelect: (T) -> Unit) {
    val c = Ring.colors
    BoxWithConstraints(
        Modifier
            .fillMaxWidth()
            .height(44.dp)
            .clip(RoundedCornerShape(10.dp))
            .background(c.surfaceMuted)
            .padding(4.dp),
    ) {
        val width = maxWidth / options.size
        val index = options.indexOfFirst { it.first == selected }.coerceAtLeast(0)
        val x by animateDpAsState(width * index, spring(dampingRatio = 1f, stiffness = 600f), label = "segment")
        Box(
            Modifier
                .offset(x = x)
                .width(width)
                .fillMaxHeight()
                .clip(RoundedCornerShape(8.dp))
                .background(c.beige),
        )
        Row(Modifier.fillMaxSize()) {
            options.forEach { (value, label) ->
                Box(
                    Modifier
                        .weight(1f)
                        .fillMaxHeight()
                        .clip(RoundedCornerShape(8.dp))
                        .clickable { onSelect(value) },
                    contentAlignment = Alignment.Center,
                ) {
                    Text(label, style = RingType.button, color = if (value == selected) c.ink else c.textSecondary)
                }
            }
        }
    }
}

private fun openUrl(context: android.content.Context, url: String) {
    runCatching { CustomTabsIntent.Builder().setShowTitle(true).build().launchUrl(context, Uri.parse(url)) }
}
