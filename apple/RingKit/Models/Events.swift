import Foundation

enum RingingStoppedReason: String, LenientStringEnum {
    case acceptedElsewhere = "accepted_elsewhere"
    case declined, silenced, cancelled, missed, unknown
    static let unknownCase = RingingStoppedReason.unknown
}

/// A server frame on `/v1/ws`: `{"type": "...", "at": "...", "data": {...}}`.
enum RealtimeEvent: Sendable {
    case ready(account: Account, deviceID: String)
    case callIncoming(call: Call, invitation: Invitation)
    case callUpdated(Call)
    case ringingStopped(ringid: String, invitationID: String, reason: RingingStoppedReason)
    case handoff(ringid: String, deviceID: String)
    case callEnded(Call)
    case transcriptEntry(ringid: String, entry: TranscriptEntry)
    case transcriptPartial(ringid: String, speaker: Account, segmentID: String, text: String)
    case voicemailReceived(Voicemail)
    case voicemailUpdated(Voicemail)
    case deviceUpdated(Device)
    case sessionRevoked
    case pong
    case other(String)

    static func decode(_ data: Data) throws -> RealtimeEvent {
        let decoder = RingJSON.decoder()
        let head = try decoder.decode(Head.self, from: data)
        func body<T: Decodable>(_ type: T.Type) throws -> T {
            try decoder.decode(Envelope<T>.self, from: data).data
        }
        switch head.type {
        case "ready":
            let d = try body(Ready.self)
            return .ready(account: d.account, deviceID: d.deviceID)
        case "call.incoming":
            let d = try body(Incoming.self)
            return .callIncoming(call: d.call, invitation: d.invitation)
        case "call.updated":
            return .callUpdated(try body(CallEnvelope.self).call)
        case "call.ringing_stopped":
            let d = try body(RingingStopped.self)
            return .ringingStopped(ringid: d.ringid, invitationID: d.invitationID, reason: d.reason)
        case "call.handoff":
            let d = try body(Handoff.self)
            return .handoff(ringid: d.ringid, deviceID: d.deviceID)
        case "call.ended":
            return .callEnded(try body(CallEnvelope.self).call)
        case "transcript.entry":
            let d = try body(Entry.self)
            return .transcriptEntry(ringid: d.ringid, entry: d.entry)
        case "transcript.partial":
            let d = try body(Partial.self)
            return .transcriptPartial(ringid: d.ringid, speaker: d.speaker, segmentID: d.segmentID, text: d.text)
        case "voicemail.received":
            return .voicemailReceived(try body(VoicemailBody.self).voicemail)
        case "voicemail.updated":
            return .voicemailUpdated(try body(VoicemailBody.self).voicemail)
        case "device.updated":
            return .deviceUpdated(try body(DeviceBody.self).device)
        case "session.revoked":
            return .sessionRevoked
        case "pong":
            return .pong
        default:
            return .other(head.type)
        }
    }

    private struct Head: Decodable { var type: String }
    private struct Envelope<T: Decodable>: Decodable { var data: T }

    private struct Ready: Decodable {
        var account: Account
        var deviceID: String
        enum CodingKeys: String, CodingKey { case account; case deviceID = "device_id" }
    }

    private struct Incoming: Decodable {
        var call: Call
        var invitation: Invitation
    }

    private struct RingingStopped: Decodable {
        var ringid: String
        var invitationID: String
        var reason: RingingStoppedReason
        enum CodingKeys: String, CodingKey { case ringid, reason; case invitationID = "invitation_id" }
    }

    private struct Handoff: Decodable {
        var ringid: String
        var deviceID: String
        enum CodingKeys: String, CodingKey { case ringid; case deviceID = "device_id" }
    }

    private struct Entry: Decodable {
        var ringid: String
        var entry: TranscriptEntry
    }

    private struct Partial: Decodable {
        var ringid: String
        var speaker: Account
        var segmentID: String
        var text: String
        enum CodingKeys: String, CodingKey { case ringid, speaker, text; case segmentID = "segment_id" }
    }

