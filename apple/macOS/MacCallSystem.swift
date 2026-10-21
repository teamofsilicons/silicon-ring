import AppKit
import SwiftUI
import UserNotifications

/// The Mac's call integration: a FaceTime-style call banner (CallBanner.swift) and a looping
/// ringtone. macOS has no CallKit, so audio is managed directly. The notification category serves
/// the server's alert push, which rings a Mac where Ring is not running.
@MainActor
final class MacCallSystem: CallSystem {
    static let shared = MacCallSystem()

    static let categoryID = "RING_CALL"
    static let answerAction = "ANSWER"
    static let declineAction = "DECLINE"

    static var ringCategory: UNNotificationCategory {
        UNNotificationCategory(
            identifier: categoryID,
            actions: [
                UNNotificationAction(identifier: answerAction, title: "Answer", options: [.foreground]),
                UNNotificationAction(identifier: declineAction, title: "Decline", options: [.destructive]),
            ],
            intentIdentifiers: [],
            options: [])
    }

    private weak var model: AppModel?
    /// Banners in the order they arrived, newest at the bottom of the stack.
    private var banners: [(invitationID: String, panel: CallBannerPanel)] = []
    private var observers: [NSObjectProtocol] = []

    /// A banner still offers a ring nobody has answered or declined.
    var keepsRinging: Bool {
        banners.contains { model?.rings[$0.invitationID] != nil }
    }

    func install(model: AppModel) {
        self.model = model
        model.callSystem = self
        model.onSignIn = { DesktopPreferences.enableLoginItemByDefault() }
        let center = NotificationCenter.default
        observers.append(center.addObserver(forName: NSApplication.didChangeScreenParametersNotification, object: nil, queue: .main) { _ in
            MainActor.assumeIsolated { MacCallSystem.shared.layout(animated: false) }
        })
        // A ring opened in Ring's window goes back to a banner when the window stops being where
        // the person is looking, so it can always be answered.
        observers.append(center.addObserver(forName: NSApplication.didResignActiveNotification, object: nil, queue: .main) { _ in
            MainActor.assumeIsolated { MacCallSystem.shared.returnInAppRings() }
        })
        for name in [NSWindow.willCloseNotification, NSWindow.didMiniaturizeNotification] {
            observers.append(center.addObserver(forName: name, object: nil, queue: .main) { notification in
                let closing = (notification.object as? NSWindow).map(ObjectIdentifier.init)
                MainActor.assumeIsolated {
                    guard let closing, let main = MainWindow.window, ObjectIdentifier(main) == closing else { return }
                    MacCallSystem.shared.returnInAppRings()
                }
            })
        }
        // A banner does not show over the lock screen: while the Mac is locked a call also posts a
        // time-sensitive notification, withdrawn again once it is unlocked.
        let distributed = DistributedNotificationCenter.default()
        observers.append(distributed.addObserver(forName: Notification.Name("com.apple.screenIsLocked"), object: nil, queue: .main) { _ in
            MainActor.assumeIsolated { MacCallSystem.shared.screenLocked() }
        })
        observers.append(distributed.addObserver(forName: Notification.Name("com.apple.screenIsUnlocked"), object: nil, queue: .main) { _ in
            MainActor.assumeIsolated { MacCallSystem.shared.screenUnlocked() }
        })
    }

    func ring(_ ring: IncomingRing) -> Bool {
        guard let model else { return true }
        guard !banners.contains(where: { $0.invitationID == ring.invitationID }) else { return true }
        let invitationID = ring.invitationID
        let panel = CallBannerPanel(ring: ring, model: model) {
            MainActor.assumeIsolated { MacCallSystem.shared.showInApp(invitationID) }
        }
        let screen = stackScreen
        banners.append((invitationID, panel))
        if let screen {
            panel.present(at: CallBannerPanel.frame(size: panel.fittedSize, on: screen, above: stackOffset(before: banners.count - 1)))
        }
        if model.activeCall?.isLive == true {
            Tones.shared.play(.connected)
        } else {
            updateRingtone()
        }
        if Self.screenIsLocked { postNotification(for: ring) }
        NSAccessibility.post(
            element: NSApp as Any, notification: .announcementRequested,
            userInfo: [.announcement: "Incoming call from \(ring.from.label)", .priority: NSAccessibilityPriorityLevel.high.rawValue])
        return true
    }

    func stopRinging(_ ring: IncomingRing, cause: CallEndCause) {
        dismiss(ring.invitationID)
    }

