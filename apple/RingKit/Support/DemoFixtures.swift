#if DEBUG && os(iOS)
import Foundation

/// Made-up people, silicons, calls and voicemail for demo mode (see DemoMode.swift). Every name
/// is fictional except TOS. History is laid out around 9:41 today, the time the screenshots'
/// status bar shows; the live calls run from the real clock, so their timers count up.
struct DemoFixtures: Sendable {
    let me: Account
    let deviceID = "dev_01kdemo7q2m4x8r3b6n9c5v1t0"
    let device: Device
    let meta: Meta
    let settings: Settings
    /// Every call, newest first: the live one (if any) and the history.
    let calls: [Call]
    let voicemails: [Voicemail]
    let people: [RecentPerson]
    /// The call the `call` scene is on, with TOS's representative.
    let activeCall: Call?
    /// The call ringing in the `incoming` scene, and its invitation to this account.
    let ringing: (call: Call, invitation: Invitation)?
    /// What has been said so far on the active call, and what TOS is saying now.
    let lines: [TranscriptLine]
    let speaking: (account: Account, text: String)?
    /// The voicemail the `voicemail` scene opens.
    let openVoicemail: Voicemail?

    static let tos = person("6f1c2a9e-4b7d-4e3a-9c21-0d5e8b7a6f10", "si:tos", "TOS")
    static let maya = person("a3d9e5b1-7c42-4f08-b6e2-91c0d4f7a823", "c:maya", "Maya Chen")
    static let scout = person("2e8b4f6a-1d93-47c5-a0f8-6b3e9c2d1a54", "si:scout", "Scout")
    static let arjun = person("c71f0a3d-5e28-4b96-8d4c-3a2f6e9b0c17", "c:arjun", "Arjun Mehta")
    static let atlas = person("94b2d6e8-0f3a-4c71-b5d9-e8a1c3f7b260", "si:atlas", "Atlas")
    static let lena = person("5d0e7c9b-3a61-48f2-9e05-b4c8a2d6f391", "c:lena", "Lena Fischer")
    static let sam = person("e6a4c8f2-9b15-4d37-a2c0-7f9e1b5d3a48", "c:sam", "Sam Ortiz")
    static let juniper = person("18f5b3d7-6c2e-4a90-8f4b-d2e7a9c1b065", "si:juniper", "Juniper")

    private static func person(_ uuid: String, _ id: String, _ name: String) -> Account {
        Account(uuid: uuid, id: id, kind: id.hasPrefix("si:") ? .silicon : .carbon, displayName: name, pfpURL: nil)
    }

