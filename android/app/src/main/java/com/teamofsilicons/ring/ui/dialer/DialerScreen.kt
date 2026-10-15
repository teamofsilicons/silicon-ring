package com.teamofsilicons.ring.ui.dialer

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.safeDrawingPadding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.OutlinedTextField
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
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.KeyboardCapitalization
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import com.teamofsilicons.ring.AppGraph
import com.teamofsilicons.ring.R
import com.teamofsilicons.ring.data.Account
import com.teamofsilicons.ring.data.AccountLookup
import com.teamofsilicons.ring.data.ApiException
import com.teamofsilicons.ring.data.RecentPerson
import com.teamofsilicons.ring.data.normalizeActorRef
import com.teamofsilicons.ring.ui.Navigator
import com.teamofsilicons.ring.ui.components.Avatar
import com.teamofsilicons.ring.ui.components.Badge
import com.teamofsilicons.ring.ui.components.CardGroup
import com.teamofsilicons.ring.ui.components.Divider
import com.teamofsilicons.ring.ui.components.GroupRow
import com.teamofsilicons.ring.ui.components.IconAction
import com.teamofsilicons.ring.ui.components.PixelId
import com.teamofsilicons.ring.ui.components.PrimaryButton
import com.teamofsilicons.ring.ui.components.RingIcon
import com.teamofsilicons.ring.ui.components.SectionHeader
import com.teamofsilicons.ring.ui.components.SiliconBadge
import com.teamofsilicons.ring.ui.rememberCallPermissions
import com.teamofsilicons.ring.ui.settings.ringFieldColors
import com.teamofsilicons.ring.ui.theme.Ring
import com.teamofsilicons.ring.ui.theme.RingType
import kotlinx.coroutines.async
import kotlinx.coroutines.awaitAll
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch

@Composable
fun DialerScreen(graph: AppGraph, nav: Navigator, prefill: String?, message: String?) {
    val permissions = rememberCallPermissions()
    var error by remember { mutableStateOf(message) }
    var placing by remember { mutableStateOf(false) }
    val scope = rememberCoroutineScope()
    Box(Modifier.fillMaxSize().safeDrawingPadding().imePadding()) {
        DialerContent(
            graph = graph,
            title = "New call",
            actionLabel = "Call",
            prefill = prefill,
            externalError = error,
            busy = placing,
            onClose = { nav.pop() },
            onPick = { account ->
                permissions.withMicrophone { granted ->
                    if (!granted) {
                        error = "Ring needs the microphone to place calls."
                        return@withMicrophone
                    }
                    placing = true
                    scope.launch {
                        error = graph.calls.placeCall(account.id)
                        placing = false
                        if (error == null) {
                            nav.pop()
                            graph.calls.openCallScreen()
                        }
                    }
                }
            },
        )
    }
}

/**
 * The dialer: an id field with a live lookup card, and recent people below. Shared by New call
 * and by "add person" during a call.
 */
