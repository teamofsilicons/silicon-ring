import AVFoundation
import CallKit
import PushKit
import UIKit

/// PushKit VoIP pushes and CallKit. Every VoIP push that must be reported is reported to CallKit
/// before its completion handler runs; CallKit actions call back into AppModel.
@MainActor
final class CallKitBridge: NSObject {
    static let shared = CallKitBridge()

    private let provider: CXProvider
    private let controller = CXCallController()
    private var registry: PKPushRegistry?
    private weak var model: AppModel?

    /// Incoming calls reported to CallKit that are still ringing.
    private var ringing: Set<UUID> = []
    /// Calls this device carries that CallKit knows about.
    private var carrying: Set<UUID> = []
    /// Start actions CallKit has performed, and connects reported before that happened.
    private var startPerformed: Set<UUID> = []
    private var pendingConnected: Set<UUID> = []

    override init() {
        let configuration = CXProviderConfiguration()
        configuration.supportsVideo = false
        configuration.maximumCallGroups = 2
        configuration.maximumCallsPerCallGroup = 1
        configuration.supportedHandleTypes = [.generic]
        configuration.includesCallsInRecents = false
        configuration.ringtoneSound = "ringtone.wav"
        if let icon = UIImage(named: "CallKitIcon") {
            configuration.iconTemplateImageData = icon.pngData()
        }
        provider = CXProvider(configuration: configuration)
        super.init()
        provider.setDelegate(self, queue: nil)
    }

    /// Must run during launch so VoIP pushes that launched the app are delivered.
    func install(model: AppModel) {
        self.model = model
        model.callSystem = self
        let registry = PKPushRegistry(queue: .main)
        registry.delegate = self
        registry.desiredPushTypes = [.voIP]
        self.registry = registry
    }

    private func isKnown(_ uuid: UUID) -> Bool { ringing.contains(uuid) || carrying.contains(uuid) }

    private static func configureAudioSession() {
        let session = AVAudioSession.sharedInstance()
        try? session.setCategory(.playAndRecord, mode: .voiceChat, options: [.allowBluetoothHFP])
    }

    private func update(for ring: IncomingRing) -> CXCallUpdate {
        let update = CXCallUpdate()
        update.remoteHandle = CXHandle(type: .generic, value: ring.from.id)
        update.localizedCallerName = ring.from.label
        update.hasVideo = false
        update.supportsHolding = false
        update.supportsGrouping = false
        update.supportsUngrouping = false
        update.supportsDTMF = false
        return update
    }

    private func report(_ ring: IncomingRing, completion: @escaping () -> Void) {
        // PushKit's completion handler may be called from any thread.
        nonisolated(unsafe) let completion = completion
        let uuid = ring.callUUID
        ringing.insert(uuid)
        provider.reportNewIncomingCall(with: uuid, update: update(for: ring)) { error in
            DevLog.log("reported incoming \(uuid) error=\(error.map { String(describing: $0) } ?? "none")")
            if error != nil {
                // Do Not Disturb, a blocked caller, or a duplicate: CallKit will not show this call.
                Task { @MainActor in CallKitBridge.shared.ringing.remove(uuid) }
            }
            completion()
        }
    }

    /// Satisfies PushKit for a push that has no live call behind it: report, then end at once.
    private func reportAndEnd(_ uuid: UUID, caller: String, cause: CallEndCause, completion: @escaping () -> Void) {
        nonisolated(unsafe) let completion = completion
        let update = CXCallUpdate()
        update.remoteHandle = CXHandle(type: .generic, value: caller)
        update.localizedCallerName = caller
        update.hasVideo = false
        provider.reportNewIncomingCall(with: uuid, update: update) { _ in
            Task { @MainActor in
                CallKitBridge.shared.provider.reportCall(with: uuid, endedAt: Date(), reason: cause.reason)
            }
            completion()
        }
    }

