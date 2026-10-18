import Foundation

// Mirrors udd/api.md (and crates/ring-proto). Field names stay exact through CodingKeys.

// MARK: - Accounts and devices

enum AccountKind: String, LenientStringEnum {
    case carbon, silicon, unknown
    static let unknownCase = AccountKind.unknown
}

struct Account: Codable, Sendable, Hashable, Identifiable {
    var uuid: String
    var id: String
    var kind: AccountKind
    var displayName: String?
    var pfpURL: String?

    enum CodingKeys: String, CodingKey {
        case uuid, id, kind
        case displayName = "display_name"
        case pfpURL = "pfp_url"
    }

    /// The display name if known, otherwise the id.
    var label: String {
        if let name = displayName?.trimmingCharacters(in: .whitespacesAndNewlines), !name.isEmpty { return name }
        return id
    }

    var hasDisplayName: Bool { label != id }
    var isSilicon: Bool { kind == .silicon }
}

struct AccountLookup: Codable, Sendable, Hashable {
    var account: Account
    var onRing: Bool

    enum CodingKeys: String, CodingKey {
        case account
        case onRing = "on_ring"
    }
}

enum Platform: String, LenientStringEnum {
    case ios, ipados, macos, android, windows, linux, web, cli, unknown
    static let unknownCase = Platform.unknown

    var label: String {
        switch self {
        case .ios: "iPhone"
        case .ipados: "iPad"
        case .macos: "Mac"
        case .android: "Android"
        case .windows: "Windows"
        case .linux: "Linux"
        case .web: "Web"
        case .cli: "CLI"
        case .unknown: "Device"
        }
    }

    var symbol: String {
        switch self {
        case .ios, .android: "iphone"
        case .ipados: "ipad"
        case .macos: "laptopcomputer"
        case .windows, .linux: "desktopcomputer"
        case .web: "globe"
        case .cli: "terminal"
        case .unknown: "questionmark.circle"
        }
    }
}

enum PushKind: String, LenientStringEnum {
    case apnsVoip = "apns_voip"
    case apns, fcm, webpush, unknown
    static let unknownCase = PushKind.unknown
}

enum PushEnvironment: String, LenientStringEnum {
    case sandbox, production, unknown
    static let unknownCase = PushEnvironment.unknown
}

struct PushRegistration: Codable, Sendable, Hashable {
    var kind: PushKind
    var token: String
    var environment: PushEnvironment?
}

struct PushInfo: Codable, Sendable, Hashable {
    var kind: PushKind
    var environment: PushEnvironment?
}

struct Device: Codable, Sendable, Hashable, Identifiable {
    var deviceID: String
    var name: String?
    var platform: Platform?
    var appVersion: String?
    var createdAt: Date
    var lastSeenAt: Date?
    var online: Bool
    var ringEnabled: Bool
    var current: Bool
    var push: [PushInfo]

    var id: String { deviceID }

    var label: String {
        if let name, !name.isEmpty { return name }
        return platform?.label ?? "Device"
    }

    enum CodingKeys: String, CodingKey {
        case name, platform, online, current, push
        case deviceID = "device_id"
        case appVersion = "app_version"
        case createdAt = "created_at"
        case lastSeenAt = "last_seen_at"
        case ringEnabled = "ring_enabled"
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        deviceID = try c.decode(String.self, forKey: .deviceID)
        name = try c.decodeIfPresent(String.self, forKey: .name)
        platform = try c.decodeIfPresent(Platform.self, forKey: .platform)
        appVersion = try c.decodeIfPresent(String.self, forKey: .appVersion)
        createdAt = try c.decode(Date.self, forKey: .createdAt)
        lastSeenAt = try c.decodeIfPresent(Date.self, forKey: .lastSeenAt)
        online = try c.decodeIfPresent(Bool.self, forKey: .online) ?? false
        ringEnabled = try c.decodeIfPresent(Bool.self, forKey: .ringEnabled) ?? true
        current = try c.decodeIfPresent(Bool.self, forKey: .current) ?? false
        push = try c.decodeIfPresent([PushInfo].self, forKey: .push) ?? []
    }
}

/// `PUT /v1/devices/current`. Absent fields are left unchanged.
struct DeviceUpdate: Encodable, Sendable {
    var name: String?
    var platform: Platform?
    var appVersion: String?
    var ringEnabled: Bool?
    var push: [PushRegistration]?

    enum CodingKeys: String, CodingKey {
        case name, platform, push
        case appVersion = "app_version"
        case ringEnabled = "ring_enabled"
    }

