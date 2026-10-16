package com.teamofsilicons.ring.data

import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.suspendCancellableCoroutine
import kotlinx.coroutines.withContext
import kotlinx.serialization.KSerializer
import kotlinx.serialization.builtins.serializer
import kotlinx.serialization.json.JsonObject
import okhttp3.Call as HttpCall
import okhttp3.Callback
import okhttp3.HttpUrl
import okhttp3.HttpUrl.Companion.toHttpUrlOrNull
import okhttp3.MediaType.Companion.toMediaType
import okhttp3.OkHttpClient
import okhttp3.Request
import okhttp3.RequestBody.Companion.toRequestBody
import okhttp3.Response
import java.io.IOException
import java.util.UUID
import kotlin.coroutines.resumeWithException

/** An error from the Ring API (or the network, with [code] `network`). */
class ApiException(
    val status: Int,
    val code: String,
    override val message: String,
    val hint: String? = null,
    val retryable: Boolean = false,
    val details: JsonObject = JsonObject(emptyMap()),
) : Exception(message) {
    val isAuthFailure: Boolean get() = status == 401
    val isNetwork: Boolean get() = code == NETWORK

    /** A short sentence to show people. */
    val userMessage: String get() = if (isNetwork) "Can't reach Ring. Check your connection." else message

    companion object {
        const val NETWORK = "network"
    }
}

/**
 * The Ring REST API. Authenticated calls use the signed-in session's origin and bearer token;
 * [onUnauthorized] fires when the server says that session is gone.
 */
