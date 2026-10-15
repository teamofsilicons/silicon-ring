package com.teamofsilicons.ring.data

import android.content.Context
import android.os.Build
import android.provider.Settings as AndroidSettings
import android.util.Log
import com.teamofsilicons.ring.BuildConfig
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Job
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock

/** A list loaded from the API, with paging and a last error. */
data class Listing<T>(
    val items: List<T> = emptyList(),
    val nextCursor: String? = null,
    val loading: Boolean = false,
    val loaded: Boolean = false,
    val error: String? = null,
)

/**
 * App-wide state that screens observe: the account, recents, voicemail, devices and settings.
 * It reconciles with REST whenever the realtime socket (re)connects and applies events in between.
 */
class RingRepository(
    private val context: Context,
    private val api: RingApi,
    private val sessions: SessionStore,
    private val prefs: AppPrefs,
    private val scope: CoroutineScope,
) {
    val session: StateFlow<StoredSession?> = sessions.session

    private val _calls = MutableStateFlow(Listing<Call>())
    val calls: StateFlow<Listing<Call>> = _calls.asStateFlow()

    private val _voicemails = MutableStateFlow(Listing<Voicemail>())
    val voicemails: StateFlow<Listing<Voicemail>> = _voicemails.asStateFlow()

    private val _devices = MutableStateFlow(Listing<Device>())
    val devices: StateFlow<Listing<Device>> = _devices.asStateFlow()

    private val _currentDevice = MutableStateFlow<Device?>(null)
    val currentDevice: StateFlow<Device?> = _currentDevice.asStateFlow()

    private val _settings = MutableStateFlow<Settings?>(null)
    val settings: StateFlow<Settings?> = _settings.asStateFlow()

    private val _meta = MutableStateFlow<Meta?>(null)
    val meta: StateFlow<Meta?> = _meta.asStateFlow()

    private val pushLock = Mutex()
    private var refreshJob: Job? = null

    val myUuid: String? get() = sessions.current?.account?.uuid
    val myDeviceId: String? get() = sessions.current?.deviceId

    /**
     * A human device name, as Android shows it in Settings > About, cut to the server's 80
     * code-point limit (a longer label would fail sign-in with `text_too_long`).
     */
    val deviceName: String by lazy {
        val name = AndroidSettings.Global.getString(context.contentResolver, AndroidSettings.Global.DEVICE_NAME)
            ?.trim()?.takeIf { it.isNotBlank() }
            ?: "${Build.MANUFACTURER.replaceFirstChar { it.uppercase() }} ${Build.MODEL}".trim()
        if (name.codePointCount(0, name.length) <= DEVICE_NAME_LIMIT) name
        else name.substring(0, name.offsetByCodePoints(0, DEVICE_NAME_LIMIT)).trim()
    }

    /** Reloads everything the home screens show. Cheap enough to run on every reconnect. */
    fun refreshAll() {
        if (sessions.current == null) return
        if (refreshJob?.isActive == true) return
        refreshJob = scope.launch {
            launch { refreshMe() }
            launch { refreshCalls() }
            launch { refreshVoicemails() }
            launch { refreshSettings() }
            launch { refreshMeta() }
        }
    }

    suspend fun refreshMe() {
        try {
            val me = api.me()
            sessions.updateAccount(me.account)
            _currentDevice.value = me.device
        } catch (e: ApiException) {
            Log.i(TAG, "me failed: ${e.code}")
        }
    }

    suspend fun refreshMeta() {
        runCatching { api.meta() }.onSuccess { _meta.value = it }
    }

    suspend fun refreshCalls() {
        _calls.update { it.copy(loading = true, error = null) }
        try {
            val page = api.calls(limit = 50)
            _calls.value = Listing(page.items, page.nextCursor, loading = false, loaded = true)
        } catch (e: ApiException) {
            _calls.update { it.copy(loading = false, error = e.userMessage) }
        }
    }

    suspend fun loadMoreCalls() {
        val current = _calls.value
        val cursor = current.nextCursor ?: return
        if (current.loading) return
        _calls.update { it.copy(loading = true) }
        try {
            val page = api.calls(limit = 50, cursor = cursor)
            _calls.update { list ->
                val known = list.items.map { it.ringid }.toSet()
                list.copy(items = list.items + page.items.filter { it.ringid !in known }, nextCursor = page.nextCursor, loading = false)
            }
        } catch (e: ApiException) {
            _calls.update { it.copy(loading = false, error = e.userMessage) }
        }
    }

    suspend fun refreshVoicemails() {
        _voicemails.update { it.copy(loading = true, error = null) }
        try {
            val page = api.voicemails(limit = 100)
            _voicemails.value = Listing(page.items, page.nextCursor, loading = false, loaded = true)
        } catch (e: ApiException) {
            _voicemails.update { it.copy(loading = false, error = e.userMessage) }
        }
    }

    suspend fun refreshDevices() {
        _devices.update { it.copy(loading = true, error = null) }
        try {
            _devices.value = Listing(api.devices(), loading = false, loaded = true)
        } catch (e: ApiException) {
            _devices.update { it.copy(loading = false, error = e.userMessage) }
        }
    }

    suspend fun refreshSettings() {
        runCatching { api.settings() }.onSuccess { _settings.value = it }
    }

    fun setSettings(value: Settings) {
        _settings.value = value
    }

    /** Applies a realtime event to the lists. Call handling itself lives in CallController. */
    fun apply(event: RingEvent) {
        when (event) {
            is RingEvent.CallIncoming -> upsertCall(event.call)
            is RingEvent.CallUpdated -> upsertCall(event.call)
            is RingEvent.CallEnded -> upsertCall(event.call)
            is RingEvent.VoicemailReceived -> upsertVoicemail(event.voicemail)
            is RingEvent.VoicemailUpdated -> upsertVoicemail(event.voicemail)
            is RingEvent.DeviceUpdated -> {
                if (event.device.current || event.device.deviceId == myDeviceId) _currentDevice.value = event.device
                _devices.update { list -> list.copy(items = list.items.map { if (it.deviceId == event.device.deviceId) event.device else it }) }
            }
            else -> Unit
        }
    }

    fun upsertCall(call: Call) {
        _calls.update { list ->
            val rest = list.items.filterNot { it.ringid == call.ringid }
            list.copy(items = (listOf(call) + rest).sortedByDescending { it.createdAt })
        }
    }

    fun upsertVoicemail(voicemail: Voicemail) {
        _voicemails.update { list ->
            val rest = list.items.filterNot { it.voicemailId == voicemail.voicemailId }
            list.copy(items = (listOf(voicemail) + rest).sortedByDescending { it.createdAt })
        }
    }

    fun removeVoicemail(id: String) {
        _voicemails.update { list -> list.copy(items = list.items.filterNot { it.voicemailId == id }) }
    }

    fun removeDevice(id: String) {
        _devices.update { list -> list.copy(items = list.items.filterNot { it.deviceId == id }) }
    }

    /**
     * Tells the server about this device: name, platform, version and the FCM token. Only sends
     * the push list when the token changed, so other device fields are not churned.
     */
    suspend fun registerDevice(force: Boolean = false) = pushLock.withLock {
        if (sessions.current == null) return@withLock
        val token = prefs.fcmToken
        val pushChanged = token != null && (force || token != prefs.registeredFcmToken)
        try {
            val device = api.updateCurrentDevice(
                DeviceUpdate(
                    name = if (force) deviceName else null,
                    platform = "android",
                    appVersion = BuildConfig.VERSION_NAME,
                    push = if (pushChanged) listOf(PushRegistration(kind = "fcm", token = token!!)) else null,
                ),
            )
            _currentDevice.value = device
            if (pushChanged) prefs.registeredFcmToken = token
        } catch (e: ApiException) {
            Log.i(TAG, "Device registration failed: ${e.code}")
        }
    }

    suspend fun updateThisDevice(name: String? = null, ringEnabled: Boolean? = null) {
        val device = api.updateCurrentDevice(DeviceUpdate(name = name, ringEnabled = ringEnabled))
        _currentDevice.value = device
        _devices.update { list -> list.copy(items = list.items.map { if (it.deviceId == device.deviceId) device else it }) }
    }

    /** Drops all signed-in state locally. */
    fun clear() {
        _calls.value = Listing()
        _voicemails.value = Listing()
        _devices.value = Listing()
        _currentDevice.value = null
        _settings.value = null
        prefs.registeredFcmToken = null
    }

    private companion object {
        const val TAG = "RingRepository"
        const val DEVICE_NAME_LIMIT = 80
    }
}