    func encode(to encoder: Encoder) throws {
        var c = encoder.container(keyedBy: CodingKeys.self)
        try c.encodeIfPresent(name, forKey: .name)
        try c.encodeIfPresent(platform, forKey: .platform)
        try c.encodeIfPresent(appVersion, forKey: .appVersion)
        try c.encodeIfPresent(ringEnabled, forKey: .ringEnabled)
        try c.encodeIfPresent(push, forKey: .push)
    }
}

/// `PATCH /v1/devices/{device_id}`.
struct DevicePatch: Encodable, Sendable {
    var name: String?
    var ringEnabled: Bool?

    enum CodingKeys: String, CodingKey {
        case name
        case ringEnabled = "ring_enabled"
    }

    func encode(to encoder: Encoder) throws {
        var c = encoder.container(keyedBy: CodingKeys.self)
        try c.encodeIfPresent(name, forKey: .name)
        try c.encodeIfPresent(ringEnabled, forKey: .ringEnabled)
    }
}

struct SessionGrant: Codable, Sendable {
    var sessionToken: String
    var expiresAt: Date
    var account: Account
    var deviceID: String

    enum CodingKeys: String, CodingKey {
        case account
        case sessionToken = "session_token"
        case expiresAt = "expires_at"
        case deviceID = "device_id"
    }
}

struct Me: Codable, Sendable {
    var account: Account
    var device: Device
}

/// `POST /v1/auth/carbon`.
struct CarbonLogin: Encodable, Sendable {
    var code: String
    var codeVerifier: String
    var redirectURI: String
    var label: String?

    enum CodingKeys: String, CodingKey {
        case code, label
        case codeVerifier = "code_verifier"
        case redirectURI = "redirect_uri"
    }
}

// MARK: - Meta

struct Limits: Codable, Sendable, Hashable {
    var context: Int
    var start: Int
    var message: Int
    var declineReason: Int
    var voicemailText: Int
    var greeting: Int

    enum CodingKeys: String, CodingKey {
        case context, start, message, greeting
        case declineReason = "decline_reason"
        case voicemailText = "voicemail_text"
    }

    static let standard = Limits(context: 400, start: 100, message: 160, declineReason: 200, voicemailText: 600, greeting: 300)
}

struct RedirectURIs: Codable, Sendable, Hashable {
    var native: String
    var loopback: String
}

struct Voice: Codable, Sendable, Hashable, Identifiable {
    var id: String
    var name: String
    var description: String
}

struct Meta: Codable, Sendable {
    var appID: String
    var version: String
    var apiVersion: String
    var accountsURL: String
    var webURL: String
    var redirectURIs: RedirectURIs
    var limits: Limits
    var ringingSeconds: Int
    var voicemailMaxSeconds: Int
    var voices: [Voice]
    var defaultVoice: String
    var webpushPublicKey: String?

    enum CodingKeys: String, CodingKey {
        case version, limits, voices
        case appID = "app_id"
        case apiVersion = "api_version"
        case accountsURL = "accounts_url"
        case webURL = "web_url"
        case redirectURIs = "redirect_uris"
        case ringingSeconds = "ringing_seconds"
        case voicemailMaxSeconds = "voicemail_max_seconds"
        case defaultVoice = "default_voice"
        case webpushPublicKey = "webpush_public_key"
    }
}

struct VoiceList: Codable, Sendable {
    var items: [Voice]
    var `default`: String
}

// MARK: - Calls

enum CallState: String, LenientStringEnum {
    case ringing, active, voicemail, ended, unknown
    static let unknownCase = CallState.unknown

    var isLive: Bool { self == .ringing || self == .active || self == .voicemail }
}

enum EndReason: String, LenientStringEnum {
    case completed, cancelled, declined, missed, busy, failed
    case serverRestart = "server_restart"
    case unknown
    static let unknownCase = EndReason.unknown
}

enum CallDirection: String, LenientStringEnum {
    case incoming, outgoing, unknown
    static let unknownCase = CallDirection.unknown
}

enum RecordingStatus: String, LenientStringEnum {
    case none, recording, processing, ready, failed, unknown
    static let unknownCase = RecordingStatus.unknown
}

struct RecordingSummary: Codable, Sendable, Hashable {
    var status: RecordingStatus
    var durationMS: Int?

    enum CodingKeys: String, CodingKey {
        case status
        case durationMS = "duration_ms"
    }
}

