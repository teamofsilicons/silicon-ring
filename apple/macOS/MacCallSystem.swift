import AppKit
import SwiftUI
import UserNotifications

/// The Mac's call integration: a floating ringing panel, a looping ringtone and a time-sensitive
/// notification with Answer and Decline. macOS has no CallKit, so audio is managed directly.
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
    private var panels: [String: RingingPanel] = [:]

    func install(model: AppModel) {
        self.model = model
        model.callSystem = self
        model.onSignIn = { DesktopPreferences.enableLoginItemByDefault() }
    }

    func ring(_ ring: IncomingRing) -> Bool {
        guard let model, panels[ring.invitationID] == nil else { return true }
        let panel = RingingPanel(ring: ring, model: model, stackIndex: panels.count)
        panels[ring.invitationID] = panel
        panel.orderFrontRegardless()
        if model.activeCall?.isLive == true {
            Tones.shared.play(.connected)
        } else {
            Tones.shared.startRingtone()
        }
        postNotification(for: ring)
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
        if session.role != .outgoing {
            session.minimized = false
            MainWindow.show()
        }
    }

    func connected(_ session: CallSession) {}

    func ended(_ session: CallSession, cause: CallEndCause) {}

    func requestHangUp(_ session: CallSession) -> Bool { false }

    func requestMute(_ session: CallSession, muted: Bool) -> Bool { false }

    private func dismiss(_ invitationID: String) {
        panels.removeValue(forKey: invitationID)?.close()
        if panels.isEmpty { Tones.shared.stopRingtone() }
        UNUserNotificationCenter.current().removeDeliveredNotifications(withIdentifiers: [invitationID])
        UNUserNotificationCenter.current().removePendingNotificationRequests(withIdentifiers: [invitationID])
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
        let request = UNNotificationRequest(identifier: ring.invitationID, content: content, trigger: nil)
        UNUserNotificationCenter.current().add(request)
    }
}

/// The floating, non-activating ringing panel in the top-right corner of the screen.
final class RingingPanel: NSPanel {
    private var pinnedTop: CGFloat?

    @MainActor
    init(ring: IncomingRing, model: AppModel, stackIndex: Int) {
        super.init(
            contentRect: NSRect(x: 0, y: 0, width: 340, height: 400),
            styleMask: [.nonactivatingPanel, .titled, .fullSizeContentView],
            backing: .buffered,
            defer: false)
        isFloatingPanel = true
        level = .statusBar
        collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary, .ignoresCycle]
        hidesOnDeactivate = false
        isReleasedWhenClosed = false
        becomesKeyOnlyIfNeeded = true
        isMovableByWindowBackground = true
        titleVisibility = .hidden
        titlebarAppearsTransparent = true
        standardWindowButton(.closeButton)?.isHidden = true
        standardWindowButton(.miniaturizeButton)?.isHidden = true
        standardWindowButton(.zoomButton)?.isHidden = true
        backgroundColor = NSColor(red: 0x18 / 255, green: 0x18 / 255, blue: 0x18 / 255, alpha: 1)
        appearance = NSAppearance(named: .darkAqua)

        let controller = NSHostingController(rootView: IncomingCallView(ring: ring, compact: true).environment(model))
        controller.sizingOptions = [.preferredContentSize]
        contentViewController = controller

        if let screen = NSScreen.main ?? NSScreen.screens.first {
            let visible = screen.visibleFrame
            let size = frame.size
            let top = visible.maxY - 16 - CGFloat(stackIndex) * 24
            pinnedTop = top
            setFrameOrigin(NSPoint(x: visible.maxX - size.width - 16, y: top - size.height))
        }
    }

    override var canBecomeKey: Bool { true }
    override var canBecomeMain: Bool { false }

    /// Keep the top edge fixed when SwiftUI grows the panel (the decline reasons).
    override func setFrame(_ frameRect: NSRect, display flag: Bool) {
        var rect = frameRect
        if let pinnedTop { rect.origin.y = pinnedTop - rect.height }
        super.setFrame(rect, display: flag)
    }
}
