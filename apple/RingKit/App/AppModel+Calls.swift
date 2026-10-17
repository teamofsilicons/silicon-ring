import AVFoundation
import Foundation
#if os(iOS)
import UIKit
#endif

/// Call flows. User actions route through the platform call system when it owns the call
/// (CallKit); the `perform…` methods do the actual work and are what the system calls back.
extension AppModel {
    // MARK: Outgoing

    func placeCall(to ref: String) {
        let target = ref.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !target.isEmpty, let myUUID else { return }
        if let current = activeCall, current.isLive {
            show("You're already on a call.", style: .error)
            return
        }
        Task {
            do {
                let created = try await api.createCall(to: target)
                let session = CallSession(
                    ringid: created.call.ringid, localUUID: UUID(), role: .outgoing, myUUID: myUUID,
                    call: created.call, invitationID: created.invitation.invitationID)
                activeCall = session
                remember(created.call)
                callSystem?.began(session)
                updateRingback()
                if session.phase == .ended {
                    finish(session, cause: .unanswered)
                    return
                }
                if let grant = created.media {
                    await join(session, grant: grant)
                }
            } catch {
                handle(error)
            }
        }
    }

    // MARK: Incoming

    func present(_ ring: IncomingRing) {
        if var known = rings[ring.invitationID] {
            if known.call == nil, let call = ring.call {
                known.call = call
                known.alreadyOnCall = ring.alreadyOnCall
                rings[ring.invitationID] = known
            }
            return
        }
        if activeCall?.ringid == ring.ringid { return }
        if currentDevice?.ringEnabled == false { return }
        rings[ring.invitationID] = ring
        scheduleExpiry(for: ring)
        #if DEBUG
        DevHooks.ringPresented(ring, model: self)
        #endif
        let handledBySystem = callSystem?.ring(ring) ?? false
        if !handledBySystem {
            inAppRingIDs.append(ring.invitationID)
            if activeCall?.isLive != true { Tones.shared.startRingtone() }
        }
    }

    /// A ring the system UI already reported (an iOS VoIP push); remember it without ringing again.
    func adoptSystemRing(_ ring: IncomingRing) {
        guard rings[ring.invitationID] == nil else { return }
        rings[ring.invitationID] = ring
        scheduleExpiry(for: ring)
        #if DEBUG
        DevHooks.ringPresented(ring, model: self)
        #endif
    }

    /// The system call UI took over a ring the app was showing itself (a VoIP push that had to be
    /// reported while the in-app incoming screen was up), so only one of them rings.
    func ringMovedToSystem(_ invitationID: String) {
        guard inAppRingIDs.contains(invitationID) else { return }
        inAppRingIDs.removeAll { $0 == invitationID }
        if inAppRingIDs.isEmpty { Tones.shared.stopRingtone() }
    }

    func stopRing(_ ring: IncomingRing, cause: CallEndCause) {
        rings[ring.invitationID] = nil
        cancelExpiry(ring.invitationID)
        inAppRingIDs.removeAll { $0 == ring.invitationID }
        if inAppRingIDs.isEmpty { Tones.shared.stopRingtone() }
        callSystem?.stopRinging(ring, cause: cause)
    }

    func ring(forCallUUID uuid: UUID) -> IncomingRing? {
        rings.values.first { $0.callUUID == uuid }
    }

    /// Moves an in-app ring to the system UI (the app went to the background while ringing).
    func handOffInAppRingsToSystem() {
        let ids = inAppRingIDs
        guard !ids.isEmpty else { return }
        inAppRingIDs = []
        Tones.shared.stopRingtone()
        for id in ids {
            guard let ring = rings[id] else { continue }
            if callSystem?.ring(ring) != true { inAppRingIDs.append(id) }
        }
    }

    func answer(_ ring: IncomingRing) {
        DevLog.log("answer requested \(ring.ringid)")
        if callSystem?.answer(ring) == true { return }
        Task {
            if await performAnswer(ring), let session = activeCall {
                callSystem?.began(session)
                callSystem?.connected(session)
            }
        }
    }