enum ParticipantState: String, LenientStringEnum {
    case joined, left, unknown
    static let unknownCase = ParticipantState.unknown
}

struct DeviceRef: Codable, Sendable, Hashable {
    var deviceID: String
    var name: String?
    var platform: Platform?

    enum CodingKeys: String, CodingKey {
        case name, platform
        case deviceID = "device_id"
    }

    /// The device's name ("Pixel 8"), or a platform word when it has none.
    var label: String {
        if let name = name?.trimmingCharacters(in: .whitespacesAndNewlines), !name.isEmpty { return name }
        return platform?.label ?? "another device"
    }
}

enum RepresentativeStatus: String, LenientStringEnum {
    case starting, live, failed, ended, unknown
    static let unknownCase = RepresentativeStatus.unknown
}

struct RepresentativeInfo: Codable, Sendable, Hashable {
    var status: RepresentativeStatus
    var voice: String
}

struct Participant: Codable, Sendable, Hashable {
    var account: Account
    var state: ParticipantState
    var joinedAt: Date
    var leftAt: Date?
    var invitedBy: Account?
    var device: DeviceRef?
    var representative: RepresentativeInfo?
    var mediaIdentity: String?

    enum CodingKeys: String, CodingKey {
        case account, state, device, representative
        case joinedAt = "joined_at"
        case leftAt = "left_at"
        case invitedBy = "invited_by"
        case mediaIdentity = "media_identity"
    }
}

enum InvitationState: String, LenientStringEnum {
    case ringing, accepted, declined, missed, cancelled, busy, unknown
    static let unknownCase = InvitationState.unknown
}

struct Invitation: Codable, Sendable, Hashable, Identifiable {
    var invitationID: String
    var ringid: String
    var from: Account
    var to: Account
    var state: InvitationState
    var reason: String?
    var createdAt: Date
    var expiresAt: Date
    var answeredAt: Date?
    var silenced: Bool
    var callUUID: String

    var id: String { invitationID }

    enum CodingKeys: String, CodingKey {
        case ringid, from, to, state, reason, silenced
        case invitationID = "invitation_id"
        case createdAt = "created_at"
        case expiresAt = "expires_at"
        case answeredAt = "answered_at"
        case callUUID = "call_uuid"
    }
}

struct Call: Codable, Sendable, Hashable, Identifiable {
    var ringid: String
    var state: CallState
    var createdAt: Date
    var answeredAt: Date?
    var endedAt: Date?
    var endReason: EndReason?
    var createdBy: Account
    var direction: CallDirection
    var participants: [Participant]
    var invitations: [Invitation]
    var recording: RecordingSummary
    var voicemailID: String?

    var id: String { ringid }

    enum CodingKeys: String, CodingKey {
        case ringid, state, direction, participants, invitations, recording
        case createdAt = "created_at"
        case answeredAt = "answered_at"
        case endedAt = "ended_at"
        case endReason = "end_reason"
        case createdBy = "created_by"
        case voicemailID = "voicemail_id"
    }

    var joined: [Participant] { participants.filter { $0.state == .joined } }

    func participant(uuid: String) -> Participant? {
        participants.last { $0.account.uuid == uuid }
    }

    func ringingInvitation(for uuid: String) -> Invitation? {
        invitations.first { $0.to.uuid == uuid && $0.state == .ringing }
    }

    /// The person this call is "with", from the viewer's side.
    func counterpart(me: String?) -> Account {
        if direction == .incoming && createdBy.uuid != me { return createdBy }
        if let invitee = invitations.first(where: { $0.to.uuid != me }) { return invitee.to }
        if let other = participants.first(where: { $0.account.uuid != me }) { return other.account }
        return createdBy
    }

    /// Everyone except the viewer, in join order.
    func others(me: String?) -> [Account] {
        var seen = Set<String>()
        var result: [Account] = []
        for account in participants.map(\.account) + invitations.map(\.to) + [createdBy] where account.uuid != me {
            if seen.insert(account.uuid).inserted { result.append(account) }
        }
        return result
    }

    var durationMS: Int? {
        guard let answeredAt else { return nil }
        let end = endedAt ?? Date()
        return Int(end.timeIntervalSince(answeredAt) * 1000)
    }
}

struct MediaGrant: Codable, Sendable, Hashable {
    var url: String
    var token: String
    var room: String
    var identity: String
    var expiresAt: Date
    /// Where the microphone's pre-roll goes (see `CallMedia`); absent when nobody would use it.
    var prerollTo: String?