@Composable
fun DialerContent(
    graph: AppGraph,
    title: String,
    actionLabel: String,
    prefill: String?,
    externalError: String?,
    busy: Boolean,
    onClose: () -> Unit,
    onPick: (Account) -> Unit,
    exclude: Set<String> = emptySet(),
) {
    val c = Ring.colors
    var query by remember { mutableStateOf(prefill.orEmpty()) }
    var found by remember { mutableStateOf<List<AccountLookup>>(emptyList()) }
    var lookupError by remember { mutableStateOf<String?>(null) }
    var looking by remember { mutableStateOf(false) }
    var recent by remember { mutableStateOf<List<RecentPerson>>(emptyList()) }
    val focus = remember { FocusRequester() }

    LaunchedEffect(Unit) { if (prefill == null) runCatching { focus.requestFocus() } }

    LaunchedEffect(query) {
        val text = query.trim()
        lookupError = null
        if (text.isEmpty()) {
            found = emptyList()
            looking = false
        } else {
            delay(300)
            val candidates = normalizeActorRef(text)?.let { listOf(it) }
                ?: text.trimStart('@').lowercase().takeIf { h -> h.all { it.isLetterOrDigit() || it in "_-." } }
                    ?.let { listOf("c:$it", "si:$it") }
                ?: emptyList()
            looking = candidates.isNotEmpty()
            val results = kotlinx.coroutines.coroutineScope {
                candidates.map { ref ->
                    async {
                        try {
                            graph.api.lookup(ref)
                        } catch (e: ApiException) {
                            if (e.code != "account_not_found" && e.status != 404) lookupError = e.userMessage
                            null
                        }
                    }
                }.awaitAll().filterNotNull()
            }
            found = results
            looking = false
            if (results.isEmpty() && lookupError == null && candidates.isNotEmpty()) lookupError = "No one uses that id."
        }
    }

    LaunchedEffect(query) {
        delay(if (query.isEmpty()) 0 else 300)
        recent = runCatching { graph.api.recentPeople(query.trim().ifEmpty { null }, 20) }.getOrDefault(recent)
    }

    LazyColumn(Modifier.fillMaxSize(), contentPadding = PaddingValues(start = 16.dp, end = 16.dp, bottom = 32.dp)) {
        item {
            Row(Modifier.fillMaxWidth().padding(top = 8.dp), verticalAlignment = Alignment.CenterVertically) {
                IconAction(R.drawable.ms_close, "Close", onClose, tint = c.foreground)
                Text(title, style = RingType.pageTitle.copy(fontSize = RingType.heading.fontSize), color = c.foreground, modifier = Modifier.padding(start = 4.dp))
            }
            Spacer(Modifier.height(12.dp))
            OutlinedTextField(
                value = query,
                onValueChange = { query = it },
                singleLine = true,
                placeholder = { Text("c:handle or si:handle", style = RingType.bodyLarge, color = c.textMuted) },
                leadingIcon = { RingIcon(R.drawable.ms_search, null, tint = c.textMuted, size = 20.dp) },
                trailingIcon = if (looking) ({ CircularProgressIndicator(Modifier.size(18.dp), strokeWidth = 2.dp, color = c.accent) }) else null,
                textStyle = RingType.bodyLarge,
                shape = RoundedCornerShape(8.dp),
                colors = ringFieldColors(),
                keyboardOptions = KeyboardOptions(
                    keyboardType = KeyboardType.Email,
                    capitalization = KeyboardCapitalization.None,
                    autoCorrectEnabled = false,
                    imeAction = ImeAction.Go,
                ),
                keyboardActions = KeyboardActions(onGo = {
                    found.firstOrNull()?.account?.takeIf { it.uuid !in exclude }?.let(onPick)
                }),
                modifier = Modifier.fillMaxWidth().focusRequester(focus),
            )
            (externalError ?: lookupError?.takeIf { query.isNotBlank() && !looking })?.let {
                Text(it, style = RingType.bodySmall, color = if (externalError != null) c.danger else c.textMuted, modifier = Modifier.padding(top = 10.dp, start = 4.dp))
            }
        }
        items(found, key = { "found:" + it.account.uuid }) { result ->
            Spacer(Modifier.height(12.dp))
            LookupCard(result, actionLabel, busy, enabled = result.account.uuid !in exclude) { onPick(result.account) }
        }
        if (recent.isNotEmpty()) {
            item { SectionHeader(if (query.isBlank()) "Recent" else "Matching recent") }
            item {
                CardGroup {
                    recent.forEachIndexed { index, person ->
                        if (index > 0) Divider(inset = 64.dp)
                        GroupRow(onClick = if (person.account.uuid in exclude || busy) null else ({ onPick(person.account) })) {
                            Avatar(person.account, 36.dp)
                            Column(Modifier.weight(1f)) {
                                Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                                    Text(person.account.label, style = RingType.title, color = c.foreground, maxLines = 1, overflow = TextOverflow.Ellipsis, modifier = Modifier.weight(1f, fill = false))
                                    SiliconBadge(person.account)
                                }
                                PixelId(person.account.id)
                            }
                            RingIcon(if (actionLabel == "Call") R.drawable.ms_call else R.drawable.ms_person_add, null, tint = c.accent, size = 20.dp)
                        }
                    }
                }
            }
        }
    }
}

@Composable
private fun LookupCard(result: AccountLookup, actionLabel: String, busy: Boolean, enabled: Boolean, onPick: () -> Unit) {
    val c = Ring.colors
    val account = result.account
    CardGroup {
        Row(Modifier.padding(16.dp), verticalAlignment = Alignment.CenterVertically) {
            Avatar(account, 48.dp)
            Column(Modifier.weight(1f).padding(horizontal = 12.dp)) {
                Text(account.label, style = RingType.serifName.copy(fontSize = RingType.heading.fontSize), color = c.foreground, maxLines = 1, overflow = TextOverflow.Ellipsis)
                Spacer(Modifier.height(4.dp))
                PixelId(account.id)
                Spacer(Modifier.height(8.dp))
                Row(horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                    SiliconBadge(account)
                    Badge(if (result.onRing) "On Ring" else "Not on Ring yet")
                }
            }
            PrimaryButton(
                actionLabel,
                onClick = onPick,
                enabled = enabled && !busy,
                icon = if (actionLabel == "Call") R.drawable.ms_call_fill else R.drawable.ms_person_add,
            )
        }
    }
}
