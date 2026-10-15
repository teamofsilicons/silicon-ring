package com.teamofsilicons.ring.call

import android.content.Context
import android.net.Uri
import android.os.Build
import android.telecom.DisconnectCause
import android.util.Log
import androidx.core.telecom.CallAttributesCompat
import androidx.core.telecom.CallControlResult
import androidx.core.telecom.CallControlScope
import androidx.core.telecom.CallEndpointCompat
import androidx.core.telecom.CallsManager
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Job
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.launch
import kotlinx.coroutines.withTimeoutOrNull

/** Registers Ring with Telecom once per process; null when this device has no Telecom. */
object Telecom {
    @Volatile
    private var manager: CallsManager? = null
    @Volatile
    private var unavailable = false

    fun manager(context: Context): CallsManager? {
        manager?.let { return it }
        if (unavailable) return null
        return synchronized(this) {
            manager ?: try {
                CallsManager(context.applicationContext).also {
                    it.registerAppWithTelecom(CallsManager.CAPABILITY_BASELINE)
                    manager = it
                }
            } catch (e: Exception) {
                Log.w("RingTelecom", "Telecom is unavailable; calls will manage audio themselves", e)
                unavailable = true
                null
            }
        }
    }
}

/**
 * One call registered with Telecom through core-telecom. Telecom owns audio focus and routing and
 * lets watches, cars and headsets answer or end the call; this class turns that into callbacks.
 */
class TelecomCall(
    private val scope: CoroutineScope,
    private val manager: CallsManager,
    private val displayName: String,
    private val handle: String,
    private val incoming: Boolean,
    private val onAnswerRequested: () -> Unit,
    private val onDisconnectRequested: (DisconnectCause) -> Unit,
) {
    private var control: CallControlScope? = null
    private val ready = CompletableDeferred<Boolean>()
    private var job: Job? = null
    private var disconnecting = false

    private val _endpoints = MutableStateFlow<List<CallEndpointCompat>>(emptyList())
    val endpoints: StateFlow<List<CallEndpointCompat>> = _endpoints.asStateFlow()

    private val _endpoint = MutableStateFlow<CallEndpointCompat?>(null)
    val endpoint: StateFlow<CallEndpointCompat?> = _endpoint.asStateFlow()

    private val _muted = MutableStateFlow(false)
    val muted: StateFlow<Boolean> = _muted.asStateFlow()

    fun start() {
        val attributes = CallAttributesCompat(
            displayName = displayName,
            address = Uri.fromParts("ring", handle, null),
            direction = if (incoming) CallAttributesCompat.DIRECTION_INCOMING else CallAttributesCompat.DIRECTION_OUTGOING,
            callType = CallAttributesCompat.CALL_TYPE_AUDIO_CALL,
            callCapabilities = 0,
        )
        job = scope.launch {
            try {
                manager.addCall(
                    attributes,
                    onAnswer = { onAnswerRequested() },
                    onDisconnect = { cause -> if (!disconnecting) onDisconnectRequested(cause) },
                    onSetActive = {},
                    onSetInactive = {},
                ) {
                    control = this
                    ready.complete(true)
                    launch { availableEndpoints.collect { _endpoints.value = it } }
                    launch { currentCallEndpoint.collect { _endpoint.value = it } }
                    launch { isMuted.collect { _muted.value = it } }
                }
            } catch (e: CancellationException) {
                throw e
            } catch (e: Exception) {
                Log.w(TAG, "Telecom did not take the call", e)
            } finally {
                control = null
                ready.complete(false)
            }
        }
    }

    /** True once Telecom holds the call; false if it refused it or it already ended. */
    suspend fun awaitReady(): Boolean = withTimeoutOrNull(6_000) { ready.await() } ?: false

    suspend fun answer(): Boolean = act("answer") { answer(CallAttributesCompat.CALL_TYPE_AUDIO_CALL) }

    suspend fun setActive(): Boolean = act("setActive") { setActive() }

    suspend fun requestEndpoint(endpoint: CallEndpointCompat): Boolean = act("endpoint") { requestEndpointChange(endpoint) }

    fun disconnect(cause: Int, reason: String? = null) {
        if (disconnecting) return
        disconnecting = true
        scope.launch {
            // A call still being added must be disconnected once Telecom holds it, or it lingers.
            val control = if (awaitReady()) control else null
            if (control != null) {
                runCatching { control.disconnect(DisconnectCause(platformCause(cause), reason)) }
                    .onFailure { Log.w(TAG, "Telecom disconnect failed", it) }
            }
            job?.cancel()
        }
    }

    /**
     * Android 14+ (CallControl) accepts only LOCAL, REMOTE, REJECTED and MISSED and throws for
     * anything else; the ConnectionService path used before 14 takes every code.
     */
    private fun platformCause(cause: Int): Int =
        if (Build.VERSION.SDK_INT < 34) {
            cause
        } else {
            when (cause) {
                DisconnectCause.LOCAL, DisconnectCause.REMOTE, DisconnectCause.REJECTED, DisconnectCause.MISSED -> cause
                DisconnectCause.ANSWERED_ELSEWHERE -> DisconnectCause.REMOTE
                else -> DisconnectCause.LOCAL
            }
        }

    private suspend fun act(name: String, block: suspend CallControlScope.() -> CallControlResult): Boolean {
        if (!awaitReady()) return false
        val control = control ?: return false
        return when (val result = runCatching { control.block() }.getOrNull()) {
            is CallControlResult.Success -> true
            is CallControlResult.Error -> {
                Log.w(TAG, "Telecom $name failed: ${result.errorCode}")
                false
            }
            else -> false
        }
    }

    private companion object {
        const val TAG = "RingTelecom"
    }
}