    enum CodingKeys: String, CodingKey {
        case url, token, room, identity
        case expiresAt = "expires_at"
        case prerollTo = "preroll_to"
    }
}

/// `POST /v1/calls`. Carbons send only `to`.
struct CreateCall: Encodable, Sendable {
    var to: String
}

struct CreatedCall: Decodable, Sendable {
    var call: Call
    var invitation: Invitation
    var media: MediaGrant?
}

/// `POST /v1/calls/{ringid}/accept`.
struct AcceptCall: Encodable, Sendable {
    var invitationID: String?

    enum CodingKeys: String, CodingKey {
        case invitationID = "invitation_id"
    }

    func encode(to encoder: Encoder) throws {
        var c = encoder.container(keyedBy: CodingKeys.self)
        try c.encodeIfPresent(invitationID, forKey: .invitationID)
    }
}

struct CallWithMedia: Decodable, Sendable {
    var call: Call
    var media: MediaGrant?
}

/// `POST /v1/calls/{ringid}/decline`.
struct DeclineCall: Encodable, Sendable {
    var invitationID: String?
    var reason: String?
    var giveNoReason: Bool

    enum CodingKeys: String, CodingKey {
        case reason
        case invitationID = "invitation_id"
        case giveNoReason = "give_no_reason"
    }

    func encode(to encoder: Encoder) throws {
        var c = encoder.container(keyedBy: CodingKeys.self)
        try c.encodeIfPresent(invitationID, forKey: .invitationID)
        try c.encodeIfPresent(reason, forKey: .reason)
        try c.encode(giveNoReason, forKey: .giveNoReason)
    }
}

/// `POST /v1/calls/{ringid}/silence`.
struct SilenceCall: Encodable, Sendable {
    var invitationID: String?

    enum CodingKeys: String, CodingKey {
        case invitationID = "invitation_id"
    }

    func encode(to encoder: Encoder) throws {
        var c = encoder.container(keyedBy: CodingKeys.self)
        try c.encodeIfPresent(invitationID, forKey: .invitationID)
    }
}

struct InviteToCall: Encodable, Sendable {
    var to: String
}

struct CallEnvelope: Decodable, Sendable {
    var call: Call
}

struct InvitedCall: Decodable, Sendable {
    var call: Call
    var invitation: Invitation
}

struct Page<Item: Decodable & Sendable>: Decodable, Sendable {
    var items: [Item]
    var nextCursor: String?

    enum CodingKeys: String, CodingKey {
        case items
        case nextCursor = "next_cursor"
    }
}

struct Items<Item: Decodable & Sendable>: Decodable, Sendable {
    var items: [Item]
}

struct RecentPerson: Codable, Sendable, Hashable, Identifiable {
    var account: Account
    var lastCallAt: Date
    var lastRingid: String
    var calls: Int

    var id: String { account.uuid }

    enum CodingKeys: String, CodingKey {
        case account, calls
        case lastCallAt = "last_call_at"
        case lastRingid = "last_ringid"
    }
}

// MARK: - Transcript

enum EntryKind: String, LenientStringEnum {
    case callStarted = "call_started"
    case answered, joined, left, invited, declined, missed, busy, speech, delegation, thinking, commentary, voicemail
    case callEnded = "call_ended"
    case note, unknown
    static let unknownCase = EntryKind.unknown
}

struct TranscriptEntry: Codable, Sendable, Hashable, Identifiable {
    var seq: Int
    var at: Date
    var offsetMS: Int?
    var kind: EntryKind
    var actor: Account?
    var text: String?
    var data: JSONValue

    var id: Int { seq }

    /// For `speech`: the id of the `transcript.partial` events this entry finalises, when sent.
    var segmentID: String? { data["segment_id"]?.stringValue }

    enum CodingKeys: String, CodingKey {
        case seq, at, kind, actor, text, data
        case offsetMS = "offset_ms"
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        seq = try c.decode(Int.self, forKey: .seq)
        at = try c.decode(Date.self, forKey: .at)
        offsetMS = try c.decodeIfPresent(Int.self, forKey: .offsetMS)
        kind = try c.decode(EntryKind.self, forKey: .kind)
        actor = try c.decodeIfPresent(Account.self, forKey: .actor)
        text = try c.decodeIfPresent(String.self, forKey: .text)
        data = try c.decodeIfPresent(JSONValue.self, forKey: .data) ?? .emptyObject
    }
}

struct TranscriptPage: Decodable, Sendable {
    var items: [TranscriptEntry]
    var nextAfterSeq: Int

