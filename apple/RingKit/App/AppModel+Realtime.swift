import Foundation

extension AppModel {
    func handle(_ event: RealtimeEvent) {
        switch event {
        case .ready(let account, _):
            me = account
            Task { await reconcile() }

        case .callIncoming(let call, let invitation):
            callChanged(call)
            guard invitation.to.uuid == myUUID, invitation.state == .ringing, !invitation.silenced else { return }
            present(IncomingRing(call: call, invitation: invitation, me: myUUID))

        case .callUpdated(let call), .callEnded(let call):
            callChanged(call)

        case .ringingStopped(let ringid, let invitationID, let reason):
            guard let ring = rings[invitationID] else { return }
            if activeCall?.ringid == ringid, activeCall?.role == .incoming {
                // This device is the one that answered.
                rings[invitationID] = nil
                return
            }
            stopRing(ring, cause: Self.cause(for: reason))

        case .handoff(let ringid, let deviceID):
            handoffOccurred(ringid: ringid, deviceID: deviceID)

        case .transcriptEntry(let ringid, let entry):
            guard let session = activeCall, session.ringid == ringid else { return }
            session.add(entry)

        case .transcriptPartial(let ringid, let speaker, let segmentID, let text):
            guard let session = activeCall, session.ringid == ringid, session.isLive else { return }
            session.addPartial(speaker: speaker, segmentID: segmentID, text: text)

        case .voicemailReceived(let voicemail), .voicemailUpdated(let voicemail):
            upsert(voicemail)

        case .deviceUpdated(var device):
            // The event is broadcast to every device of the account, so `current` is decided here.
            device.current = device.deviceID == myDeviceID
            if let index = devices.firstIndex(where: { $0.deviceID == device.deviceID }) {
                devices[index] = device
            } else if !devices.isEmpty {
                devices.append(device)
            }
            if device.deviceID == myDeviceID { currentDevice = device }

        case .sessionRevoked:
            sessionInvalidated()

        case .pong, .other:
            break
        }
    }

    /// Brings local call state in line with the server after any gap in the realtime stream.
    func reconcile() async {
        guard isSignedIn, let myUUID else { return }
        let page: Page<Call>
        do {
            page = try await api.calls(states: [.ringing, .active, .voicemail], limit: 50)
        } catch {
            handle(error, silently: true)
            return
        }
        let live = Dictionary(page.items.map { ($0.ringid, $0) }, uniquingKeysWith: { first, _ in first })
        liveCalls = live

        for call in page.items {
            callChanged(call)
            if let invitation = call.ringingInvitation(for: myUUID), !invitation.silenced, activeCall?.ringid != call.ringid {
                present(IncomingRing(call: call, invitation: invitation, me: myUUID))
            }
        }
        for ring in rings.values where live[ring.ringid]?.ringingInvitation(for: myUUID)?.invitationID != ring.invitationID {
            if activeCall?.ringid == ring.ringid { continue }
            stopRing(ring, cause: .remoteEnded)
        }

        // The server says this device carries a live call the app has no session for: the app was
        // restarted or crashed mid-call. Rejoin it rather than stranding the call on a dead device.
        if activeCall == nil, let myDeviceID {
            for call in page.items where call.state == .active {
                if let me = call.participant(uuid: myUUID), me.state == .joined, me.device?.deviceID == myDeviceID {
                    switchHere(call)
                    break
                }
            }
        }

        if let session = activeCall, session.isLive, live[session.ringid] == nil {
            if let call = try? await api.call(session.ringid) {
                callChanged(call)
            }
        }
    }

    /// Opens what an alert notification was about: a missed call or a voicemail.
    func openNotification(type: String?) {
        switch type {
        case "voicemail.received":
            requestedSection = .voicemail
            Task { await refreshVoicemails() }
        case "call.missed":
            requestedSection = .recents
            Task { await refreshRecents() }
        default:
            break
        }
        resume()
    }

    /// Handles a notification action or a launch from an alert push.
    func open(ringid: String, invitationID: String?, answer: Bool) {
        Task {
            guard isSignedIn, let myUUID else { return }
            guard let call = try? await api.call(ringid) else { return }
            callChanged(call)
            guard answer, let invitation = call.invitations.first(where: { ($0.invitationID == invitationID || invitationID == nil) && $0.to.uuid == myUUID }),
                  invitation.state == .ringing else { return }
            let ring = IncomingRing(call: call, invitation: invitation, me: myUUID)
            rings[ring.invitationID] = ring
            if await performAnswer(ring), let session = activeCall {
                callSystem?.began(session)
                callSystem?.connected(session)
            }
        }
    }
}