    private func fallbackAudioActivation() {
        let session = AVAudioSession.sharedInstance()
        Self.configureAudioSession()
        try? session.setActive(true)
        CallMedia.audioSessionActivated()
        model?.audioActive = true
        model?.updateRingback()
    }
}

// MARK: - CallSystem

extension CallKitBridge: CallSystem {
    func ring(_ ring: IncomingRing) -> Bool {
        // In the foreground the app shows its own incoming screen.
        #if DEBUG
        if UIApplication.shared.applicationState == .active, !DevHooks.forceCallKit { return false }
        #else
        if UIApplication.shared.applicationState == .active { return false }
        #endif
        if !isKnown(ring.callUUID) { report(ring) {} }
        return true
    }

    func stopRinging(_ ring: IncomingRing, cause: CallEndCause) {
        guard ringing.remove(ring.callUUID) != nil else { return }
        provider.reportCall(with: ring.callUUID, endedAt: Date(), reason: cause.reason)
    }

    func ownsRing(_ ring: IncomingRing) -> Bool {
        ringing.contains(ring.callUUID)
    }

    func answer(_ ring: IncomingRing) {
        let action = CXAnswerCallAction(call: ring.callUUID)
        controller.request(CXTransaction(action: action)) { error in
            guard let error else { return }
            DevLog.log("CallKit refused the answer request: \(error)")
            Task { @MainActor in
                let bridge = CallKitBridge.shared
                bridge.ringing.remove(ring.callUUID)
                guard let model = bridge.model else { return }
                if await model.performAnswer(ring), let session = model.activeCall {
                    bridge.began(session)
                    bridge.connected(session)
                }
            }
        }
    }

    func began(_ session: CallSession) {
        let uuid = session.localUUID
        guard !isKnown(uuid) else { return }
        carrying.insert(uuid)
        let action = CXStartCallAction(call: uuid, handle: CXHandle(type: .generic, value: session.counterpart.id))
        action.isVideo = false
        controller.request(CXTransaction(action: action)) { error in
            guard error != nil else { return }
            // CallKit refused (for example during a cellular call): carry on without it.
            Task { @MainActor in
                let bridge = CallKitBridge.shared
                bridge.carrying.remove(uuid)
                bridge.fallbackAudioActivation()
            }
        }
    }

    func connected(_ session: CallSession) {
        let uuid = session.localUUID
        guard carrying.contains(uuid) else { return }
        if startPerformed.contains(uuid) {
            provider.reportOutgoingCall(with: uuid, connectedAt: Date())
        } else {
            pendingConnected.insert(uuid)
        }
    }

    func ended(_ session: CallSession, cause: CallEndCause) {
        let uuid = session.localUUID
        guard carrying.remove(uuid) != nil else { return }
        startPerformed.remove(uuid)
        pendingConnected.remove(uuid)
        provider.reportCall(with: uuid, endedAt: Date(), reason: cause.reason)
    }

    func requestHangUp(_ session: CallSession) -> Bool {
        let uuid = session.localUUID
        guard carrying.contains(uuid) else { return false }
        controller.request(CXTransaction(action: CXEndCallAction(call: uuid))) { error in
            guard error != nil else { return }
            Task { @MainActor in
                let bridge = CallKitBridge.shared
                bridge.carrying.remove(uuid)
                await bridge.model?.performHangUp(session)
            }
        }
        return true
    }

    func requestMute(_ session: CallSession, muted: Bool) -> Bool {
        let uuid = session.localUUID
        guard carrying.contains(uuid) else { return false }
        controller.request(CXTransaction(action: CXSetMutedCallAction(call: uuid, muted: muted))) { error in
            guard error != nil else { return }
            Task { @MainActor in await CallKitBridge.shared.model?.applyMute(muted) }
        }
        return true
    }
}

// MARK: - CXProviderDelegate