    enum CodingKeys: String, CodingKey {
        case items
        case nextAfterSeq = "next_after_seq"
    }
}

struct Recording: Decodable, Sendable {
    var status: RecordingStatus
    var durationMS: Int?
    var url: String?
    var expiresAt: Date?
    var mimeType: String?

    enum CodingKeys: String, CodingKey {
        case status, url
        case durationMS = "duration_ms"
        case expiresAt = "expires_at"
        case mimeType = "mime_type"
    }
}

// MARK: - Voicemail

enum VoicemailReason: String, LenientStringEnum {
    case busy, declined, missed, unknown
    static let unknownCase = VoicemailReason.unknown
}

enum AudioStatus: String, LenientStringEnum {
    case recording, processing, ready, failed, unknown
    static let unknownCase = AudioStatus.unknown
}

enum TranscriptStatus: String, LenientStringEnum {
    case pending, ready, failed, unknown
    static let unknownCase = TranscriptStatus.unknown
}

struct Voicemail: Codable, Sendable, Hashable, Identifiable {
    var voicemailID: String
    var ringid: String
    var from: Account
    var to: Account
    var reason: VoicemailReason
    var declineReason: String?
    var createdAt: Date
    var durationMS: Int?
    var audioStatus: AudioStatus
    var transcript: String?
    var transcriptStatus: TranscriptStatus
    var read: Bool
    var audioURL: String?

    var id: String { voicemailID }

    enum CodingKeys: String, CodingKey {
        case ringid, from, to, reason, transcript, read
        case voicemailID = "voicemail_id"
        case declineReason = "decline_reason"
        case createdAt = "created_at"
        case durationMS = "duration_ms"
        case audioStatus = "audio_status"
        case transcriptStatus = "transcript_status"
        case audioURL = "audio_url"
    }
}

struct VoicemailReadPatch: Encodable, Sendable {
    var read: Bool
}

// MARK: - Settings

enum ContextMode: String, LenientStringEnum {
    case append, prepend, overwrite, unknown
    static let unknownCase = ContextMode.unknown
}

struct RepresentativeSettings: Codable, Sendable, Hashable {
    var defaultContext: String
    var contextMode: ContextMode
    var contextRequired: Bool

    enum CodingKeys: String, CodingKey {
        case defaultContext = "default_context"
        case contextMode = "context_mode"
        case contextRequired = "context_required"
    }
}

struct Greetings: Codable, Sendable, Hashable {
    var busy: String?
    var declined: String?
    var missed: String?

    subscript(reason: VoicemailReason) -> String? {
        switch reason {
        case .busy: busy
        case .declined: declined
        case .missed: missed
        case .unknown: nil
        }
    }
}

struct VoicemailSettings: Codable, Sendable, Hashable {
    var enabled: Bool
    var greetings: Greetings
}

struct Settings: Codable, Sendable, Hashable {
    var voice: String
    var representative: RepresentativeSettings
    var voicemail: VoicemailSettings
    var revision: Int
}

/// Builders for `PATCH /v1/settings`, which takes any subset; a greeting sent as `null` resets it.
enum SettingsPatch {
    static func voice(_ id: String) -> JSONValue {
        .object(["voice": .string(id)])
    }

    static func voicemailEnabled(_ enabled: Bool) -> JSONValue {
        .object(["voicemail": .object(["enabled": .bool(enabled)])])
    }

    static func greeting(_ reason: VoicemailReason, _ text: String?) -> JSONValue {
        .object(["voicemail": .object(["greetings": .object([reason.rawValue: text.map(JSONValue.string) ?? .null])])])
    }
}

// MARK: - Errors

struct APIErrorInfo: Codable, Sendable, Hashable {
    var code: String
    var message: String
    var hint: String?
    var retryable: Bool
    var details: JSONValue

    init(code: String, message: String, hint: String? = nil, retryable: Bool = false, details: JSONValue = .emptyObject) {
        self.code = code
        self.message = message
        self.hint = hint
        self.retryable = retryable
        self.details = details
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        code = try c.decode(String.self, forKey: .code)
        message = try c.decodeIfPresent(String.self, forKey: .message) ?? code
        hint = try c.decodeIfPresent(String.self, forKey: .hint)
        retryable = try c.decodeIfPresent(Bool.self, forKey: .retryable) ?? false
        details = try c.decodeIfPresent(JSONValue.self, forKey: .details) ?? .emptyObject
    }
}

struct APIErrorBody: Decodable, Sendable {
    var error: APIErrorInfo
}