    private struct VoicemailBody: Decodable { var voicemail: Voicemail }
    private struct DeviceBody: Decodable { var device: Device }
}

// MARK: - Push payloads

struct PushPerson: Codable, Sendable, Hashable {
    var uuid: String
    var id: String
    var displayName: String?
    var pfpURL: String?

    enum CodingKeys: String, CodingKey {
        case uuid, id
        case displayName = "display_name"
        case pfpURL = "pfp_url"
    }

    var label: String {
        if let name = displayName?.trimmingCharacters(in: .whitespacesAndNewlines), !name.isEmpty { return name }
        return id
    }

    var account: Account {
        Account(uuid: uuid, id: id, kind: id.hasPrefix("si:") ? .silicon : .carbon, displayName: displayName, pfpURL: pfpURL)
    }
}

struct PushParticipant: Codable, Sendable, Hashable {
    var id: String
    var displayName: String?

    enum CodingKeys: String, CodingKey {
        case id
        case displayName = "display_name"
    }
}

/// The object every push channel carries (APNs adds `aps` beside it).
enum PushPayload: Sendable {
    case callIncoming(ringid: String, invitationID: String, callUUID: UUID, from: PushPerson, participants: [PushParticipant], expiresAt: Date?)
    case callCancelled(ringid: String, invitationID: String, callUUID: UUID, reason: RingingStoppedReason)
    case callMissed(ringid: String, from: PushPerson?)
    case voicemailReceived(voicemailID: String, from: PushPerson?, transcript: String?)

    static func parse(_ dictionary: [AnyHashable: Any]) -> PushPayload? {
        var object: [String: Any] = [:]
        for (key, value) in dictionary {
            if let key = key as? String, key != "aps" { object[key] = value }
        }
        guard JSONSerialization.isValidJSONObject(object),
              let data = try? JSONSerialization.data(withJSONObject: object),
              let raw = try? RingJSON.decoder().decode(Raw.self, from: data) else { return nil }
        switch raw.type {
        case "call.incoming":
            guard let ringid = raw.ringid, let invitationID = raw.invitationID,
                  let uuid = raw.callUUID.flatMap(UUID.init(uuidString:)), let from = raw.from else { return nil }
            return .callIncoming(ringid: ringid, invitationID: invitationID, callUUID: uuid, from: from,
                                 participants: raw.participants ?? [], expiresAt: raw.expiresAt.flatMap(RingJSON.parseDate))
        case "call.cancelled":
            guard let ringid = raw.ringid, let invitationID = raw.invitationID,
                  let uuid = raw.callUUID.flatMap(UUID.init(uuidString:)) else { return nil }
            return .callCancelled(ringid: ringid, invitationID: invitationID, callUUID: uuid,
                                  reason: raw.reason.flatMap(RingingStoppedReason.init(rawValue:)) ?? .unknown)
        case "call.missed":
            guard let ringid = raw.ringid else { return nil }
            return .callMissed(ringid: ringid, from: raw.from)
        case "voicemail.received":
            guard let id = raw.voicemailID else { return nil }
            return .voicemailReceived(voicemailID: id, from: raw.from, transcript: raw.transcript)
        default:
            return nil
        }
    }

    /// The call UUID a VoIP push refers to, even when the rest of the payload is unusable.
    static func callUUID(in dictionary: [AnyHashable: Any]) -> UUID? {
        (dictionary["call_uuid"] as? String).flatMap(UUID.init(uuidString:))
    }

    private struct Raw: Decodable {
        var type: String
        var ringid: String?
        var invitationID: String?
        var callUUID: String?
        var from: PushPerson?
        var participants: [PushParticipant]?
        var expiresAt: String?
        var reason: String?
        var voicemailID: String?
        var transcript: String?

        enum CodingKeys: String, CodingKey {
            case type, ringid, from, participants, reason, transcript
            case invitationID = "invitation_id"
            case callUUID = "call_uuid"
            case expiresAt = "expires_at"
            case voicemailID = "voicemail_id"
        }
    }
}