extension CallKitBridge: @preconcurrency CXProviderDelegate {
    func providerDidReset(_ provider: CXProvider) {
        ringing.removeAll()
        carrying.removeAll()
        startPerformed.removeAll()
        pendingConnected.removeAll()
        guard let model else { return }
        if let session = model.activeCall, session.isLive {
            Task { await model.performHangUp(session) }
        }
    }

    func provider(_ provider: CXProvider, perform action: CXAnswerCallAction) {
        let uuid = action.callUUID
        DevLog.log("CXAnswer \(uuid) known=\(model?.ring(forCallUUID: uuid) != nil)")
        guard let model, let ring = model.ring(forCallUUID: uuid) else {
            action.fail()
            ringing.remove(uuid)
            return
        }
        Self.configureAudioSession()
        Task {
            if await model.performAnswer(ring) {
                ringing.remove(uuid)
                carrying.insert(uuid)
                startPerformed.insert(uuid)
                action.fulfill()
            } else {
                // performAnswer already ended the ringing call in CallKit.
                action.fail()
            }
        }
    }

    func provider(_ provider: CXProvider, perform action: CXStartCallAction) {
        let uuid = action.callUUID
        Self.configureAudioSession()
        action.fulfill()
        startPerformed.insert(uuid)
        provider.reportOutgoingCall(with: uuid, startedConnectingAt: Date())
        if let session = model?.activeCall, session.localUUID == uuid {
            let update = CXCallUpdate()
            update.remoteHandle = CXHandle(type: .generic, value: session.counterpart.id)
            update.localizedCallerName = session.counterpart.label
            update.hasVideo = false
            update.supportsHolding = false
            update.supportsGrouping = false
            update.supportsUngrouping = false
            update.supportsDTMF = false
            provider.reportCall(with: uuid, updated: update)
        }
        if pendingConnected.remove(uuid) != nil {
            provider.reportOutgoingCall(with: uuid, connectedAt: Date())
        }
    }

    func provider(_ provider: CXProvider, perform action: CXEndCallAction) {
        DevLog.log("CXEnd \(action.callUUID)")
        let uuid = action.callUUID
        defer { action.fulfill() }
        guard let model else { return }
        if ringing.remove(uuid) != nil, let ring = model.ring(forCallUUID: uuid) {
            // Declined from the system call screen: no reason.
            model.decline(ring, reason: nil)
            return
        }
        carrying.remove(uuid)
        startPerformed.remove(uuid)
        pendingConnected.remove(uuid)
        if let session = model.activeCall, session.localUUID == uuid {
            Task { await model.performHangUp(session) }
        }
    }

    func provider(_ provider: CXProvider, perform action: CXSetMutedCallAction) {
        let muted = action.isMuted
        action.fulfill()
        Task { await model?.applyMute(muted) }
    }

    func provider(_ provider: CXProvider, perform action: CXSetHeldCallAction) {
        action.fail()
    }

    func provider(_ provider: CXProvider, timedOutPerforming action: CXAction) {
        DevLog.log("CallKit timed out performing \(type(of: action))")
        action.fail()
    }

    func provider(_ provider: CXProvider, didActivate audioSession: AVAudioSession) {
        DevLog.log("audio session activated")
        CallMedia.audioSessionActivated()
        model?.audioActive = true
        model?.updateRingback()
    }

    func provider(_ provider: CXProvider, didDeactivate audioSession: AVAudioSession) {
        DevLog.log("audio session deactivated")
        CallMedia.audioSessionDeactivated()
        model?.audioActive = false
        Tones.shared.stopRingback()
    }
}

// MARK: - PKPushRegistryDelegate

extension CallKitBridge: @preconcurrency PKPushRegistryDelegate {
    func pushRegistry(_ registry: PKPushRegistry, didUpdate pushCredentials: PKPushCredentials, for type: PKPushType) {
        guard type == .voIP else { return }
        model?.voipToken = pushCredentials.token.map { String(format: "%02x", $0) }.joined()
    }

