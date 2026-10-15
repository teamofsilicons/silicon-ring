using System.Text.Json;
using System.Text.Json.Nodes;
using System.Text.Json.Serialization;

namespace Ring.Api;

// Wire types for the Ring API v1 (udd/api.md). Property names map to snake_case through
// RingJson.Options. Enumerations stay strings so an unknown value from a newer server never
// breaks deserialization; the constants below name the values this client understands.

public static class RingJson
{
    public static readonly JsonSerializerOptions Options = new(JsonSerializerDefaults.Web)
    {
        PropertyNamingPolicy = JsonNamingPolicy.SnakeCaseLower,
        DefaultIgnoreCondition = JsonIgnoreCondition.WhenWritingNull,
    };

    public static T Parse<T>(JsonElement element) =>
        element.Deserialize<T>(Options) ?? throw new JsonException($"Expected {typeof(T).Name}, got null.");
}

public static class AccountKind
{
    public const string Carbon = "carbon";
    public const string Silicon = "silicon";
}

public static class CallStates
{
    public const string Ringing = "ringing";
    public const string Active = "active";
    public const string Voicemail = "voicemail";
    public const string Ended = "ended";
}

public static class InvitationStates
{
    public const string Ringing = "ringing";
    public const string Accepted = "accepted";
    public const string Declined = "declined";
    public const string Missed = "missed";
    public const string Cancelled = "cancelled";
    public const string Busy = "busy";
}

public static class EndReasons
{
    public const string Completed = "completed";
    public const string Cancelled = "cancelled";
    public const string Declined = "declined";
    public const string Missed = "missed";
    public const string Busy = "busy";
    public const string Failed = "failed";
    public const string ServerRestart = "server_restart";
}

public static class EntryKinds
{
    public const string CallStarted = "call_started";
    public const string Answered = "answered";
    public const string Joined = "joined";
    public const string Left = "left";
    public const string Invited = "invited";
    public const string Declined = "declined";
    public const string Missed = "missed";
    public const string Busy = "busy";
    public const string Speech = "speech";
    public const string Voicemail = "voicemail";
    public const string CallEnded = "call_ended";
    public const string Note = "note";
}

public sealed class Account
{
    public string Uuid { get; init; } = "";
    public string Id { get; init; } = "";
    public string Kind { get; init; } = AccountKind.Carbon;
    public string? DisplayName { get; init; }
    public string? PfpUrl { get; init; }

    [JsonIgnore]
    public bool IsSilicon => Kind == AccountKind.Silicon;

    /// <summary>The display name when known, otherwise the id.</summary>
    [JsonIgnore]
    public string Label => string.IsNullOrWhiteSpace(DisplayName) ? Id : DisplayName!.Trim();

    [JsonIgnore]
    public bool HasDisplayName => !string.IsNullOrWhiteSpace(DisplayName);
}

public sealed class AccountLookup
{
    public Account Account { get; init; } = new();
    public bool OnRing { get; init; }
}

public sealed class PushInfo
{
    public string Kind { get; init; } = "";
    public string? Environment { get; init; }
}

public sealed class Device
{
    public string DeviceId { get; init; } = "";
    public string? Name { get; init; }
    public string? Platform { get; init; }
    public string? AppVersion { get; init; }
    public DateTimeOffset CreatedAt { get; init; }
    public DateTimeOffset? LastSeenAt { get; init; }
    public bool Online { get; init; }
    public bool RingEnabled { get; init; }
    public bool Current { get; init; }
    public List<PushInfo> Push { get; init; } = [];
}

public sealed class DeviceRef
{
    public string DeviceId { get; init; } = "";
    public string? Name { get; init; }
    public string? Platform { get; init; }
}

public sealed class SessionGrant
{
    public string SessionToken { get; init; } = "";
    public DateTimeOffset ExpiresAt { get; init; }
    public Account Account { get; init; } = new();
    public string DeviceId { get; init; } = "";
}

public sealed class MeResponse
{
    public Account Account { get; init; } = new();
    public Device Device { get; init; } = new();
}

public sealed class RedirectUris
{
    public string Native { get; init; } = "";
    public string Loopback { get; init; } = "";
}

