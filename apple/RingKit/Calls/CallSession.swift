import Foundation
import Observation

/// A call ringing on this device, shown in-app or handed to the system call UI.
struct IncomingRing: Identifiable, Hashable, Sendable {
    var ringid: String
    var invitationID: String
    var callUUID: UUID
    var from: Account
    /// Display names of the people already on the call, other than the caller.
    var alreadyOnCall: [String]
    var expiresAt: Date?
    var call: Call?

    var id: String { invitationID }

    init(call: Call, invitation: Invitation, me: String?) {
        ringid = call.ringid
        invitationID = invitation.invitationID
        callUUID = UUID(uuidString: invitation.callUUID) ?? UUID()
        from = invitation.from
        alreadyOnCall = call.joined.map(\.account).filter { $0.uuid != invitation.from.uuid && $0.uuid != me }.map(\.label)
        expiresAt = invitation.expiresAt
        self.call = call
    }

    init(ringid: String, invitationID: String, callUUID: UUID, from: PushPerson, participants: [PushParticipant], expiresAt: Date?) {
        self.ringid = ringid
        self.invitationID = invitationID
        self.callUUID = callUUID
        self.from = from.account
        alreadyOnCall = participants.filter { !$0.isSame(as: from) }.map(\.label)
        self.expiresAt = expiresAt
        call = nil
    }
}

/// One line of the live transcript.
struct TranscriptLine: Identifiable, Hashable, Sendable {
    enum Style: Hashable, Sendable { case speech, partial, event }

    var id: String
    /// The transcript entry behind a final line; nil while the line is interim.
    var seq: Int?
    var speaker: Account?
    var text: String
    var style: Style
    var at: Date
}

/// The call this device is carrying (or about to carry) audio for.
@MainActor
@Observable
final class CallSession: Identifiable {
    enum Role: Sendable { case outgoing, incoming, handoff }

    enum Phase: Equatable, Sendable {
        case calling, ringing, active, voicemail, ended
    }

    let ringid: String
    /// The UUID this device's system call UI knows the call by.
    let localUUID: UUID
    let role: Role
    let myUUID: String
    var call: Call
    var invitationID: String?
    var phase: Phase
    var muted = false
    var mediaState: CallMedia.State = .idle
    var speakingIdentities: Set<String> = []
    /// Final lines in transcript order.
    var lines: [TranscriptLine] = []
    /// Interim speech, by segment id.
    var partials: [String: TranscriptLine] = [:]
    var endSummary: String?
    var endedAt: Date?
    var minimized = false
    private(set) var lastSeq = 0
    @ObservationIgnored private var seenSeqs: Set<Int> = []
    /// Segments whose final entry arrived; a straggling partial must not bring them back.
    @ObservationIgnored private var finishedSegments: Set<String> = []
    /// When this device took the call; early updates may still describe the previous device.
    let startedAt = Date()

    init(ringid: String, localUUID: UUID, role: Role, myUUID: String, call: Call, invitationID: String?) {
        self.ringid = ringid
        self.localUUID = localUUID
        self.role = role
        self.myUUID = myUUID
        self.call = call
        self.invitationID = invitationID
        phase = role == .outgoing ? .calling : .active
        apply(call)
    }

    nonisolated var id: String { ringid }

    /// The main person on the other end.
    var counterpart: Account { call.counterpart(me: myUUID) }

    /// Everyone else on the call right now, or the callee while it rings.
    var others: [Account] {
        let joined = call.joined.map(\.account).filter { $0.uuid != myUUID }
        if !joined.isEmpty { return joined }
        return [counterpart]
    }

    var answeredAt: Date? { call.answeredAt }

    /// Where an outgoing call stands before it is answered, as the call screen says it.
    var statusText: String {
        switch phase {
        case .calling: "Calling…"
        case .ringing:
            if outgoingInvitation?.state == .busy {
                "Busy"
            } else if mediaState == .connected {
                "Ringing"
            } else {
                // The callee's devices are being reached while this device joins the room.
                "Calling…"
            }
        case .voicemail: "Leave a message after the beep"
        case .active: "Connected"
        case .ended: endSummary ?? "Call ended"
        }
    }

    var isLive: Bool { phase != .ended }

    /// The invitation this outgoing call is waiting on.
    var outgoingInvitation: Invitation? {
        if let invitationID, let invitation = call.invitations.first(where: { $0.invitationID == invitationID }) { return invitation }
        return call.invitations.first { $0.from.uuid == myUUID }
    }

    /// Accounts whose LiveKit identity is speaking.
    var speakingUUIDs: Set<String> {
        var result = Set<String>()
        for identity in speakingIdentities {
            if let participant = call.participants.first(where: { $0.mediaIdentity == identity }) {
                result.insert(participant.account.uuid)
            } else if let uuid = Self.accountUUID(fromIdentity: identity) {
                result.insert(uuid)
            }
        }
        return result
    }