    init(scene: DemoScene, platform: Platform) {
        let me = Self.person("0c4f8a2e-6d1b-4f93-a7e5-3b9d2c8f1a06", "c:alex", "Alex Rivera")
        self.me = me
        let now = Date()
        let calendar = Calendar.current
        let anchor = calendar.date(bySettingHour: 9, minute: 41, second: 0, of: now) ?? now
        func at(daysAgo days: Int, _ hour: Int, _ minute: Int) -> Date {
            let day = calendar.date(byAdding: .day, value: -days, to: anchor) ?? anchor
            return calendar.date(bySettingHour: hour, minute: minute, second: 0, of: day) ?? day
        }

        let deviceName = platform == .ipados ? "Alex's iPad" : "Alex's iPhone"
        device = Self.decode([
            "device_id": deviceID, "name": deviceName, "platform": platform.rawValue, "app_version": AppInfo.version,
            "created_at": RingJSON.formatDate(at(daysAgo: 40, 10, 2)), "last_seen_at": RingJSON.formatDate(now),
            "online": true, "ring_enabled": true, "current": true,
            "push": [["kind": "apns_voip", "environment": "sandbox"], ["kind": "apns", "environment": "sandbox"]],
        ])
        let myDevice = DeviceRef(deviceID: deviceID, name: deviceName, platform: platform)

        let voices = [
            Voice(id: "gleam", name: "Gleam", description: "Bright and friendly"),
            Voice(id: "vesper", name: "Vesper", description: "Calm and warm"),
            Voice(id: "meridian", name: "Meridian", description: "Clear and balanced"),
        ]
        meta = Meta(
            appID: "ring", version: AppInfo.version, apiVersion: "1", accountsURL: AppInfo.defaultAccountsURL.absoluteString,
            webURL: "https://ring.teamofsilicons.com",
            redirectURIs: RedirectURIs(native: AppInfo.nativeRedirectURI, loopback: "http://127.0.0.1/auth/callback"),
            limits: .standard, ringingSeconds: 60, voicemailMaxSeconds: 120, voices: voices, defaultVoice: "gleam",
            webpushPublicKey: nil)
        settings = Settings(
            voice: "gleam",
            representative: RepresentativeSettings(defaultContext: "", contextMode: .append, contextRequired: false),
            voicemail: VoicemailSettings(enabled: true, greetings: Greetings(busy: nil, declined: nil, missed: nil)),
            revision: 1)

        let history = Self.history(me: me, device: myDevice, at: at)
        voicemails = Self.voicemails(me: me, history: history)
        people = Self.people(from: history, me: me)

        switch scene {
        case .call:
            let call = Self.activeCall(me: me, device: myDevice, now: now)
            activeCall = call
            ringing = nil
            calls = [call] + history
            let tos = Self.tos
            lines = [
                TranscriptLine(id: "seq-2", seq: 2, speaker: tos, text: "Hey Alex, it's TOS. What can I do for you?", style: .speech, at: now.addingTimeInterval(-130)),
                TranscriptLine(id: "seq-3", seq: 3, speaker: me, text: "Hi TOS. Is my flight tomorrow still on time?", style: .speech, at: now.addingTimeInterval(-118)),
                TranscriptLine(id: "seq-4", seq: 4, speaker: tos, text: "Let me check on that.", style: .speech, at: now.addingTimeInterval(-112)),
                // seq 5 is the delegation to TOS itself, which only TOS sees.
                TranscriptLine(id: "seq-6", seq: 6, speaker: tos, text: "It is. You leave at 10:40 from gate F12, and you're in seat 14A.", style: .speech, at: now.addingTimeInterval(-104)),
                TranscriptLine(id: "seq-7", seq: 7, speaker: me, text: "Perfect. Can you book me a car to the airport?", style: .speech, at: now.addingTimeInterval(-96)),
            ]
            speaking = (tos, "Sure. I'll book one for 8:15 and send you the details")
        case .incoming:
            let ringing = Self.incomingCall(from: Self.atlas, me: me, now: now)
            self.ringing = ringing
            activeCall = nil
            calls = [ringing.call] + history
            lines = []
            speaking = nil
        case .recents, .voicemail, .dialer:
            activeCall = nil
            ringing = nil
            calls = history
            lines = []
            speaking = nil
        }
        openVoicemail = scene == .voicemail ? voicemails.first : nil
    }

    // MARK: History

    /// Ended calls with this account, newest first: answered, missed, declined, outgoing and incoming.
    private static func history(me: Account, device: DeviceRef, at: (Int, Int, Int) -> Date) -> [Call] {
        func call(_ number: Int, with other: Account, outgoing: Bool, at start: Date, talked seconds: Int? = nil,
                  outcome: InvitationState = .accepted, reason: String? = nil, voicemail: Bool = false) -> Call {
            ended(number, me: me, device: device, with: other, outgoing: outgoing, at: start,
                  talked: seconds, outcome: outcome, reason: reason, voicemail: voicemail)
        }
        return [
            call(1, with: tos, outgoing: true, at: at(0, 9, 24), talked: 372),
            call(2, with: maya, outgoing: false, at: at(0, 8, 52), outcome: .missed, voicemail: true),
            call(3, with: scout, outgoing: false, at: at(0, 8, 15), talked: 167),
            call(4, with: arjun, outgoing: true, at: at(1, 17, 30), outcome: .declined, reason: "In a meeting"),
            call(5, with: atlas, outgoing: false, at: at(1, 16, 5), outcome: .missed, voicemail: true),
            call(6, with: lena, outgoing: true, at: at(2, 19, 10), talked: 725),
            call(7, with: sam, outgoing: false, at: at(3, 13, 2), outcome: .declined, reason: "Driving", voicemail: true),
            call(8, with: juniper, outgoing: false, at: at(4, 11, 48), outcome: .busy, voicemail: true),
            call(9, with: tos, outgoing: false, at: at(5, 18, 20), talked: 220),
            call(10, with: maya, outgoing: true, at: at(6, 21, 2), talked: 1458),
            call(11, with: arjun, outgoing: false, at: at(8, 10, 30), talked: 512),
            call(12, with: scout, outgoing: true, at: at(9, 15, 45), talked: 96),
        ]
    }

