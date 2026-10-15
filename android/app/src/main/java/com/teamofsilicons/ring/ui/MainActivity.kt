package com.teamofsilicons.ring.ui

import android.content.Context
import android.content.Intent
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.SystemBarStyle
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.lifecycle.lifecycleScope
import com.teamofsilicons.ring.auth.SignIn
import com.teamofsilicons.ring.data.Appearance
import com.teamofsilicons.ring.ring
import com.teamofsilicons.ring.ui.theme.RingTheme
import kotlinx.coroutines.launch

class MainActivity : ComponentActivity() {

    private val nav = Navigator()

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        val graph = ring
        setContent {
            val appearance by graph.prefs.appearance.collectAsStateWithLifecycle()
            val signInMessage by graph.signIn.message.collectAsStateWithLifecycle()
            val signingIn by graph.signIn.busy.collectAsStateWithLifecycle()
            val dark = when (appearance) {
                Appearance.System -> (resources.configuration.uiMode and android.content.res.Configuration.UI_MODE_NIGHT_MASK) == android.content.res.Configuration.UI_MODE_NIGHT_YES
                Appearance.Light -> false
                Appearance.Dark -> true
            }
            LaunchedEffect(dark) {
                val transparent = android.graphics.Color.TRANSPARENT
                val style = if (dark) SystemBarStyle.dark(transparent) else SystemBarStyle.light(transparent, transparent)
                enableEdgeToEdge(statusBarStyle = style, navigationBarStyle = style)
            }
            RingTheme(appearance) {
                RingRoot(
                    graph = graph,
                    nav = nav,
                    signInMessage = signInMessage,
                    signingIn = signingIn,
                    onSignIn = ::startSignIn,
                )
            }
        }
        // A recreated activity gets its launch intent again; it was handled the first time.
        if (savedInstanceState == null) handle(intent)
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        handle(intent)
    }

    private fun startSignIn() {
        val signIn = ring.signIn
        signIn.message.value = null
        signIn.busy.value = true
        lifecycleScope.launch {
            try {
                signIn.message.value = signIn.start(this@MainActivity)
            } finally {
                signIn.busy.value = false
            }
        }
    }

    private fun handle(intent: Intent?) {
        intent ?: return
        val data = intent.data
        if (intent.action == Intent.ACTION_VIEW && data?.scheme == SignIn.SCHEME) {
            val graph = ring
            val signIn = graph.signIn
            val navigator = nav
            signIn.busy.value = true
            // The app scope, not the activity's: the exchange must finish even if this activity
            // is recreated meanwhile, because the code cannot be used twice.
            graph.scope.launch {
                try {
                    when (val result = signIn.complete(data, graph.repo.deviceName)) {
                        is SignIn.Result.Success -> {
                            graph.completeSignIn(result.apiUrl, result.session)
                            signIn.message.value = null
                            navigator.reset()
                        }
                        is SignIn.Result.Failure -> signIn.message.value = result.message
                        SignIn.Result.Cancelled -> signIn.message.value = null
                        SignIn.Result.NotOurs -> Unit
                    }
                } finally {
                    signIn.busy.value = false
                }
            }
            setIntent(Intent(this, MainActivity::class.java))
            return
        }
        intent.getStringExtra(EXTRA_CALL)?.let { nav.push(Route.CallDetails(it)) }
        intent.getStringExtra(EXTRA_VOICEMAIL)?.let { nav.push(Route.VoicemailDetail(it)) }
        intent.getStringExtra(EXTRA_CALL_BACK)?.let { nav.push(Route.Dialer(prefill = it)) }
        intent.removeExtra(EXTRA_CALL)
        intent.removeExtra(EXTRA_VOICEMAIL)
        intent.removeExtra(EXTRA_CALL_BACK)
    }

    companion object {
        private const val EXTRA_CALL = "open_call"
        private const val EXTRA_VOICEMAIL = "open_voicemail"
        private const val EXTRA_CALL_BACK = "call_back"

        fun intentForCall(context: Context, ringid: String): Intent =
            Intent(context, MainActivity::class.java).putExtra(EXTRA_CALL, ringid).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)

        fun intentForVoicemail(context: Context, voicemailId: String): Intent =
            Intent(context, MainActivity::class.java).putExtra(EXTRA_VOICEMAIL, voicemailId).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)

        fun intentForCallBack(context: Context, ref: String): Intent =
            Intent(context, MainActivity::class.java).putExtra(EXTRA_CALL_BACK, ref).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
    }
}
