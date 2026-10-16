package com.teamofsilicons.ring.data

import kotlinx.serialization.Serializable
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.contentOrNull
import java.time.Instant

// Wire types for the Ring API v1 (udd/api.md). Property names are camelCase and map to the
// snake_case JSON through the shared Json naming strategy. Enumerations stay strings so that a
// newer server never breaks an older app; the constants below name the known values.

@Serializable
data class Account(
    val uuid: String,
    val id: String,
    val kind: String = KIND_CARBON,
    val displayName: String? = null,
    val pfpUrl: String? = null,
) {
    val isSilicon: Boolean get() = kind == KIND_SILICON

    /** Display name if known, otherwise the id. */
    val label: String get() = displayName?.takeIf { it.isNotBlank() } ?: id

    companion object {
        const val KIND_CARBON = "carbon"
        const val KIND_SILICON = "silicon"
    }
}

@Serializable
data class AccountLookup(val account: Account, val onRing: Boolean)

@Serializable
data class PushInfo(val kind: String, val environment: String? = null)

@Serializable
data class Device(
    val deviceId: String,
    val name: String? = null,
    val platform: String? = null,
    val appVersion: String? = null,
    val createdAt: String? = null,
    val lastSeenAt: String? = null,
    val online: Boolean = false,
    val ringEnabled: Boolean = true,
    val current: Boolean = false,
    val push: List<PushInfo> = emptyList(),
)

@Serializable
data class Session(
    val sessionToken: String,
    val expiresAt: String,
    val account: Account,
    val deviceId: String,
)

@Serializable
data class Me(val account: Account, val device: Device)

@Serializable
data class Voice(val id: String, val name: String, val description: String = "")

@Serializable
data class Meta(
    val appId: String = "ring",
    val version: String = "",
    val accountsUrl: String,
    val webUrl: String = "",
    val ringingSeconds: Int = 60,
    val voicemailMaxSeconds: Int = 120,
    val voices: List<Voice> = emptyList(),
    val defaultVoice: String = "gleam",
)

@Serializable
data class DeviceRef(val deviceId: String, val name: String? = null, val platform: String? = null) {
    /** How to name this device in a sentence: its name, else a platform word ("Mac"), else null. */
    val label: String? get() = name?.trim()?.takeIf { it.isNotEmpty() } ?: platformWord(platform)
}

/** A device platform as a word for sentences like "Moved to Mac"; null when unknown. */
fun platformWord(platform: String?): String? = when (platform) {
    "ios" -> "iPhone"
    "ipados" -> "iPad"
    "macos" -> "Mac"
    "android" -> "Android"
    "windows" -> "Windows"
    "linux" -> "Linux"
    "web" -> "the web"
    else -> null
}

@Serializable
data class RepresentativeInfo(val status: String, val voice: String? = null)

@Serializable
data class Participant(
    val account: Account,
    val state: String,
    val joinedAt: String? = null,
    val leftAt: String? = null,
    val invitedBy: Account? = null,
    val device: DeviceRef? = null,
    val representative: RepresentativeInfo? = null,
    val mediaIdentity: String? = null,
) {
    val joined: Boolean get() = state == "joined"
}

@Serializable
data class Invitation(
    val invitationId: String,
    val ringid: String,
    val from: Account,
    val to: Account,
    val state: String,
    val reason: String? = null,
    val createdAt: String? = null,
    val expiresAt: String? = null,
    val answeredAt: String? = null,
    val silenced: Boolean = false,
    val callUuid: String? = null,
) {
    val ringing: Boolean get() = state == InvitationState.RINGING
}

object InvitationState {
    const val RINGING = "ringing"
    const val ACCEPTED = "accepted"
    const val DECLINED = "declined"
    const val MISSED = "missed"
    const val CANCELLED = "cancelled"
    const val BUSY = "busy"
}

object CallState {
    const val RINGING = "ringing"
    const val ACTIVE = "active"
    const val VOICEMAIL = "voicemail"
    const val ENDED = "ended"
}

@Serializable
data class RecordingSummary(val status: String = "none", val durationMs: Long? = null)

@Serializable
data class Call(
    val ringid: String,
    val state: String,
    val createdAt: String,
    val answeredAt: String? = null,
    val endedAt: String? = null,
    val endReason: String? = null,
    val createdBy: Account,
    val direction: String,
    val participants: List<Participant> = emptyList(),
    val invitations: List<Invitation> = emptyList(),
    val recording: RecordingSummary = RecordingSummary(),
    val voicemailId: String? = null,
) {
    val isOutgoing: Boolean get() = direction == "outgoing"
    val isLive: Boolean get() = state != CallState.ENDED

    fun joined(): List<Participant> = participants.filter { it.joined }

    fun participant(uuid: String): Participant? = participants.lastOrNull { it.account.uuid == uuid }

    fun ringingInvitationFor(uuid: String): Invitation? =
        invitations.firstOrNull { it.to.uuid == uuid && it.ringing }

    fun latestInvitationFor(uuid: String): Invitation? = invitations.lastOrNull { it.to.uuid == uuid }

    /** The people this call is "with", from the viewer's side: everyone except [me]. */
    fun others(me: String): List<Account> {
        val seen = LinkedHashMap<String, Account>()
        if (createdBy.uuid != me) seen[createdBy.uuid] = createdBy
        participants.forEach { if (it.account.uuid != me) seen.putIfAbsent(it.account.uuid, it.account) }
        invitations.forEach { if (it.to.uuid != me) seen.putIfAbsent(it.to.uuid, it.to) }
        return seen.values.toList()
    }
}