    private static func ended(_ number: Int, me: Account, device: DeviceRef, with other: Account, outgoing: Bool, at start: Date,
                              talked seconds: Int?, outcome: InvitationState, reason: String?, voicemail: Bool) -> Call {
        let caller = outgoing ? me : other
        let callee = outgoing ? other : me
        let answeredAt = seconds.map { _ in start.addingTimeInterval(7) }
        let endedAt = answeredAt.map { $0.addingTimeInterval(TimeInterval(seconds ?? 0)) }
            ?? start.addingTimeInterval(outcome == .declined ? 9 : (outcome == .busy ? 1 : 60)) + (voicemail ? 40 : 0)
        func participant(_ account: Account, joined: Date) -> Participant {
            Participant(account: account, state: .left, joinedAt: joined, leftAt: endedAt, invitedBy: nil,
                        device: account.uuid == me.uuid ? device : nil, representative: nil, mediaIdentity: nil)
        }
        var participants = [participant(caller, joined: start)]
        if let answeredAt { participants.append(participant(callee, joined: answeredAt)) }
        let ringid = String(format: "ring_01kdemo%04d", number)
        let invitation = Invitation(
            invitationID: String(format: "inv_01kdemo%04d", number), ringid: ringid, from: caller, to: callee,
            state: outcome, reason: reason, createdAt: start, expiresAt: start.addingTimeInterval(60),
            answeredAt: answeredAt, silenced: false, callUUID: demoUUID(number))
        let endReason: EndReason = switch outcome {
        case .declined: .declined
        case .missed: .missed
        case .busy: .busy
        default: .completed
        }
        return Call(
            ringid: ringid, state: .ended, createdAt: start, answeredAt: answeredAt, endedAt: endedAt,
            endReason: endReason, createdBy: caller, direction: outgoing ? .outgoing : .incoming,
            participants: participants, invitations: [invitation],
            recording: RecordingSummary(status: seconds == nil ? .none : .ready, durationMS: seconds.map { $0 * 1000 }),
            voicemailID: voicemail ? voicemailID(for: ringid) : nil)
    }

    private static func voicemailID(for ringid: String) -> String {
        "vm_" + ringid.dropFirst("ring_".count)
    }

    private static func demoUUID(_ number: Int) -> String {
        String(format: "6c1b9f0e-2d4a-4c8b-9e3f-%012d", number)
    }

    // MARK: Voicemail

    private static func voicemails(me: Account, history: [Call]) -> [Voicemail] {
        let messages: [String: (seconds: Int, read: Bool, text: String)] = [
            maya.uuid: (31, false, "Hey Alex, it's Maya. I just landed! Are we still on for dinner tonight? I booked a table for seven at the place by the river. Call me back when you get this."),
            atlas.uuid: (22, false, "Hi Alex, this is Atlas. Your passport appointment moved to Friday at 2:30. I've already updated your calendar, so just call me back if that time doesn't work."),
            sam.uuid: (46, true, "Hi, Sam here. The photos from Saturday are in the shared album now. No rush, but let me know which ones you'd like printed and I'll order them this week."),
            juniper.uuid: (18, true, "Hello Alex, it's Juniper. Your grocery order is confirmed for tomorrow morning, between 8 and 10. I added the oat milk you asked for."),
        ]
        return history.compactMap { call -> Voicemail? in
            guard let id = call.voicemailID, let invitation = call.invitations.first,
                  let message = messages[invitation.from.uuid] else { return nil }
            let reason: VoicemailReason = switch invitation.state {
            case .declined: .declined
            case .busy: .busy
            default: .missed
            }
            return Voicemail(
                voicemailID: id, ringid: call.ringid, from: invitation.from, to: me, reason: reason,
                declineReason: invitation.reason, createdAt: (call.endedAt ?? call.createdAt).addingTimeInterval(-38),
                durationMS: message.seconds * 1000, audioStatus: .ready, transcript: message.text, transcriptStatus: .ready,
                read: message.read, audioURL: DemoServer.origin.appendingPathComponent("audio/\(id).m4a").absoluteString)
        }
    }