class RingApi(
    private val http: OkHttpClient,
    private val sessions: SessionStore,
    private val prefs: AppPrefs,
    private val appVersion: String,
) {
    var onUnauthorized: (ApiException) -> Unit = {}

    private val jsonType = "application/json; charset=utf-8".toMediaType()

    // Public

    suspend fun meta(apiUrl: String = prefs.apiUrl.value): Meta =
        send(Request.Builder().url(url(apiUrl, "v1/meta")).get(), Meta.serializer(), authenticated = false)

    suspend fun signInCarbon(apiUrl: String, body: CarbonLogin): Session =
        send(
            Request.Builder().url(url(apiUrl, "v1/auth/carbon")).post(encode(CarbonLogin.serializer(), body)),
            Session.serializer(),
            authenticated = false,
        )

    // Session and devices

    suspend fun me(): Me = get("v1/me", Me.serializer())

    suspend fun signOut() = delete("v1/session")

    /** Revokes [session] on its own server, for when it is no longer the current one. */
    suspend fun signOut(session: StoredSession) {
        val request = Request.Builder()
            .url(url(session.apiUrl, "v1/session"))
            .delete()
            .header("Authorization", "Bearer ${session.token}")
        send(request, Unit.serializer(), authenticated = false)
    }

    suspend fun updateCurrentDevice(update: DeviceUpdate): Device =
        put("v1/devices/current", DeviceUpdate.serializer(), update, Device.serializer())

    suspend fun devices(): List<Device> = get("v1/devices", Items.serializer(Device.serializer())).items

    suspend fun patchDevice(deviceId: String, patch: DevicePatch): Device =
        patch("v1/devices/${seg(deviceId)}", DevicePatch.serializer(), patch, Device.serializer())

    suspend fun deleteDevice(deviceId: String) = delete("v1/devices/${seg(deviceId)}")

    // People

    suspend fun lookup(ref: String): AccountLookup = get("v1/accounts/${seg(ref)}", AccountLookup.serializer())

    suspend fun recentPeople(query: String?, limit: Int = 20): List<RecentPerson> =
        get(
            "v1/people/recent",
            Items.serializer(RecentPerson.serializer()),
            "q" to query?.takeIf { it.isNotBlank() },
            "limit" to limit.toString(),
        ).items

    // Calls

    suspend fun calls(states: String? = null, limit: Int = 50, cursor: String? = null): Page<Call> =
        get("v1/calls", Page.serializer(Call.serializer()), "state" to states, "limit" to limit.toString(), "cursor" to cursor)

    suspend fun call(ringid: String): Call = get("v1/calls/${seg(ringid)}", Call.serializer())

    suspend fun createCall(to: String, idempotencyKey: String = UUID.randomUUID().toString()): CreatedCall =
        post("v1/calls", CreateCall.serializer(), CreateCall(to), CreatedCall.serializer(), idempotencyKey)

    suspend fun accept(ringid: String, invitationId: String?): CallWithMedia =
        post("v1/calls/${seg(ringid)}/accept", AcceptCall.serializer(), AcceptCall(invitationId), CallWithMedia.serializer())

    suspend fun decline(ringid: String, invitationId: String?, reason: String?): CallEnvelope =
        post(
            "v1/calls/${seg(ringid)}/decline",
            DeclineCall.serializer(),
            DeclineCall(invitationId, reason?.takeIf { it.isNotBlank() }, giveNoReason = reason.isNullOrBlank()),
            CallEnvelope.serializer(),
        )

    suspend fun silence(ringid: String, invitationId: String?): CallEnvelope =
        post("v1/calls/${seg(ringid)}/silence", SilenceCall.serializer(), SilenceCall(invitationId), CallEnvelope.serializer())

    suspend fun leave(ringid: String): CallEnvelope =
        post("v1/calls/${seg(ringid)}/leave", JsonObject.serializer(), JsonObject(emptyMap()), CallEnvelope.serializer())

    suspend fun invite(ringid: String, to: String, idempotencyKey: String = UUID.randomUUID().toString()): InvitedCall =
        post("v1/calls/${seg(ringid)}/invite", InviteToCall.serializer(), InviteToCall(to), InvitedCall.serializer(), idempotencyKey)

    suspend fun media(ringid: String): CallWithMedia =
        post("v1/calls/${seg(ringid)}/media", JsonObject.serializer(), JsonObject(emptyMap()), CallWithMedia.serializer())

    suspend fun transcript(ringid: String, afterSeq: Long = 0, limit: Int = 200): TranscriptPage =
        get(
            "v1/calls/${seg(ringid)}/transcript",
            TranscriptPage.serializer(),
            "after_seq" to afterSeq.toString(),
            "limit" to limit.toString(),
        )

    /** Every transcript entry after [afterSeq], following pages to the end. */
    suspend fun fullTranscript(ringid: String, afterSeq: Long = 0): List<TranscriptEntry> {
        val all = mutableListOf<TranscriptEntry>()
        var after = afterSeq
        while (true) {
            val page = transcript(ringid, after, 200)
            all += page.items
            if (page.items.size < 200 || page.nextAfterSeq <= after) return all
            after = page.nextAfterSeq
        }
    }

    suspend fun recording(ringid: String): Recording = get("v1/calls/${seg(ringid)}/recording", Recording.serializer())

    // Voicemail

    suspend fun voicemails(limit: Int = 50, cursor: String? = null): Page<Voicemail> =
        get("v1/voicemails", Page.serializer(Voicemail.serializer()), "limit" to limit.toString(), "cursor" to cursor)

    suspend fun voicemail(id: String): Voicemail = get("v1/voicemails/${seg(id)}", Voicemail.serializer())

    suspend fun markVoicemail(id: String, read: Boolean): Voicemail =
        patch("v1/voicemails/${seg(id)}", VoicemailPatch.serializer(), VoicemailPatch(read), Voicemail.serializer())

    suspend fun deleteVoicemail(id: String) = delete("v1/voicemails/${seg(id)}")

    // Settings

    suspend fun settings(): Settings = get("v1/settings", Settings.serializer())

    /** `PATCH /v1/settings` with a raw body, because a greeting reset needs an explicit JSON null. */
    suspend fun patchSettings(body: JsonObject): Settings =
        patch("v1/settings", JsonObject.serializer(), body, Settings.serializer())

    // Plumbing

    private fun base(): String = sessions.current?.apiUrl ?: prefs.apiUrl.value

    private fun url(apiUrl: String, path: String, vararg query: Pair<String, String?>): HttpUrl {
        val builder = "${apiUrl.trimEnd('/')}/$path".toHttpUrlOrNull()?.newBuilder()
            ?: throw ApiException(0, "invalid_server", "The Ring server address $apiUrl is not valid.")
        query.forEach { (k, v) -> if (v != null) builder.addQueryParameter(k, v) }
        return builder.build()
    }

    private fun seg(value: String): String =
        java.net.URLEncoder.encode(value, Charsets.UTF_8.name()).replace("+", "%20")

    private fun <T> encode(serializer: KSerializer<T>, body: T) =
        RingJson.encodeToString(serializer, body).toRequestBody(jsonType)

    private suspend fun <R> get(path: String, out: KSerializer<R>, vararg query: Pair<String, String?>): R =
        send(Request.Builder().url(url(base(), path, *query)).get(), out)

    private suspend fun <B, R> post(path: String, inS: KSerializer<B>, body: B, out: KSerializer<R>, idempotencyKey: String? = null): R {
        val request = Request.Builder().url(url(base(), path)).post(encode(inS, body))
        if (idempotencyKey != null) request.header("Idempotency-Key", idempotencyKey)
        return send(request, out)
    }

    private suspend fun <B, R> put(path: String, inS: KSerializer<B>, body: B, out: KSerializer<R>): R =
        send(Request.Builder().url(url(base(), path)).put(encode(inS, body)), out)

    private suspend fun <B, R> patch(path: String, inS: KSerializer<B>, body: B, out: KSerializer<R>): R =
        send(Request.Builder().url(url(base(), path)).patch(encode(inS, body)), out)

    private suspend fun delete(path: String) {
        send(Request.Builder().url(url(base(), path)).delete(), Unit.serializer())
    }

    private suspend fun <R> send(builder: Request.Builder, out: KSerializer<R>, authenticated: Boolean = true): R {
        builder.header("Accept", "application/json")
        builder.header("User-Agent", "Ring-Android/$appVersion")
        if (authenticated) sessions.current?.let { builder.header("Authorization", "Bearer ${it.token}") }
        // The response body is read on the IO dispatcher: callers resume on the main thread, and
        // reading a slow body there throws NetworkOnMainThreadException.
        val (status, ok, text) = withContext(Dispatchers.IO) {
            val response = try {
                http.newCall(builder.build()).await()
            } catch (e: IOException) {
                throw ApiException(0, ApiException.NETWORK, e.message ?: "Network error", retryable = true)
            }
            response.use {
                val body = try {
                    it.body?.string().orEmpty()
                } catch (e: IOException) {
                    throw ApiException(0, ApiException.NETWORK, e.message ?: "Network error", retryable = true)
                }
                Triple(it.code, it.isSuccessful, body)
            }
        }
        if (!ok) {
            val error = runCatching { RingJson.decodeFromString(ApiErrorBody.serializer(), text).error }.getOrNull()
            val failure = ApiException(
                status = status,
                code = error?.code ?: "http_$status",
                message = error?.message ?: "Ring returned HTTP $status.",
                hint = error?.hint,
                retryable = error?.retryable ?: (status >= 500),
                details = error?.details ?: JsonObject(emptyMap()),
            )
            if (authenticated && failure.isAuthFailure) onUnauthorized(failure)
            throw failure
        }
        @Suppress("UNCHECKED_CAST")
        if (out == Unit.serializer() || text.isBlank()) return Unit as R
        return try {
            RingJson.decodeFromString(out, text)
        } catch (e: Exception) {
            throw ApiException(status, "decode", "Ring sent a response this app could not read.")
        }
    }
}

private suspend fun HttpCall.await(): Response = suspendCancellableCoroutine { cont ->
    enqueue(object : Callback {
        override fun onResponse(call: HttpCall, response: Response) {
            cont.resume(response) { _, _, _ -> response.close() }
        }

        override fun onFailure(call: HttpCall, e: IOException) {
            if (!cont.isCancelled) cont.resumeWithException(e)
        }
    })
    cont.invokeOnCancellation { cancel() }
}
