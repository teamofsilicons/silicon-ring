package com.teamofsilicons.ring.ui.signin

import androidx.compose.foundation.clickable
import androidx.compose.foundation.interaction.MutableInteractionSource
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
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.widthIn
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import com.teamofsilicons.ring.AppGraph
import com.teamofsilicons.ring.BuildConfig
import com.teamofsilicons.ring.ui.components.PixelLabel
import com.teamofsilicons.ring.ui.components.PrimaryButton
import com.teamofsilicons.ring.ui.components.RingMark
import com.teamofsilicons.ring.ui.settings.ServerDialog
import com.teamofsilicons.ring.ui.theme.Ring
import com.teamofsilicons.ring.ui.theme.RingType

@Composable
fun SignInScreen(message: String?, busy: Boolean, onSignIn: () -> Unit, graph: AppGraph) {
    val c = Ring.colors
    val apiUrl by graph.prefs.apiUrl.collectAsStateWithLifecycle()
    var taps by remember { mutableIntStateOf(0) }
    var editingServer by remember { mutableStateOf(false) }

    Box(Modifier.fillMaxSize().safeDrawingPadding().padding(horizontal = 24.dp)) {
        Column(
            Modifier.align(Alignment.Center).widthIn(max = 420.dp).fillMaxWidth(),
            horizontalAlignment = Alignment.CenterHorizontally,
        ) {
            // Five taps on the mark open the developer server setting.
            RingMark(
                88.dp,
                Modifier.semantics { contentDescription = "Ring" }.clickable(remember { MutableInteractionSource() }, null) {
                    taps++
                    if (taps >= 5) {
                        taps = 0
                        editingServer = true
                    }
                },
            )
            Spacer(Modifier.height(20.dp))
            Text("RING", style = RingType.wordmark, color = c.ink)
            Spacer(Modifier.height(14.dp))
            Text(
                "Calls between carbons and silicons.",
                style = RingType.hero,
                color = c.foreground,
                textAlign = TextAlign.Center,
            )
            Spacer(Modifier.height(36.dp))
            if (busy) {
                Box(Modifier.height(44.dp), contentAlignment = Alignment.Center) {
                    CircularProgressIndicator(Modifier.size(24.dp), color = c.accent, strokeWidth = 2.dp)
                }
            } else {
                PrimaryButton("Continue with Silicon Accounts", onClick = onSignIn, modifier = Modifier.fillMaxWidth())
            }
            if (message != null) {
                Spacer(Modifier.height(16.dp))
                Text(message, style = RingType.bodySmall, color = c.danger, textAlign = TextAlign.Center)
            }
        }
        Column(
            Modifier.align(Alignment.BottomCenter).padding(bottom = 20.dp),
            horizontalAlignment = Alignment.CenterHorizontally,
        ) {
            Row(horizontalArrangement = Arrangement.Center) {
                Text(
                    "Silicons use the ring CLI · Ring sends no telemetry",
                    style = RingType.bodySmall,
                    color = c.textMuted,
                    textAlign = TextAlign.Center,
                )
            }
            if (BuildConfig.DEBUG || graph.prefs.apiUrlOverridden) {
                Spacer(Modifier.height(6.dp))
                PixelLabel(apiUrl, uppercase = false)
            }
        }
    }

    if (editingServer) {
        ServerDialog(
            current = apiUrl,
            onDismiss = { editingServer = false },
            onSave = {
                graph.changeServer(it)
                editingServer = false
            },
        )
    }
}
