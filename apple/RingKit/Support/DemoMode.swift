#if DEBUG && os(iOS)
import SwiftUI
import UIKit

/// The screen `-RingDemo <scene>` opens on, for App Store screenshots.
enum DemoScene: String, CaseIterable, Sendable {
    /// Recents with people and silicons: answered, missed, declined, incoming and outgoing.
    case recents
    /// A call with TOS's representative, mid-conversation, with TOS speaking.
    case call
    /// Atlas calling, on the in-app incoming screen.
    case incoming
    /// A voicemail and its transcript.
    case voicemail
    /// The New call dialer.
    case dialer
}

/// Debug-only demo mode: made-up people, silicons and calls, with no server and no sign-in, so
/// each screen renders the same way every time. This launch talks to an origin that does not exist
/// and that `DemoServer` answers inside the app, with a session made for it. That session replaces
/// any signed-in one, and the next ordinary launch drops it because it belongs to another server.
@MainActor
enum DemoMode {
    static func start(_ scene: DemoScene) {
        let fixtures = DemoFixtures(scene: scene, platform: AppInfo.platform)
        DemoServer.fixtures = fixtures
        URLProtocol.registerClass(DemoServer.self)

        // The server override for this launch only: the argument domain is never written to disk.
        let defaults = UserDefaults.standard
        var arguments = defaults.volatileDomain(forName: UserDefaults.argumentDomain)
        arguments["ring.server.override"] = DemoServer.origin.absoluteString
        defaults.setVolatileDomain(arguments, forName: UserDefaults.argumentDomain)
        SessionStore.save(StoredSession(
            token: "demo", server: DemoServer.origin, accountUUID: fixtures.me.uuid, accountID: fixtures.me.id,
            deviceID: fixtures.deviceID, expiresAt: Date().addingTimeInterval(86_400)))

        // What the first loads would bring, in place before the first frame.
        let model = AppModel.shared
        model.me = fixtures.me
        model.currentDevice = fixtures.device
        model.devices = [fixtures.device]
        model.meta = fixtures.meta
        model.voices = fixtures.meta.voices
        model.settings = fixtures.settings
        model.recents = fixtures.calls
        model.recentsLoaded = true
        model.voicemails = fixtures.voicemails
        model.voicemailsLoaded = true
        for call in fixtures.calls where call.state.isLive { model.liveCalls[call.ringid] = call }
        model.requestedSection = scene == .voicemail ? .voicemail : .recents

        if let call = fixtures.activeCall {
            let session = CallSession(
                ringid: call.ringid, localUUID: UUID(), role: .outgoing, myUUID: fixtures.me.uuid,
                call: call, invitationID: call.invitations.first?.invitationID)
            session.mediaState = .connected
            session.lines = fixtures.lines
            if let speaking = fixtures.speaking {
                session.addPartial(speaker: speaking.account, segmentID: "demo-speaking", text: speaking.text)
                if let identity = call.participant(uuid: speaking.account.uuid)?.mediaIdentity {
                    session.speakingIdentities = [identity]
                }
            }
            model.activeCall = session
        }
        if let ringing = fixtures.ringing {
            let ring = IncomingRing(call: ringing.call, invitation: ringing.invitation, me: fixtures.me.uuid)
            model.rings[ring.invitationID] = ring
            model.inAppRingIDs = [ring.invitationID]
        }
        if scene == .dialer || fixtures.openVoicemail != nil {
            Task { await openWhenReady(scene, fixtures: fixtures) }
        }
    }

    /// The dialer and a voicemail open from a tap: wait for the home screen, then open them on top.
    private static func openWhenReady(_ scene: DemoScene, fixtures: DemoFixtures) async {
        var root: UIViewController?
        for _ in 0..<300 {
            root = UIApplication.shared.connectedScenes.compactMap { $0 as? UIWindowScene }
                .flatMap(\.windows).first(where: \.isKeyWindow)?.rootViewController
            if root != nil, UIApplication.shared.applicationState == .active { break }
            try? await Task.sleep(for: .milliseconds(100))
        }
        guard let root else { return }
        try? await Task.sleep(for: .milliseconds(500))
        let model = AppModel.shared

        if scene == .dialer {
            let dialer = UIHostingController(rootView: DialerView(mode: .call).environment(model).tint(Palette.accent))
            root.present(dialer, animated: false)
            // The dialer focuses its field as it appears; show the recent people rather than a keyboard.
            try? await Task.sleep(for: .seconds(1.5))
            dialer.view.endEditing(true)
        } else if let voicemail = fixtures.openVoicemail, let navigation = largestNavigation(in: root) {
            let detail = UIHostingController(rootView: VoicemailDetailView(voicemail: voicemail).environment(model).tint(Palette.accent))
            navigation.setNavigationBarHidden(false, animated: false)
            navigation.pushViewController(detail, animated: false)
        }
    }

    /// The navigation stack showing the current section: on iPad, the split view's detail column.
    private static func largestNavigation(in controller: UIViewController) -> UINavigationController? {
        var found: [UINavigationController] = []
        func visit(_ controller: UIViewController) {
            if let navigation = controller as? UINavigationController, navigation.viewIfLoaded?.window != nil {
                found.append(navigation)
            }
            controller.children.forEach(visit)
        }
        visit(controller)
        return found.max { $0.view.bounds.width * $0.view.bounds.height < $1.view.bounds.width * $1.view.bounds.height }
    }
}
#endif