    func pushRegistry(_ registry: PKPushRegistry, didInvalidatePushTokenFor type: PKPushType) {
        guard type == .voIP else { return }
        model?.voipToken = nil
    }

    /// iOS 17 – 26.3: every VoIP push must be reported.
    func pushRegistry(_ registry: PKPushRegistry, didReceiveIncomingPushWith payload: PKPushPayload, for type: PKPushType, completion: @escaping () -> Void) {
        guard type == .voIP else { completion(); return }
        handleVoIPPush(payload.dictionaryPayload, mustReport: true, completion: completion)
    }

    /// iOS 26.4+: the system says whether this push must be reported (not in the foreground,
    /// not with a call already active, not for a very late push).
    @available(iOS 26.4, *)
    func pushRegistry(_ registry: PKPushRegistry, didReceiveIncomingVoIPPushWith payload: PKPushPayload, metadata: PKVoIPPushMetadata, withCompletionHandler completion: @escaping () -> Void) {
        DevLog.log("voip push type=\(payload.dictionaryPayload["type"] ?? "?") mustReport=\(metadata.mustReport)")
        handleVoIPPush(payload.dictionaryPayload, mustReport: metadata.mustReport, completion: completion)
    }

    private func handleVoIPPush(_ dictionary: [AnyHashable: Any], mustReport: Bool, completion: @escaping () -> Void) {
        guard let model, model.isSignedIn else {
            if mustReport {
                reportAndEnd(PushPayload.callUUID(in: dictionary) ?? UUID(), caller: "Ring", cause: .failed, completion: completion)
            } else {
                completion()
            }
            return
        }
        model.start()

        switch PushPayload.parse(dictionary) {
        case .callIncoming(let ringid, let invitationID, let callUUID, let from, let participants, let expiresAt):
            let ring = IncomingRing(ringid: ringid, invitationID: invitationID, callUUID: callUUID, from: from,
                                    participants: participants, expiresAt: expiresAt)
            let expired = expiresAt.map { $0 < Date() } ?? false
            if isKnown(callUUID) {
                if mustReport {
                    // A duplicate that PushKit still wants reported; CallKit rejects the UUID, which is fine.
                    nonisolated(unsafe) let completion = completion
                    provider.reportNewIncomingCall(with: callUUID, update: update(for: ring)) { _ in completion() }
                } else {
                    completion()
                }
                return
            }
            if !mustReport {
                if !expired { model.present(ring) }
                completion()
                model.resume()
                return
            }
            if expired || model.currentDevice?.ringEnabled == false {
                reportAndEnd(callUUID, caller: from.label, cause: .unanswered, completion: completion)
                return
            }
            model.adoptSystemRing(ring)
            model.ringMovedToSystem(invitationID)
            report(ring, completion: completion)
            model.resume()

        case .callCancelled(_, let invitationID, let callUUID, let reason):
            let cause = AppModel.cause(for: reason)
            let knownToCallKit = isKnown(callUUID)
            if let ring = model.rings[invitationID] {
                model.stopRing(ring, cause: cause)
            } else if ringing.remove(callUUID) != nil {
                provider.reportCall(with: callUUID, endedAt: Date(), reason: cause.reason)
            }
            if mustReport && !knownToCallKit {
                reportAndEnd(callUUID, caller: "Ring", cause: cause, completion: completion)
            } else {
                completion()
            }

        default:
            // VoIP pushes only ever carry incoming and cancelled calls.
            if mustReport {
                reportAndEnd(PushPayload.callUUID(in: dictionary) ?? UUID(), caller: "Ring", cause: .failed, completion: completion)
            } else {
                completion()
            }
        }
    }
}

extension CallEndCause {
    var reason: CXCallEndedReason {
        switch self {
        case .remoteEnded: .remoteEnded
        case .answeredElsewhere: .answeredElsewhere
        case .declinedElsewhere: .declinedElsewhere
        case .unanswered: .unanswered
        case .failed: .failed
        }
    }
}