public sealed class Limits
{
    public int Context { get; init; } = 400;
    public int Start { get; init; } = 100;
    public int Message { get; init; } = 160;
    public int DeclineReason { get; init; } = 200;
    public int VoicemailText { get; init; } = 600;
    public int Greeting { get; init; } = 300;
}

public sealed class Voice
{
    public string Id { get; init; } = "";
    public string Name { get; init; } = "";
    public string Description { get; init; } = "";
}

public sealed class VoiceList
{
    public List<Voice> Items { get; init; } = [];
    public string Default { get; init; } = "gleam";
}

public sealed class Meta
{
    public string AppId { get; init; } = "ring";
    public string Version { get; init; } = "";
    public string ApiVersion { get; init; } = "v1";
    public string AccountsUrl { get; init; } = "https://accounts.teamofsilicons.com";
    public string WebUrl { get; init; } = "https://ring.teamofsilicons.com";
    public RedirectUris? RedirectUris { get; init; }
    public Limits Limits { get; init; } = new();
    public int RingingSeconds { get; init; } = 60;
    public int VoicemailMaxSeconds { get; init; } = 120;
    public List<Voice> Voices { get; init; } = [];
    public string DefaultVoice { get; init; } = "gleam";
    public string? WebpushPublicKey { get; init; }
}

public sealed class RecordingSummary
{
    public string Status { get; init; } = "none";
    public long? DurationMs { get; init; }
}

public sealed class RepresentativeInfo
{
    public string Status { get; init; } = "";
    public string Voice { get; init; } = "";
}

public sealed class Participant
{
    public Account Account { get; init; } = new();
    public string State { get; init; } = "joined";
    public DateTimeOffset JoinedAt { get; init; }
    public DateTimeOffset? LeftAt { get; init; }
    public Account? InvitedBy { get; init; }
    public DeviceRef? Device { get; init; }
    public RepresentativeInfo? Representative { get; init; }
    public string? MediaIdentity { get; init; }

    [JsonIgnore]
    public bool IsJoined => State == "joined";
}

public sealed class Invitation
{
    public string InvitationId { get; init; } = "";
    public string Ringid { get; init; } = "";
    public Account From { get; init; } = new();
    public Account To { get; init; } = new();
    public string State { get; init; } = InvitationStates.Ringing;
    public string? Reason { get; init; }
    public DateTimeOffset CreatedAt { get; init; }
    public DateTimeOffset ExpiresAt { get; init; }
    public DateTimeOffset? AnsweredAt { get; init; }
    public bool Silenced { get; init; }
    public string CallUuid { get; init; } = "";
}

public sealed class Call
{
    public string Ringid { get; init; } = "";
    public string State { get; init; } = CallStates.Ringing;
    public DateTimeOffset CreatedAt { get; init; }
    public DateTimeOffset? AnsweredAt { get; init; }
    public DateTimeOffset? EndedAt { get; init; }
    public string? EndReason { get; init; }
    public Account CreatedBy { get; init; } = new();
    public string Direction { get; init; } = "outgoing";
    public List<Participant> Participants { get; init; } = [];
    public List<Invitation> Invitations { get; init; } = [];
    public RecordingSummary Recording { get; init; } = new();
    public string? VoicemailId { get; init; }

    [JsonIgnore]
    public bool IsOpen => State != CallStates.Ended;

    public Participant? ParticipantFor(string uuid) =>
        Participants.LastOrDefault(p => p.Account.Uuid == uuid);

    public IEnumerable<Participant> JoinedParticipants => Participants.Where(p => p.IsJoined);

    public Invitation? RingingInvitationFor(string uuid) =>
        Invitations.FirstOrDefault(i => i.To.Uuid == uuid && i.State == InvitationStates.Ringing);

    public Invitation? InvitationFor(string uuid) =>
        Invitations.LastOrDefault(i => i.To.Uuid == uuid);
}

public sealed class MediaGrant
{
    public string Url { get; init; } = "";
    public string Token { get; init; } = "";
    public string Room { get; init; } = "";
    public string Identity { get; init; } = "";
    public DateTimeOffset ExpiresAt { get; init; }
}

public sealed class CreatedCall
{
    public Call Call { get; init; } = new();
    public Invitation Invitation { get; init; } = new();
    public MediaGrant? Media { get; init; }
}

