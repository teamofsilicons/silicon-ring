package com.teamofsilicons.ring.data

import android.content.Context
import androidx.core.content.edit
import com.teamofsilicons.ring.BuildConfig
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonNamingStrategy

/** The one Json configuration for the Ring wire format. */
@OptIn(kotlinx.serialization.ExperimentalSerializationApi::class)
val RingJson: Json = Json {
    namingStrategy = JsonNamingStrategy.SnakeCase
    ignoreUnknownKeys = true
    explicitNulls = false
    encodeDefaults = true
    coerceInputValues = true
}

enum class Appearance { System, Light, Dark }

/** Non-secret app preferences. Secrets live in [SessionStore]. */
class AppPrefs(context: Context) {
    private val prefs = context.getSharedPreferences("ring-prefs", Context.MODE_PRIVATE)

    private val _appearance = MutableStateFlow(
        runCatching { Appearance.valueOf(prefs.getString(KEY_APPEARANCE, null) ?: "") }.getOrDefault(Appearance.System),
    )
    val appearance: StateFlow<Appearance> = _appearance.asStateFlow()

    private val _apiUrl = MutableStateFlow(prefs.getString(KEY_API_URL, null) ?: BuildConfig.DEFAULT_API_URL)
    val apiUrl: StateFlow<String> = _apiUrl.asStateFlow()

    val apiUrlOverridden: Boolean get() = prefs.contains(KEY_API_URL)

    fun setAppearance(value: Appearance) {
        prefs.edit { putString(KEY_APPEARANCE, value.name) }
        _appearance.value = value
    }

    /** Points the app at another Ring server (developer setting). `null` restores the default. */
    fun setApiUrl(value: String?) {
        val cleaned = value?.trim()?.trimEnd('/')?.takeIf { it.isNotEmpty() }
        prefs.edit { if (cleaned == null) remove(KEY_API_URL) else putString(KEY_API_URL, cleaned) }
        _apiUrl.value = cleaned ?: BuildConfig.DEFAULT_API_URL
    }

    var fcmToken: String?
        get() = prefs.getString(KEY_FCM_TOKEN, null)
        set(value) = prefs.edit { if (value == null) remove(KEY_FCM_TOKEN) else putString(KEY_FCM_TOKEN, value) }

    /** The token most recently accepted by the server, so we only re-register on change. */
    var registeredFcmToken: String?
        get() = prefs.getString(KEY_FCM_REGISTERED, null)
        set(value) = prefs.edit { if (value == null) remove(KEY_FCM_REGISTERED) else putString(KEY_FCM_REGISTERED, value) }

    /** PKCE state for an in-flight sign-in; it survives process death while the browser is open. */
    fun savePendingSignIn(state: String, verifier: String, apiUrl: String) = prefs.edit(commit = true) {
        putString(KEY_PENDING_STATE, state)
        putString(KEY_PENDING_VERIFIER, verifier)
        putString(KEY_PENDING_API, apiUrl)
    }

    fun pendingSignIn(): Triple<String, String, String>? {
        val state = prefs.getString(KEY_PENDING_STATE, null) ?: return null
        val verifier = prefs.getString(KEY_PENDING_VERIFIER, null) ?: return null
        val api = prefs.getString(KEY_PENDING_API, null) ?: return null
        return Triple(state, verifier, api)
    }

    fun clearPendingSignIn() = prefs.edit(commit = true) {
        remove(KEY_PENDING_STATE)
        remove(KEY_PENDING_VERIFIER)
        remove(KEY_PENDING_API)
    }

    var permissionsIntroSeen: Boolean
        get() = prefs.getBoolean(KEY_PERMISSIONS_SEEN, false)
        set(value) = prefs.edit { putBoolean(KEY_PERMISSIONS_SEEN, value) }

    private companion object {
        const val KEY_APPEARANCE = "appearance"
        const val KEY_API_URL = "api_url"
        const val KEY_FCM_TOKEN = "fcm_token"
        const val KEY_FCM_REGISTERED = "fcm_registered"
        const val KEY_PENDING_STATE = "signin_state"
        const val KEY_PENDING_VERIFIER = "signin_verifier"
        const val KEY_PENDING_API = "signin_api"
        const val KEY_PERMISSIONS_SEEN = "permissions_seen"
    }
}
