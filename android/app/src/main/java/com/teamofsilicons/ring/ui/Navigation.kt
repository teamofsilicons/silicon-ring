package com.teamofsilicons.ring.ui

import androidx.activity.compose.BackHandler
import androidx.compose.animation.AnimatedContent
import androidx.compose.animation.core.tween
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.slideInHorizontally
import androidx.compose.animation.slideOutHorizontally
import androidx.compose.animation.togetherWith
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.ui.Modifier
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import com.teamofsilicons.ring.AppGraph
import com.teamofsilicons.ring.ui.details.CallDetailsScreen
import com.teamofsilicons.ring.ui.details.VoicemailDetailScreen
import com.teamofsilicons.ring.ui.dialer.DialerScreen
import com.teamofsilicons.ring.ui.home.HomeScreen
import com.teamofsilicons.ring.ui.signin.SignInScreen
import com.teamofsilicons.ring.ui.theme.Ring

sealed interface Route {
    data object Home : Route
    data class Dialer(val prefill: String? = null, val message: String? = null) : Route
    data class CallDetails(val ringid: String) : Route
    data class VoicemailDetail(val voicemailId: String) : Route
}

/** A small back stack; Home is always at the bottom. */
class Navigator {
    val stack = mutableStateListOf<Route>(Route.Home)
    val current: Route get() = stack.last()

    fun push(route: Route) {
        if (stack.last() != route) stack.add(route)
    }

    fun pop(): Boolean {
        if (stack.size <= 1) return false
        stack.removeAt(stack.lastIndex)
        return true
    }

    fun reset() {
        stack.clear()
        stack.add(Route.Home)
    }
}

@Composable
fun RingRoot(
    graph: AppGraph,
    nav: Navigator,
    signInMessage: String?,
    signingIn: Boolean,
    onSignIn: () -> Unit,
) {
    val session by graph.sessions.session.collectAsStateWithLifecycle()
    Box(Modifier.fillMaxSize().background(Ring.colors.background)) {
        if (session == null) {
            SignInScreen(message = signInMessage, busy = signingIn, onSignIn = onSignIn, graph = graph)
            return@Box
        }
        BackHandler(enabled = nav.stack.size > 1) { nav.pop() }
        val depth = nav.stack.size
        AnimatedContent(
            targetState = nav.current to depth,
            transitionSpec = {
                val forward = targetState.second >= initialState.second
                val enter = slideInHorizontally(tween(220)) { if (forward) it / 6 else -it / 6 } + fadeIn(tween(220))
                val exit = slideOutHorizontally(tween(220)) { if (forward) -it / 6 else it / 6 } + fadeOut(tween(160))
                enter togetherWith exit
            },
            label = "nav",
        ) { (route, _) ->
            when (route) {
                Route.Home -> HomeScreen(graph, nav)
                is Route.Dialer -> DialerScreen(graph, nav, route.prefill, route.message)
                is Route.CallDetails -> CallDetailsScreen(graph, nav, route.ringid)
                is Route.VoicemailDetail -> VoicemailDetailScreen(graph, nav, route.voicemailId)
            }
        }
    }
}