    /// The dialer's recent people, from the history: most recent first.
    private static func people(from history: [Call], me: Account) -> [RecentPerson] {
        var people: [RecentPerson] = []
        for call in history {
            let other = call.counterpart(me: me.uuid)
            if let index = people.firstIndex(where: { $0.account.uuid == other.uuid }) {
                people[index].calls += 1
            } else {
                people.append(RecentPerson(account: other, lastCallAt: call.createdAt, lastRingid: call.ringid, calls: 1))
            }
        }
        return people
    }

    // MARK: Live calls

    /// Alex called TOS a little over two minutes ago; TOS's representative answered.
    private static func activeCall(me: Account, device: DeviceRef, now: Date) -> Call {
        let start = now.addingTimeInterval(-141)
        let answered = now.addingTimeInterval(-134)
        let ringid = "ring_01kdemolive0001"
        return Call(
            ringid: ringid, state: .active, createdAt: start, answeredAt: answered, endedAt: nil, endReason: nil,
            createdBy: me, direction: .outgoing,
            participants: [
                Participant(account: me, state: .joined, joinedAt: start, leftAt: nil, invitedBy: nil, device: device,
                            representative: nil, mediaIdentity: "carbon:\(me.uuid):\(device.deviceID)"),
                Participant(account: tos, state: .joined, joinedAt: answered, leftAt: nil, invitedBy: nil, device: nil,
                            representative: RepresentativeInfo(status: .live, voice: "vesper"), mediaIdentity: "rep:\(tos.uuid)"),
            ],
            invitations: [
                Invitation(invitationID: "inv_01kdemolive0001", ringid: ringid, from: me, to: tos, state: .accepted, reason: nil,
                           createdAt: start, expiresAt: start.addingTimeInterval(60), answeredAt: answered, silenced: false,
                           callUUID: demoUUID(100)),
            ],
            recording: RecordingSummary(status: .recording, durationMS: nil), voicemailID: nil)
    }

    /// `caller` is ringing this account right now. The ring never runs out in the demo.
    private static func incomingCall(from caller: Account, me: Account, now: Date) -> (call: Call, invitation: Invitation) {
        let start = now.addingTimeInterval(-3)
        let ringid = "ring_01kdemolive0002"
        let invitation = Invitation(
            invitationID: "inv_01kdemolive0002", ringid: ringid, from: caller, to: me, state: .ringing, reason: nil,
            createdAt: start, expiresAt: now.addingTimeInterval(86_400), answeredAt: nil, silenced: false, callUUID: demoUUID(101))
        let call = Call(
            ringid: ringid, state: .ringing, createdAt: start, answeredAt: nil, endedAt: nil, endReason: nil,
            createdBy: caller, direction: .incoming,
            participants: [
                Participant(account: caller, state: .joined, joinedAt: start, leftAt: nil, invitedBy: nil, device: nil,
                            representative: RepresentativeInfo(status: .live, voice: "meridian"), mediaIdentity: "rep:\(caller.uuid)"),
            ],
            invitations: [invitation], recording: RecordingSummary(status: .none, durationMS: nil), voicemailID: nil)
        return (call, invitation)
    }

    // MARK: Helpers

    /// Types without a memberwise initializer are built from their wire form.
    private static func decode<T: Decodable>(_ object: [String: Any]) -> T {
        do {
            let data = try JSONSerialization.data(withJSONObject: object)
            return try RingJSON.decoder().decode(T.self, from: data)
        } catch {
            fatalError("Demo fixture \(T.self) does not decode: \(error)")
        }
    }
}
#endif