    /// Accepts on the server and joins the room. Returns whether this device now carries the call.
    @discardableResult
    func performAnswer(_ ring: IncomingRing) async -> Bool {
        DevLog.log("answer \(ring.ringid) inv=\(ring.invitationID)")
        guard let myUUID else { return false }
        rings[ring.invitationID] = nil
        cancelExpiry(ring.invitationID)
        inAppRingIDs.removeAll { $0 == ring.invitationID }
        if inAppRingIDs.isEmpty { Tones.shared.stopRingtone() }
        if let current = activeCall, current.isLive, current.ringid != ring.ringid {
            await performHangUp(current)
        }
        do {
            let accepted = try await api.accept(ring.ringid, invitationID: ring.invitationID)
            guard let grant = accepted.media else {
                show("Ring couldn't connect this device to the call.", style: .error)
                callSystem?.stopRinging(ring, cause: .failed)
                return false
            }
            let session = CallSession(
                ringid: ring.ringid, localUUID: ring.callUUID, role: .incoming, myUUID: myUUID,
                call: accepted.call, invitationID: ring.invitationID)
            activeCall = session
            remember(accepted.call)
            Task { await join(session, grant: grant) }
            return true
        } catch {
            if let error = error as? RingError, error.code == "invitation_not_ringing" || error.code == "invitation_expired" {
                show("That call is no longer ringing.", style: .error)
            } else {
                handle(error)
            }
            callSystem?.stopRinging(ring, cause: .failed)
            return false
        }
    }

    func decline(_ ring: IncomingRing, reason: String?) {
        stopRing(ring, cause: .remoteEnded)
        Task {
            do {
                try await api.decline(ring.ringid, invitationID: ring.invitationID, reason: reason)
            } catch {
                handle(error)
            }
        }
    }

    /// Stops ringing on every device of this account without declining.
    func silence(_ ring: IncomingRing) {
        stopRing(ring, cause: .declinedElsewhere)
        Task {
            do {
                try await api.silence(ring.ringid, invitationID: ring.invitationID)
            } catch {
                handle(error, silently: true)
            }
        }
    }

    // MARK: During a call

    func hangUp() {
        guard let session = activeCall else { return }
        guard session.isLive else {
            dismissCall(session)
            return
        }
        if callSystem?.requestHangUp(session) == true { return }
        Task { await performHangUp(session) }
    }

    func performHangUp(_ session: CallSession, cause: CallEndCause = .remoteEnded) async {
        DevLog.log("hang up \(session.ringid) cause=\(cause)")
        guard session.isLive else { return }
        // A no-op when the system UI itself performed the hang-up.
        callSystem?.ended(session, cause: cause)
        session.phase = .ended
        session.endedAt = Date()
        if session.endSummary == nil {
            session.endSummary = session.call.state == .voicemail && session.role == .outgoing ? "Message sent" : "Call ended"
        }
        Tones.shared.stopRingback()
        Tones.shared.play(.ended)
        await media.disconnect()
        scheduleDismissal(session)
        do {
            let call = try await api.leave(session.ringid)
            session.call = call
            remember(call)
        } catch {
            handle(error, silently: true)
        }
        scheduleRecentsRefresh()
    }

    func toggleMute() {
        guard let session = activeCall, session.isLive else { return }
        let muted = !session.muted
        if callSystem?.requestMute(session, muted: muted) == true { return }
        Task { await applyMute(muted) }
    }

    func applyMute(_ muted: Bool) async {
        guard let session = activeCall else { return }
        session.muted = muted
        await media.setMuted(muted)
    }

    func invite(_ ref: String) async -> Bool {
        guard let session = activeCall, session.isLive else { return false }
        do {
            let invited = try await api.invite(session.ringid, to: ref)
            session.apply(invited.call)
            show("Calling \(invited.invitation.to.label)…")
            return true
        } catch {
            handle(error)
            return false
        }
    }

    // MARK: Handoff

    /// A call this account is on, carried by another of its devices.
    var callOnOtherDevice: (call: Call, device: DeviceRef)? {
        guard let myUUID, let myDeviceID else { return nil }
        for call in liveCalls.values where call.state == .active && activeCall?.ringid != call.ringid {
            if let me = call.participant(uuid: myUUID), me.state == .joined, let device = me.device, device.deviceID != myDeviceID {
                return (call, device)
            }
        }
        return nil
    }

