package com.teamofsilicons.ring

import android.app.Application
import android.content.Context
import android.util.Log
import androidx.core.app.NotificationManagerCompat
import androidx.lifecycle.DefaultLifecycleObserver
import androidx.lifecycle.LifecycleOwner
import androidx.lifecycle.ProcessLifecycleOwner
import coil3.ImageLoader
import coil3.SingletonImageLoader
import coil3.network.okhttp.OkHttpNetworkFetcherFactory
import com.google.firebase.FirebaseApp
import com.google.firebase.messaging.FirebaseMessaging
import com.teamofsilicons.ring.auth.SignIn
import com.teamofsilicons.ring.call.CallController
import com.teamofsilicons.ring.call.CallNotifications
import com.teamofsilicons.ring.data.AppPrefs
import com.teamofsilicons.ring.data.Realtime
import com.teamofsilicons.ring.data.RingApi
import com.teamofsilicons.ring.data.RingEvent
import com.teamofsilicons.ring.data.RingRepository
import com.teamofsilicons.ring.data.Session
import com.teamofsilicons.ring.data.SessionStore
import com.teamofsilicons.ring.data.StoredSession
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.launch
import kotlinx.coroutines.tasks.await
import okhttp3.OkHttpClient
import java.util.concurrent.TimeUnit

class RingApplication : Application(), SingletonImageLoader.Factory {
    lateinit var graph: AppGraph
        private set

    override fun onCreate() {
        super.onCreate()
        graph = AppGraph(this)
        CallNotifications.createChannels(this)
        ProcessLifecycleOwner.get().lifecycle.addObserver(object : DefaultLifecycleObserver {
            override fun onStart(owner: LifecycleOwner) = graph.onForeground()
            override fun onStop(owner: LifecycleOwner) = graph.onBackground()
        })
    }

    override fun newImageLoader(context: Context): ImageLoader =
        ImageLoader.Builder(context)
            .components { add(OkHttpNetworkFetcherFactory(callFactory = { graph.http })) }
            .build()
}

/** The app's long-lived objects. Everything here runs on the main thread unless noted. */
class AppGraph(val app: Application) {
    val scope = CoroutineScope(SupervisorJob() + Dispatchers.Main.immediate)
    val prefs = AppPrefs(app)
    val sessions = SessionStore(app)
    val http: OkHttpClient = OkHttpClient.Builder()
        .connectTimeout(15, TimeUnit.SECONDS)
        .readTimeout(30, TimeUnit.SECONDS)
        .writeTimeout(30, TimeUnit.SECONDS)
        .build()
    val api = RingApi(http, sessions, prefs, BuildConfig.VERSION_NAME)
    val realtime = Realtime(app, http, sessions, scope)
    val repo = RingRepository(app, api, sessions, prefs, scope)
    val calls = CallController(app, api, repo, realtime, scope)
    val signIn = SignIn(api, prefs)

    init {
        api.onUnauthorized = { scope.launch { signOutLocally() } }
        scope.launch {
            realtime.events.collect { event ->
                when (event) {
                    RingEvent.SessionRevoked -> signOutLocally()
                    is RingEvent.Ready -> {
                        repo.refreshAll()
                        calls.onEvent(event)
                    }
                    else -> {
                        repo.apply(event)
                        calls.onEvent(event)
                    }
                }
            }
        }
        if (sessions.current != null) {
            registerPush()
        }
    }

    fun onForeground() {
        realtime.hold(HOLD_APP)
        repo.refreshAll()
        if (sessions.current != null) calls.reconcile()
    }

    fun onBackground() {
        realtime.release(HOLD_APP)
    }

    /** Stores a fresh session from sign-in and brings the app up for it. */
    fun completeSignIn(apiUrl: String, session: Session) {
        sessions.save(StoredSession(apiUrl, session.sessionToken, session.expiresAt, session.account, session.deviceId))
        prefs.registeredFcmToken = null
        realtime.reset()
        repo.refreshAll()
        scope.launch { repo.registerDevice(force = true) }
        registerPush()
    }

    /** Turns on FCM (off until signed in) and registers this device's token with Ring. */
    fun registerPush() {
        val messaging = messaging() ?: return
        scope.launch {
            messaging.isAutoInitEnabled = true
            // Token registration (getToken/onNewToken). Firebase 26's register()/unregister() only
            // work with firebase_messaging_installation_id_enabled, which Ring does not opt into;
            // without it they always fail.
            if (prefs.fcmToken == null) {
                @Suppress("DEPRECATION")
                runCatching { messaging.token.await() }
                    .onSuccess { prefs.fcmToken = it }
                    .onFailure { Log.i(TAG, "FCM token unavailable: ${it.javaClass.simpleName}") }
            }
            repo.registerDevice()
        }
    }

    /** A new FCM token for this install. */
    fun onPushToken(token: String) {
        prefs.fcmToken = token
        if (sessions.current != null) scope.launch { repo.registerDevice() }
    }

    /** Signs out on the server (best effort) and locally. */
    suspend fun signOut() {
        runCatching { api.signOut() }
        signOutLocally()
    }

    fun signOutLocally() {
        if (sessions.current == null) return
        calls.shutdown()
        sessions.clear()
        repo.clear()
        prefs.fcmToken = null
        realtime.reset()
        NotificationManagerCompat.from(app).cancelAll()
        messaging()?.let { messaging ->
            messaging.isAutoInitEnabled = false
            @Suppress("DEPRECATION")
            scope.launch { runCatching { messaging.deleteToken().await() } }
        }
    }

    /**
     * Points the app at another server. A session belongs to one server, so this signs out, and
     * revokes the old session there so it does not linger as a device that still rings.
     */
    fun changeServer(url: String?) {
        val target = url?.trim()?.trimEnd('/')?.takeIf { it.isNotEmpty() } ?: BuildConfig.DEFAULT_API_URL
        val old = sessions.current
        val sameServer = target == (old?.apiUrl ?: prefs.apiUrl.value)
        prefs.setApiUrl(url)
        if (sameServer) return
        signOutLocally()
        scope.launch {
            old?.let { runCatching { api.signOut(it) } }
            repo.refreshMeta()
        }
    }

    private fun messaging(): FirebaseMessaging? = try {
        if (FirebaseApp.getApps(app).isEmpty()) null else FirebaseMessaging.getInstance()
    } catch (e: IllegalStateException) {
        null
    }

    private companion object {
        const val TAG = "Ring"
        const val HOLD_APP = "app"
    }
}

val Context.ring: AppGraph get() = (applicationContext as RingApplication).graph
