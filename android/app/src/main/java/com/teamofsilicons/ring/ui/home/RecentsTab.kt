package com.teamofsilicons.ring.ui.home

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.LazyListScope
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Text
import androidx.compose.material3.pulltorefresh.PullToRefreshBox
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.derivedStateOf
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import com.teamofsilicons.ring.AppGraph
import com.teamofsilicons.ring.R
import com.teamofsilicons.ring.data.Call
import com.teamofsilicons.ring.ui.CallPermissions
import com.teamofsilicons.ring.ui.Format
import com.teamofsilicons.ring.ui.Navigator
import com.teamofsilicons.ring.ui.Route
import com.teamofsilicons.ring.ui.components.Avatar
import com.teamofsilicons.ring.ui.components.cardSegment
import com.teamofsilicons.ring.ui.components.Divider
import com.teamofsilicons.ring.ui.components.GroupRow
import com.teamofsilicons.ring.ui.components.IconAction
import com.teamofsilicons.ring.ui.components.PixelLabel
import com.teamofsilicons.ring.ui.components.RingIcon
import com.teamofsilicons.ring.ui.components.RingRings
import com.teamofsilicons.ring.ui.components.SiliconBadge
import com.teamofsilicons.ring.ui.theme.Ring
import com.teamofsilicons.ring.ui.theme.RingType
import kotlinx.coroutines.launch

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun RecentsTab(graph: AppGraph, nav: Navigator, permissions: CallPermissions) {
    val calls by graph.repo.calls.collectAsStateWithLifecycle()
    val session by graph.sessions.session.collectAsStateWithLifecycle()
    val me = session?.account?.uuid.orEmpty()
    val scope = rememberCoroutineScope()
    val list = rememberLazyListState()
    var error by remember { mutableStateOf<String?>(null) }

    val nearEnd by remember { derivedStateOf { list.layoutInfo.visibleItemsInfo.lastOrNull()?.index?.let { it >= list.layoutInfo.totalItemsCount - 3 } == true } }
    LaunchedEffect(nearEnd, calls.nextCursor) { if (nearEnd && calls.nextCursor != null) graph.repo.loadMoreCalls() }

    val redial: (Call) -> Unit = { call ->
        val target = Format.counterpart(call, me).first()
        permissions.withMicrophone { ok ->
            if (!ok) {
                error = "Ring needs the microphone to place calls."
                return@withMicrophone
            }
            scope.launch {
                error = graph.calls.placeCall(target.id)
                if (error == null) graph.calls.openCallScreen()
            }
        }
    }

    PullToRefreshBox(
        isRefreshing = calls.loading && calls.loaded,
        onRefresh = { scope.launch { graph.repo.refreshCalls() } },
        modifier = Modifier.fillMaxSize(),
    ) {
        LazyColumn(state = list, contentPadding = PaddingValues(start = 16.dp, end = 16.dp, bottom = 120.dp), modifier = Modifier.fillMaxSize()) {
            item { PageTitle("Recents") }
            (error ?: calls.error)?.let { message ->
                item { Text(message, style = RingType.bodySmall, color = Ring.colors.danger, modifier = Modifier.padding(bottom = 12.dp)) }
            }
            if (calls.loaded && calls.items.isEmpty()) {
                item { EmptyState("No calls yet") }
            } else {
                cardItems(calls.items, key = { it.ringid }) { call ->
                    CallRow(call, me, onOpen = { nav.push(Route.CallDetails(call.ringid)) }, onRedial = { redial(call) })
                }
            }
        }
    }
}

@Composable
fun PageTitle(text: String) {
    Text(text, style = RingType.pageTitle, color = Ring.colors.foreground, modifier = Modifier.padding(top = 20.dp, bottom = 16.dp, start = 4.dp))
}

@Composable
fun EmptyState(text: String) {
    val c = Ring.colors
    Column(Modifier.fillMaxWidth().padding(top = 72.dp), horizontalAlignment = Alignment.CenterHorizontally) {
        RingRings(88.dp, c.borderStrong)
        Spacer(Modifier.height(20.dp))
        Text(text, style = RingType.body, color = c.textMuted)
    }
}

/** Renders [items] as rows of one card group, with hairline dividers between rows. */
fun <T> LazyListScope.cardItems(items: List<T>, key: (T) -> Any, row: @Composable (T) -> Unit) {
    items.forEachIndexed { index, item ->
        item(key = key(item)) {
            val last = index == items.lastIndex
            Column(Modifier.fillMaxWidth().cardSegment(first = index == 0, last = last)) {
                row(item)
                if (!last) Divider(inset = 68.dp)
            }
        }
    }
}

@Composable
private fun CallRow(call: Call, me: String, onOpen: () -> Unit, onRedial: () -> Unit) {
    val c = Ring.colors
    val people = Format.counterpart(call, me)
    val main = people.first()
    val outcome = Format.outcome(call, me)
    GroupRow(onClick = onOpen) {
        Avatar(main, 40.dp)
        Column(Modifier.weight(1f)) {
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                val name = if (people.size > 1) "${main.label} +${people.size - 1}" else main.label
                Text(
                    name,
                    style = RingType.title,
                    color = if (outcome.missed) c.danger else c.foreground,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                    modifier = Modifier.weight(1f, fill = false),
                )
                SiliconBadge(main)
            }
            Spacer(Modifier.height(4.dp))
            Row(verticalAlignment = Alignment.CenterVertically) {
                RingIcon(outcome.icon, null, tint = if (outcome.missed) c.danger else c.textMuted, size = 14.dp)
                Spacer(Modifier.width(6.dp))
                PixelLabel("${Format.whenShort(call.createdAt)} · ${outcome.label}", color = if (outcome.missed) c.danger else c.textMuted)
            }
        }
        IconAction(R.drawable.ms_call, "Call ${main.label}", onRedial)
    }
}