    func switchHere(_ call: Call) {
        guard let myUUID else { return }
        Task {
            do {
                let moved = try await api.media(call.ringid)
                guard let grant = moved.media else { return }
                let session = CallSession(
                    ringid: call.ringid, localUUID: UUID(), role: .handoff, myUUID: myUUID, call: moved.call, invitationID: nil)
                activeCall = session
                remember(moved.call)
                movedNotice = nil
                callSystem?.began(session)
                callSystem?.connected(session)
                await join(session, grant: grant)
            } catch {
                handle(error)
            }
        }
    }

    // MARK: Media

    func join(_ session: CallSession, grant: MediaGrant) async {
        #if os(iOS)
        // CallKit enables LiveKit's input only once access is decided; ask while the prompt can show.
        if AVAudioApplication.shared.recordPermission == .undetermined, UIApplication.shared.applicationState == .active {
            _ = await AVAudioApplication.requestRecordPermission()
        }
        #endif
        if AVAudioApplication.shared.recordPermission == .denied {
            show("Ring can't use the microphone, so the call can't hear you. Allow it in Settings.", style: .error)
        }
        DevLog.log("join \(session.ringid) audioActive=\(audioActive) mic=\(AVAudioApplication.shared.recordPermission.rawValue)")
        do {
            try await media.connect(grant, muted: session.muted)
            DevLog.log("joined \(session.ringid)")
            guard activeCall === session, session.isLive else {
                await media.disconnect()
                return
            }
            await loadTranscript(session)
        } catch {
            DevLog.log("join failed \(session.ringid): \(error)")
            guard activeCall === session, session.isLive else { return }
            show("Ring couldn't connect the call audio.", style: .error)
            await performHangUp(session, cause: .failed)
        }
    }

    private func loadTranscript(_ session: CallSession) async {
        guard let entries = try? await api.fullTranscript(session.ringid) else { return }
        for entry in entries { session.add(entry) }
    }

    /// LiveKit gave up on the room; rejoin within the server's grace period if the call is still on.
    func mediaLost() {
        DevLog.log("media lost \(activeCall?.ringid ?? "-")")
        guard let session = activeCall, session.isLive, session.phase != .ended else { return }
        Task {
            // The room also drops when the call ends or moves to another device (the server removes
            // this participant); asking for media then would end the call or pull it back here.
            if let call = try? await api.call(session.ringid) {
                callChanged(call)
                guard activeCall === session, session.isLive else { return }
                if let mine = call.participant(uuid: session.myUUID), mine.state == .joined,
                   let device = mine.device, device.deviceID != myDeviceID {
                    handoffOccurred(ringid: call.ringid, deviceID: device.deviceID, device: device)
                    return
                }
            }
            do {
                let rejoined = try await api.media(session.ringid)
                session.apply(rejoined.call)
                if let grant = rejoined.media, session.isLive {
                    await join(session, grant: grant)
                }
            } catch {
                guard activeCall === session, session.isLive else { return }
                await performHangUp(session, cause: .failed)
            }
        }
    }

    // MARK: State changes from the server

    func callChanged(_ call: Call) {
        remember(call)
        #if DEBUG
        DevHooks.callChanged(model: self)
        #endif

        // Ringing that this device shows but the server no longer considers ringing.
        for ring in rings.values where ring.ringid == call.ringid {
            if let invitation = call.invitations.first(where: { $0.invitationID == ring.invitationID }), invitation.state != .ringing || invitation.silenced {
                stopRing(ring, cause: Self.cause(for: invitation.state))
            } else if call.state == .ended {
                stopRing(ring, cause: .remoteEnded)
            }
        }

        guard let session = activeCall, session.ringid == call.ringid, session.isLive else { return }
        let before = session.phase
        session.apply(call)
        if let myUUID, let mine = call.participant(uuid: myUUID), session.phase != .ended {
            if let device = mine.device, mine.state == .joined, device.deviceID != myDeviceID,
               Date().timeIntervalSince(session.startedAt) > 3 {
                // The audio moved to another of this account's devices (in case call.handoff was missed).
                handoffOccurred(ringid: call.ringid, deviceID: device.deviceID, device: device)
                return
            }
            if mine.state == .left {
                // This account left: another device hung up, or the server removed it.
                session.phase = .ended
                session.endedAt = Date()
                session.endSummary = "Call ended"
            }
        }
        if before != .active && session.phase == .active {
            Tones.shared.play(.connected)
            callSystem?.connected(session)
        }
        updateRingback()
        if session.phase == .ended {
            finish(session, cause: .remoteEnded)
        }
    }