    func apply(_ call: Call) {
        self.call = call
        switch call.state {
        case .ringing:
            if phase == .calling || phase == .ringing { phase = .ringing }
        case .active: phase = .active
        case .voicemail: phase = .voicemail
        case .ended:
            phase = .ended
            endedAt = call.endedAt ?? Date()
            endSummary = Self.summary(for: call, outgoing: role == .outgoing, invitation: outgoingInvitation)
        case .unknown: break
        }
    }

    func add(_ entry: TranscriptEntry) {
        guard seenSeqs.insert(entry.seq).inserted else { return }
        lastSeq = max(lastSeq, entry.seq)
        // A final speech entry replaces its interim line: the one with its segment id, or, from a
        // server that sends none, the speaker's newest. It takes over that line's id, so the line
        // settles in place instead of jumping.
        var replaced: TranscriptLine?
        if entry.kind == .speech {
            if let segment = entry.segmentID {
                finishedSegments.insert(segment)
                replaced = partials.removeValue(forKey: segment)
            } else if let actor = entry.actor,
                      let newest = partials.filter({ $0.value.speaker?.uuid == actor.uuid }).max(by: { $0.value.at < $1.value.at }) {
                finishedSegments.insert(newest.key)
                replaced = partials.removeValue(forKey: newest.key)
            }
        }
        guard var line = Self.line(for: entry, myUUID: myUUID) else { return }
        if let replaced { line.id = replaced.id }
        // Entries can arrive out of order (the history loads while live entries stream in).
        let index = lines.lastIndex { ($0.seq ?? 0) < entry.seq }.map { $0 + 1 } ?? 0
        lines.insert(line, at: index)
        if lines.count > 200 { lines.removeFirst(lines.count - 200) }
    }

    func addPartial(speaker: Account, segmentID: String, text: String) {
        guard !finishedSegments.contains(segmentID) else { return }
        let trimmed = text.trimmingCharacters(in: .whitespacesAndNewlines)
        if trimmed.isEmpty {
            partials[segmentID] = nil
        } else {
            // Keeps the time it first appeared, so interim lines hold their order as they grow.
            let started = partials[segmentID]?.at ?? Date()
            partials[segmentID] = TranscriptLine(
                id: "segment-\(segmentID)", seq: nil, speaker: speaker, text: trimmed, style: .partial, at: started)
        }
    }

    /// Final lines followed by interim ones, oldest first.
    var visibleLines: [TranscriptLine] {
        lines + partials.values.sorted { $0.at < $1.at }
    }

    static func line(for entry: TranscriptEntry, myUUID: String) -> TranscriptLine? {
        let who = entry.actor.map { $0.uuid == myUUID ? "You" : $0.label } ?? "Someone"
        let text: String
        let style: TranscriptLine.Style
        switch entry.kind {
        case .speech:
            guard let spoken = entry.text?.trimmingCharacters(in: .whitespacesAndNewlines), !spoken.isEmpty else { return nil }
            text = spoken
            style = .speech
        case .joined:
            text = "\(who) joined"
            style = .event
        case .left:
            text = "\(who) left"
            style = .event
        case .invited:
            let invitee = entry.data["invitee"].flatMap { value -> String? in
                value["display_name"]?.stringValue ?? value["id"]?.stringValue
            } ?? "someone"
            text = "\(who) invited \(invitee)"
            style = .event
        case .declined:
            let reason = entry.data["reason"]?.stringValue
            text = reason.map { "\(who) declined · \($0)" } ?? "\(who) declined"
            style = .event
        case .note:
            guard let note = entry.text, !note.isEmpty else { return nil }
            text = note
            style = .event
        default:
            return nil
        }
        return TranscriptLine(id: "seq-\(entry.seq)", seq: entry.seq, speaker: entry.actor, text: text, style: style, at: entry.at)
    }

    static func accountUUID(fromIdentity identity: String) -> String? {
        let parts = identity.split(separator: ":")
        guard parts.count >= 2 else { return nil }
        switch parts[0] {
        case "carbon", "rep", "silicon": return String(parts[1])
        default: return nil
        }
    }

    static func summary(for call: Call, outgoing: Bool, invitation: Invitation?) -> String {
        switch call.endReason {
        case .declined:
            if let reason = invitation?.reason ?? call.invitations.first(where: { $0.state == .declined })?.reason, !reason.isEmpty {
                return "Declined · \(reason)"
            }
            return "Declined"
        case .missed: return outgoing ? "No answer" : "Missed"
        case .busy: return "Busy"
        case .cancelled: return outgoing ? "Cancelled" : "Missed"
        case .failed: return "Call failed"
        case .serverRestart: return "Call dropped"
        case .completed, .unknown, .none:
            if call.voicemailID != nil && call.answeredAt == nil { return "Message sent" }
            return "Call ended"
        }
    }
}