public sealed class CallWithMedia
{
    public Call Call { get; init; } = new();
    public MediaGrant? Media { get; init; }
}

public sealed class CallEnvelope
{
    public Call Call { get; init; } = new();
}

public sealed class InvitedCall
{
    public Call Call { get; init; } = new();
    public Invitation Invitation { get; init; } = new();
}

public sealed class ApiPage<T>
{
    public List<T> Items { get; init; } = [];
    public string? NextCursor { get; init; }
}

public sealed class ItemsResponse<T>
{
    public List<T> Items { get; init; } = [];
}

public sealed class RecentPerson
{
    public Account Account { get; init; } = new();
    public DateTimeOffset LastCallAt { get; init; }
    public string LastRingid { get; init; } = "";
    public long Calls { get; init; }
}

public sealed class TranscriptEntry
{
    public long Seq { get; init; }
    public DateTimeOffset At { get; init; }
    public long? OffsetMs { get; init; }
    public string Kind { get; init; } = "";
    public Account? Actor { get; init; }
    public string? Text { get; init; }
    public JsonElement Data { get; init; }

    public string? DataString(string name) =>
        Data.ValueKind == JsonValueKind.Object && Data.TryGetProperty(name, out var v) && v.ValueKind == JsonValueKind.String
            ? v.GetString()
            : null;

    public Account? DataAccount(string name)
    {
        if (Data.ValueKind != JsonValueKind.Object || !Data.TryGetProperty(name, out var v) || v.ValueKind != JsonValueKind.Object)
        {
            return null;
        }
        return v.Deserialize<Account>(RingJson.Options);
    }
}

public sealed class TranscriptPage
{
    public List<TranscriptEntry> Items { get; init; } = [];
    public long NextAfterSeq { get; init; }
}

public sealed class Recording
{
    public string Status { get; init; } = "none";
    public long? DurationMs { get; init; }
    public string? Url { get; init; }
    public DateTimeOffset? ExpiresAt { get; init; }
    public string? MimeType { get; init; }
}

public sealed class Voicemail
{
    public string VoicemailId { get; init; } = "";
    public string Ringid { get; init; } = "";
    public Account From { get; init; } = new();
    public Account To { get; init; } = new();
    public string Reason { get; init; } = "missed";
    public string? DeclineReason { get; init; }
    public DateTimeOffset CreatedAt { get; init; }
    public long? DurationMs { get; init; }
    public string AudioStatus { get; init; } = "processing";
    public string? Transcript { get; init; }
    public string TranscriptStatus { get; init; } = "pending";
    public bool Read { get; init; }
    public string? AudioUrl { get; init; }
}

public sealed class RepresentativeSettings
{
    public string DefaultContext { get; init; } = "";
    public string ContextMode { get; init; } = "append";
    public bool ContextRequired { get; init; }
}

public sealed class Greetings
{
    // Explicit nulls matter here: null means "use the built-in greeting".
    [JsonIgnore(Condition = JsonIgnoreCondition.Never)]
    public string? Busy { get; init; }

    [JsonIgnore(Condition = JsonIgnoreCondition.Never)]
    public string? Declined { get; init; }

    [JsonIgnore(Condition = JsonIgnoreCondition.Never)]
    public string? Missed { get; init; }
}

public sealed class VoicemailSettings
{
    public bool Enabled { get; init; } = true;
    public Greetings Greetings { get; init; } = new();
}

public sealed class RingSettings
{
    public string Voice { get; init; } = "gleam";
    public RepresentativeSettings Representative { get; init; } = new();
    public VoicemailSettings Voicemail { get; init; } = new();
    public long Revision { get; init; }
}

// ---- requests ---------------------------------------------------------------------------------

public sealed class CarbonLoginRequest
{
    public string Code { get; init; } = "";
    public string CodeVerifier { get; init; } = "";
    public string RedirectUri { get; init; } = "";
    public string? Label { get; init; }
}

public sealed class PushRegistration
{
    public string Kind { get; init; } = "";
    public string Token { get; init; } = "";
    public string? Environment { get; init; }
}

public sealed class DeviceUpdate
{
    public string? Name { get; init; }
    public string? Platform { get; init; }
    public string? AppVersion { get; init; }
    public bool? RingEnabled { get; init; }
    public List<PushRegistration>? Push { get; init; }
}