@Serializable
data class MediaGrant(
    val url: String,
    val token: String,
    val room: String,
    val identity: String,
    val expiresAt: String? = null,
)

@Serializable
data class CreatedCall(val call: Call, val invitation: Invitation, val media: MediaGrant? = null)

@Serializable
data class CallWithMedia(val call: Call, val media: MediaGrant? = null)

@Serializable
data class CallEnvelope(val call: Call)

@Serializable
data class InvitedCall(val call: Call, val invitation: Invitation)

@Serializable
data class Page<T>(val items: List<T>, val nextCursor: String? = null)

@Serializable
data class Items<T>(val items: List<T>)

@Serializable
data class RecentPerson(
    val account: Account,
    val lastCallAt: String? = null,
    val lastRingid: String? = null,
    val calls: Long = 0,
)

object EntryKind {
    const val CALL_STARTED = "call_started"
    const val ANSWERED = "answered"
    const val JOINED = "joined"
    const val LEFT = "left"
    const val INVITED = "invited"
    const val DECLINED = "declined"
    const val MISSED = "missed"
    const val BUSY = "busy"
    const val SPEECH = "speech"
    const val VOICEMAIL = "voicemail"
    const val CALL_ENDED = "call_ended"
    const val NOTE = "note"
}

@Serializable
data class TranscriptEntry(
    val seq: Long,
    val at: String,
    val offsetMs: Long? = null,
    val kind: String,
    val actor: Account? = null,
    val text: String? = null,
    val data: JsonObject = JsonObject(emptyMap()),
) {
    fun dataString(key: String): String? = (data[key] as? JsonPrimitive)?.contentOrNull
    fun dataElement(key: String): JsonElement? = data[key]

    /** For speech: the id of the `transcript.partial` segment this entry finalises, when sent. */
    val segmentId: String? get() = dataString("segment_id")?.takeIf { it.isNotEmpty() }
}

@Serializable
data class TranscriptPage(val items: List<TranscriptEntry>, val nextAfterSeq: Long = 0)

@Serializable
data class Recording(
    val status: String,
    val durationMs: Long? = null,
    val url: String? = null,
    val expiresAt: String? = null,
    val mimeType: String? = null,
)

@Serializable
data class Voicemail(
    val voicemailId: String,
    val ringid: String,
    val from: Account,
    val to: Account,
    val reason: String,
    val declineReason: String? = null,
    val createdAt: String,
    val durationMs: Long? = null,
    val audioStatus: String = "processing",
    val transcript: String? = null,
    val transcriptStatus: String = "pending",
    val read: Boolean = false,
    val audioUrl: String? = null,
)

@Serializable
data class Greetings(val busy: String? = null, val declined: String? = null, val missed: String? = null) {
    fun get(reason: String): String? = when (reason) {
        "busy" -> busy
        "declined" -> declined
        "missed" -> missed
        else -> null
    }
}

@Serializable
data class VoicemailSettings(val enabled: Boolean = true, val greetings: Greetings = Greetings())

@Serializable
data class Settings(
    val voice: String = "gleam",
    val voicemail: VoicemailSettings = VoicemailSettings(),
    val revision: Long = 0,
)

@Serializable
data class VoiceList(val items: List<Voice>, val default: String = "gleam")

// Requests

@Serializable
data class CarbonLogin(val code: String, val codeVerifier: String, val redirectUri: String, val label: String?)

@Serializable
data class PushRegistration(val kind: String, val token: String, val environment: String? = null)

@Serializable
data class DeviceUpdate(
    val name: String? = null,
    val platform: String? = null,
    val appVersion: String? = null,
    val ringEnabled: Boolean? = null,
    val push: List<PushRegistration>? = null,
)

@Serializable
data class DevicePatch(val name: String? = null, val ringEnabled: Boolean? = null)

@Serializable
data class CreateCall(val to: String)

@Serializable
data class AcceptCall(val invitationId: String? = null)

@Serializable
data class DeclineCall(val invitationId: String? = null, val reason: String? = null, val giveNoReason: Boolean = false)

@Serializable
data class SilenceCall(val invitationId: String? = null)

@Serializable
data class InviteToCall(val to: String)

@Serializable
data class VoicemailPatch(val read: Boolean)

@Serializable
data class ApiErrorBody(val error: ApiErrorDetail)

@Serializable
data class ApiErrorDetail(
    val code: String,
    val message: String,
    val hint: String? = null,
    val retryable: Boolean = false,
    val details: JsonObject = JsonObject(emptyMap()),
)

/** Parses an RFC 3339 timestamp from the API; null for absent or malformed values. */
fun parseInstant(value: String?): Instant? = value?.let { runCatching { Instant.parse(it) }.getOrNull() }

/** Normalizes an actor reference the way the server does; null when it cannot be one. */
fun normalizeActorRef(input: String): String? {
    val s = input.trim().trimStart('@').trim()
    if (s.isEmpty()) return null
    val lower = s.lowercase()
    val handle = lower.removePrefix("c:").takeIf { lower.startsWith("c:") }
        ?: lower.removePrefix("si:").takeIf { lower.startsWith("si:") }
    if (handle != null) {
        if (handle.isEmpty() || !handle.all { it.isLetterOrDigit() && it.code < 128 || it == '_' || it == '-' || it == '.' }) return null
        return lower
    }
    val uuid = Regex("^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$")
    return if (uuid.matches(lower)) lower else null
}
