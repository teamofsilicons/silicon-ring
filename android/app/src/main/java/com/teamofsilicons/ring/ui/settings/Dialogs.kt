package com.teamofsilicons.ring.ui.settings

import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.OutlinedTextFieldDefaults
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.ui.unit.dp
import com.teamofsilicons.ring.BuildConfig
import com.teamofsilicons.ring.ui.components.PixelLabel
import com.teamofsilicons.ring.ui.components.TextAction
import com.teamofsilicons.ring.ui.theme.Ring
import com.teamofsilicons.ring.ui.theme.RingType
import okhttp3.HttpUrl.Companion.toHttpUrlOrNull

@Composable
fun ringFieldColors() = OutlinedTextFieldDefaults.colors(
    focusedBorderColor = Ring.colors.accent,
    unfocusedBorderColor = Ring.colors.borderStrong,
    focusedContainerColor = Ring.colors.surface,
    unfocusedContainerColor = Ring.colors.surfaceMuted,
    cursorColor = Ring.colors.accent,
    focusedTextColor = Ring.colors.foreground,
    unfocusedTextColor = Ring.colors.foreground,
)

/** A text editing dialog with a code-point limit, as every Ring text limit is counted. */
@Composable
fun TextEditDialog(
    title: String,
    initial: String,
    limit: Int,
    placeholder: String,
    singleLine: Boolean,
    onDismiss: () -> Unit,
    onSave: (String) -> Unit,
    help: String? = null,
) {
    var text by remember { mutableStateOf(initial) }
    val length = text.codePointCount(0, text.length)
    val c = Ring.colors
    AlertDialog(
        onDismissRequest = onDismiss,
        containerColor = c.surfaceRaised,
        shape = RoundedCornerShape(12.dp),
        title = { Text(title, style = RingType.heading, color = c.foreground) },
        text = {
            Column {
                help?.let {
                    Text(it, style = RingType.bodySmall, color = c.textSecondary)
                    Spacer(Modifier.height(12.dp))
                }
                OutlinedTextField(
                    value = text,
                    onValueChange = { text = it },
                    singleLine = singleLine,
                    minLines = if (singleLine) 1 else 3,
                    placeholder = { Text(placeholder, style = RingType.body, color = c.textMuted) },
                    textStyle = RingType.body,
                    shape = RoundedCornerShape(8.dp),
                    colors = ringFieldColors(),
                    modifier = Modifier.fillMaxWidth(),
                )
                Spacer(Modifier.height(6.dp))
                PixelLabel("$length / $limit", color = if (length > limit) c.danger else c.textMuted)
            }
        },
        confirmButton = {
            val valid = text.isNotBlank() && length <= limit
            TextAction("Save", { if (valid) onSave(text.trim()) }, color = if (valid) c.accentText else c.textMuted)
        },
        dismissButton = { TextAction("Cancel", onDismiss, color = c.textSecondary) },
    )
}

/** Developer setting: which Ring server this app talks to. Changing it signs out. */
@Composable
fun ServerDialog(current: String, onDismiss: () -> Unit, onSave: (String?) -> Unit) {
    var text by remember { mutableStateOf(current) }
    val c = Ring.colors
    AlertDialog(
        onDismissRequest = onDismiss,
        containerColor = c.surfaceRaised,
        shape = RoundedCornerShape(12.dp),
        title = { Text("Ring server", style = RingType.heading, color = c.foreground) },
        text = {
            Column {
                Text(
                    "For development. A session belongs to one server, so changing it signs you out.",
                    style = RingType.bodySmall,
                    color = c.textSecondary,
                )
                Spacer(Modifier.height(12.dp))
                OutlinedTextField(
                    value = text,
                    onValueChange = { text = it },
                    singleLine = true,
                    textStyle = RingType.body,
                    shape = RoundedCornerShape(8.dp),
                    colors = ringFieldColors(),
                    keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Uri),
                    modifier = Modifier.fillMaxWidth(),
                )
                Spacer(Modifier.height(6.dp))
                PixelLabel("Default: ${BuildConfig.DEFAULT_API_URL}", uppercase = false)
            }
        },
        confirmButton = {
            // Release builds forbid cleartext traffic, so only debug builds may point at http://.
            val valid = text.trim().let {
                (it.startsWith("https://") || (BuildConfig.DEBUG && it.startsWith("http://"))) && it.toHttpUrlOrNull() != null
            }
            TextAction("Save", { if (valid) onSave(text.trim()) }, color = if (valid) c.accentText else c.textMuted)
        },
        dismissButton = {
            TextAction("Use default", { onSave(null) }, color = c.textSecondary)
        },
    )
}

/** A yes/no confirmation for destructive actions. */
@Composable
fun ConfirmDialog(title: String, text: String, confirm: String, onDismiss: () -> Unit, onConfirm: () -> Unit) {
    val c = Ring.colors
    AlertDialog(
        onDismissRequest = onDismiss,
        containerColor = c.surfaceRaised,
        shape = RoundedCornerShape(12.dp),
        title = { Text(title, style = RingType.heading, color = c.foreground) },
        text = { Text(text, style = RingType.body, color = c.textSecondary) },
        confirmButton = { TextAction(confirm, onConfirm, color = c.danger) },
        dismissButton = { TextAction("Cancel", onDismiss, color = c.textSecondary) },
    )
}