    /// Without an APNs token (Debug builds, or a Mac without a push profile) the server's
    /// "Missed call" alert never reaches this Mac, so it posts its own.
    func missedCall(from caller: Account, ringid: String) {
        guard let model, model.apnsToken == nil, model.currentDevice?.ringEnabled != false else { return }
        let content = UNMutableNotificationContent()
        content.title = "Missed call"
        content.body = caller.label
        content.sound = .default
        content.threadIdentifier = ringid
        content.userInfo = ["type": "call.missed", "ringid": ringid]
        UNUserNotificationCenter.current().add(UNNotificationRequest(identifier: "missed-\(ringid)", content: content, trigger: nil))
    }

    func answer(_ ring: IncomingRing) -> Bool { false }

    func began(_ session: CallSession) {
        if let invitationID = session.invitationID { dismiss(invitationID) }
        updateRingtone()
        if session.role != .outgoing {
            session.minimized = false
            MainWindow.show()
        }
    }

    func connected(_ session: CallSession) {}

    /// A ring that waited during the call rings properly once the call is over.
    func ended(_ session: CallSession, cause: CallEndCause) {
        Task { @MainActor in MacCallSystem.shared.updateRingtone() }
    }

    func requestHangUp(_ session: CallSession) -> Bool { false }

    func requestMute(_ session: CallSession, muted: Bool) -> Bool { false }

    /// Rings while a ring is pending (a banner, or the incoming screen in Ring's window) and no call
    /// is live; a ring during a call only plays the call-waiting tone.
    private func updateRingtone() {
        guard let model else { return }
        if (keepsRinging || model.inAppRing != nil) && model.activeCall?.isLive != true {
            Tones.shared.startRingtone()
        } else {
            Tones.shared.stopRingtone()
        }
    }

    /// Clicking the banner: the full incoming screen in Ring's window, with decline reasons. The
    /// ringtone carries on; the in-app screen now owns the ring.
    private func showInApp(_ invitationID: String) {
        guard let model, model.rings[invitationID] != nil else { return }
        if let index = banners.firstIndex(where: { $0.invitationID == invitationID }) {
            banners.remove(at: index).panel.dismiss()
            layout(animated: true)
        }
        if !model.inAppRingIDs.contains(invitationID) { model.inAppRingIDs.append(invitationID) }
        MainWindow.show()
    }

    private func returnInAppRings() {
        guard let model, model.inAppRing != nil, model.activeCall?.isLive != true else { return }
        model.handOffInAppRingsToSystem()
    }

    private func dismiss(_ invitationID: String) {
        if let index = banners.firstIndex(where: { $0.invitationID == invitationID }) {
            banners.remove(at: index).panel.dismiss()
            layout(animated: true)
        }
        updateRingtone()
        UNUserNotificationCenter.current().removeDeliveredNotifications(withIdentifiers: [invitationID])
    }

    // MARK: Placement

    /// The screen the stack lives on: the one the first banner went to while it is still attached,
    /// otherwise the screen in use.
    private var stackScreen: NSScreen? {
        if let screen = banners.first?.panel.screen, NSScreen.screens.contains(screen) { return screen }
        return CallBannerPanel.activeScreen
    }

    private func stackOffset(before index: Int) -> CGFloat {
        banners.prefix(index).reduce(0) { $0 + $1.panel.frame.height + CallBannerPanel.spacing }
    }

    /// Restacks the banners after one leaves or the displays change.
    private func layout(animated: Bool) {
        guard let screen = stackScreen else { return }
        for (index, banner) in banners.enumerated() {
            let target = CallBannerPanel.frame(size: banner.panel.fittedSize, on: screen, above: stackOffset(before: index))
            if animated { banner.panel.move(to: target) } else { banner.panel.setFrame(target, display: true) }
        }
    }

    // MARK: Lock screen

    private static var screenIsLocked: Bool {
        let session = CGSessionCopyCurrentDictionary() as? [String: Any]
        return session?["CGSSessionScreenIsLocked"] as? Bool ?? false
    }

    private func screenLocked() {
        guard let model else { return }
        for banner in banners {
            if let ring = model.rings[banner.invitationID] { postNotification(for: ring) }
        }
    }

    private func screenUnlocked() {
        UNUserNotificationCenter.current().removeDeliveredNotifications(withIdentifiers: banners.map(\.invitationID))
    }

    private func postNotification(for ring: IncomingRing) {
        let content = UNMutableNotificationContent()
        content.title = ring.from.label
        content.subtitle = ring.from.hasDisplayName ? ring.from.id : ""
        content.body = ring.alreadyOnCall.isEmpty ? "Is calling" : "Is calling, with \(ring.alreadyOnCall.joined(separator: ", "))"
        content.categoryIdentifier = Self.categoryID
        content.interruptionLevel = .timeSensitive
        content.threadIdentifier = ring.ringid
        content.userInfo = ["type": "call.incoming", "ringid": ring.ringid, "invitation_id": ring.invitationID]
        UNUserNotificationCenter.current().add(UNNotificationRequest(identifier: ring.invitationID, content: content, trigger: nil))
    }
}