public sealed class DevicePatch
{
    public string? Name { get; init; }
    public bool? RingEnabled { get; init; }
}

public sealed class CreateCallRequest
{
    public string To { get; init; } = "";
}

public sealed class AcceptRequest
{
    public string? InvitationId { get; init; }
}

public sealed class DeclineRequest
{
    public string? InvitationId { get; init; }
    public string? Reason { get; init; }
    public bool GiveNoReason { get; init; }
}

public sealed class SilenceRequest
{
    public string? InvitationId { get; init; }
}

public sealed class InviteRequest
{
    public string To { get; init; } = "";
}

public sealed class VoicemailReadPatch
{
    public bool Read { get; init; }
}

/// <summary>Builds the partial body for <c>PATCH /v1/settings</c>.</summary>
public static class SettingsPatch
{
    public static JsonObject VoicemailEnabled(bool enabled) =>
        new() { ["voicemail"] = new JsonObject { ["enabled"] = enabled } };

    /// <summary><paramref name="text"/> null resets the greeting to the built-in default.</summary>
    public static JsonObject Greeting(string reason, string? text) =>
        new() { ["voicemail"] = new JsonObject { ["greetings"] = new JsonObject { [reason] = text is null ? null : JsonValue.Create(text) } } };

    public static JsonObject Voice(string voice) => new() { ["voice"] = voice };
}

// ---- realtime ---------------------------------------------------------------------------------

/// <summary>One server frame on <c>/v1/ws</c>: <c>{"type","at","data"}</c>.</summary>
public sealed class RealtimeEvent
{
    public string Type { get; init; } = "";
    public DateTimeOffset? At { get; init; }
    public JsonElement Data { get; init; }

    public T DataAs<T>() => Data.ValueKind == JsonValueKind.Object
        ? RingJson.Parse<T>(Data)
        : throw new JsonException($"The {Type} event carried no data.");
}

public static class EventTypes
{
    public const string Ready = "ready";
    public const string CallIncoming = "call.incoming";
    public const string CallUpdated = "call.updated";
    public const string CallRingingStopped = "call.ringing_stopped";
    public const string CallHandoff = "call.handoff";
    public const string CallEnded = "call.ended";
    public const string TranscriptEntry = "transcript.entry";
    public const string TranscriptPartial = "transcript.partial";
    public const string DelegationCreated = "delegation.created";
    public const string VoicemailReceived = "voicemail.received";
    public const string VoicemailUpdated = "voicemail.updated";
    public const string DeviceUpdated = "device.updated";
    public const string SessionRevoked = "session.revoked";
    public const string Pong = "pong";
}

public sealed class ReadyData
{
    public Account Account { get; init; } = new();
    public string DeviceId { get; init; } = "";
}

public sealed class CallIncomingData
{
    public Call Call { get; init; } = new();
    public Invitation Invitation { get; init; } = new();
}

public sealed class RingingStoppedData
{
    public string Ringid { get; init; } = "";
    public string InvitationId { get; init; } = "";
    public string Reason { get; init; } = "";
}

public sealed class HandoffData
{
    public string Ringid { get; init; } = "";
    public string DeviceId { get; init; } = "";
}

public sealed class TranscriptEntryData
{
    public string Ringid { get; init; } = "";
    public TranscriptEntry Entry { get; init; } = new();
}

public sealed class TranscriptPartialData
{
    public string Ringid { get; init; } = "";
    public Account Speaker { get; init; } = new();
    public string SegmentId { get; init; } = "";
    public string Text { get; init; } = "";
}

public sealed class VoicemailData
{
    public Voicemail Voicemail { get; init; } = new();
}

public sealed class DeviceData
{
    public Device Device { get; init; } = new();
}

// ---- errors -----------------------------------------------------------------------------------

public sealed class ApiErrorInfo
{
    public string Code { get; init; } = "";
    public string Message { get; init; } = "";
    public string? Hint { get; init; }
    public bool Retryable { get; init; }
    public JsonElement Details { get; init; }
}

public sealed class ApiErrorBody
{
    public ApiErrorInfo? Error { get; init; }
}