    /// The server moved this account's audio to another device ("Moved to Pixel 8").
    func handoffOccurred(ringid: String, deviceID: String, device: DeviceRef? = nil) {
        guard let session = activeCall, session.ringid == ringid, deviceID != myDeviceID, session.isLive else { return }
        let known = session.call.participant(uuid: session.myUUID)?.device
        let target = device ?? (known?.deviceID == deviceID ? known : nil) ?? DeviceRef(deviceID: deviceID)
        let notice = "Moved to \(deviceLabel(target))"
        session.phase = .ended
        session.endedAt = Date()
        session.endSummary = notice
        movedNotice = notice
        Tones.shared.stopRingback()
        Task { await media.disconnect() }
        callSystem?.ended(session, cause: .answeredElsewhere)
        scheduleDismissal(session, after: 1.2)
        Task {
            try? await Task.sleep(for: .seconds(8))
            if movedNotice == notice { movedNotice = nil }
        }
    }

    /// What to call one of this account's devices: its name ("Pixel 8"), from the reference or the
    /// device list, and only failing that a platform word.
    func deviceLabel(_ device: DeviceRef) -> String {
        if let name = device.name?.trimmingCharacters(in: .whitespacesAndNewlines), !name.isEmpty { return name }
        let listed = devices.first { $0.deviceID == device.deviceID }
        if let name = listed?.name?.trimmingCharacters(in: .whitespacesAndNewlines), !name.isEmpty { return name }
        return (device.platform ?? listed?.platform)?.label ?? "another device"
    }

    // MARK: Recents

    /// Records the latest copy of a call: in the live calls while it is on, and in Recents always,
    /// so a call shows there from its first event rather than only once it has ended.
    func remember(_ call: Call) {
        liveCalls[call.ringid] = call.state.isLive ? call : nil
        if let index = recents.firstIndex(where: { $0.ringid == call.ringid }) {
            recents[index] = call
        } else {
            // Newest first; a call older than everything loaded waits for its page.
            let index = recents.firstIndex { $0.createdAt < call.createdAt } ?? recents.endIndex
            if index < recents.endIndex || recentsCursor == nil { recents.insert(call, at: index) }
        }
    }

    /// Tears down a call that ended remotely.
    func finish(_ session: CallSession, cause: CallEndCause) {
        DevLog.log("finish \(session.ringid) cause=\(cause)")
        Tones.shared.stopRingback()
        Tones.shared.play(.ended)
        Task { await media.disconnect() }
        callSystem?.ended(session, cause: cause)
        scheduleDismissal(session)
        scheduleRecentsRefresh()
    }

    func dismissCall(_ session: CallSession) {
        if activeCall === session { activeCall = nil }
    }

    private func scheduleDismissal(_ session: CallSession, after seconds: Double = 2.4) {
        Task {
            try? await Task.sleep(for: .seconds(seconds))
            if activeCall === session, !session.isLive { activeCall = nil }
        }
    }

    /// Ringback plays on the caller's device until the call is answered or goes to voicemail.
    func updateRingback() {
        guard let session = activeCall, session.role == .outgoing,
              session.phase == .calling || session.phase == .ringing, audioActive else {
            Tones.shared.stopRingback()
            return
        }
        Tones.shared.startRingback()
    }

    /// Ends every call and ring on this device without telling the server (sign-out, revoked session).
    func endEverythingLocally() async {
        for ring in rings.values { stopRing(ring, cause: .failed) }
        if let session = activeCall, session.isLive {
            session.phase = .ended
            callSystem?.ended(session, cause: .failed)
        }
        activeCall = nil
        Tones.shared.stopAll()
        await media.disconnect()
    }

    static func cause(for state: InvitationState) -> CallEndCause {
        switch state {
        case .accepted: .answeredElsewhere
        case .declined: .declinedElsewhere
        case .missed: .unanswered
        case .cancelled, .busy, .ringing, .unknown: .remoteEnded
        }
    }

    static func cause(for reason: RingingStoppedReason) -> CallEndCause {
        switch reason {
        case .acceptedElsewhere: .answeredElsewhere
        case .declined, .silenced: .declinedElsewhere
        case .missed: .unanswered
        case .cancelled, .unknown: .remoteEnded
        }
    }
}
