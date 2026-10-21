import AppKit
import SwiftUI

/// The call, small and always on top while Ring's window is not in front, as WhatsApp, Signal and
/// FaceTime do: who is on it (and who is speaking), the timer, the latest transcript line, Mute and
/// Hang Up. Drag it anywhere; it remembers where. Clicking it brings the full call back.
final class CallHUDPanel: NSPanel {
    static let size = NSSize(width: 320, height: 112)
    private static let originKey = "ring.callHUD.origin"

    @MainActor
    init(model: AppModel) {
        super.init(
            contentRect: NSRect(origin: .zero, size: Self.size),
            styleMask: [.borderless, .nonactivatingPanel],
            backing: .buffered,
            defer: true)
        isFloatingPanel = true
        // Above other apps' windows, below the incoming call banners.
        level = .floating
        collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary, .ignoresCycle, .canJoinAllApplications]
        hidesOnDeactivate = false
        canHide = false
        isReleasedWhenClosed = false
        becomesKeyOnlyIfNeeded = true
        isOpaque = false
        backgroundColor = .clear
        hasShadow = true
        animationBehavior = .utilityWindow
        let host = HUDHostingView(rootView: AnyView(CallHUDView().environment(model)))
        contentView = host
        setAccessibilityLabel("Ring call")
    }

    override var canBecomeKey: Bool { true }
    override var canBecomeMain: Bool { false }

    func show() {
        guard !isVisible else { return }
        setFrameOrigin(Self.restoredOrigin())
        clamp()
        orderFrontRegardless()
    }

    /// The last place the person dragged it to, while that is on a screen; otherwise the top-right
    /// corner of the screen in use, one banner down so an incoming call's banner fits above it.
    private static func restoredOrigin() -> NSPoint {
        if let saved = UserDefaults.standard.string(forKey: originKey) {
            let point = NSPointFromString(saved)
            let frame = NSRect(origin: point, size: size)
            if NSScreen.screens.contains(where: { $0.visibleFrame.intersects(frame) }) { return point }
        }
        guard let screen = CallBannerPanel.activeScreen else { return .zero }
        return CallBannerPanel.frame(size: size, on: screen, above: 64 + CallBannerPanel.spacing).origin
    }

    /// Keeps it fully on its screen (after a drag, a restore or a display change).
    func clamp() {
        guard let screen = screen ?? CallBannerPanel.activeScreen else { return }
        let visible = screen.visibleFrame.insetBy(dx: 8, dy: 8)
        var origin = frame.origin
        origin.x = min(max(origin.x, visible.minX), visible.maxX - frame.width)
        origin.y = min(max(origin.y, visible.minY), visible.maxY - frame.height)
        if origin != frame.origin { setFrameOrigin(origin) }
    }

    /// The person put it somewhere: keep it there, and there next time.
    func finishDrag() {
        clamp()
        UserDefaults.standard.set(NSStringFromPoint(frame.origin), forKey: Self.originKey)
        NotificationCenter.default.post(name: Self.didMove, object: self)
    }

    static let didMove = Notification.Name("ring.callHUD.didMove")
}

/// Buttons answer the first click even though the panel never activates Ring.
private final class HUDHostingView: NSHostingView<AnyView> {
    override func acceptsFirstMouse(for event: NSEvent?) -> Bool { true }
}

struct CallHUDView: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        if let session = model.activeCall, session.isLive {
            card(session)
        } else {
            Color.clear.frame(width: CallHUDPanel.size.width, height: CallHUDPanel.size.height)
        }
    }

    private func card(_ session: CallSession) -> some View {
        let people = session.others
        let first = people.first ?? session.counterpart
        let speaking = session.speakingUUIDs
        return VStack(alignment: .leading, spacing: 8) {
            HStack(spacing: 10) {
                ZStack {
                    AvatarView(account: first, size: 36)
                    SpeakingPulse(active: speaking.contains(first.uuid), color: Color(nsColor: .systemGreen))
                        .frame(width: 36, height: 36)
                }
                VStack(alignment: .leading, spacing: 1) {
                    Text(people.count > 1 ? "\(first.label) +\(people.count - 1)" : first.label)
                        .font(.system(size: 13, weight: .semibold))
                        .lineLimit(1)
                    HStack(spacing: 6) {
                        if session.phase == .active {
                            CallTimer(since: session.answeredAt)
                        } else {
                            Text(session.statusText)
                                .foregroundStyle(session.phase == .voicemail ? Color.accentColor : .secondary)
                        }
                        if session.muted {
                            Text("Muted").foregroundStyle(Color(nsColor: .systemRed))
                        } else if speaking.contains(session.myUUID) {
                            Text("Speaking")
                        }
                    }
                    .font(.system(size: 12))
                    .foregroundStyle(.secondary)
                }
                .frame(maxWidth: .infinity, alignment: .leading)
                HUDButton(
                    symbol: session.muted ? "mic.slash.fill" : "mic.fill",
                    label: session.muted ? "Unmute" : "Mute",
                    fill: session.muted ? Color.primary.opacity(0.85) : Color.primary.opacity(0.12),
                    tint: session.muted ? Color(nsColor: .windowBackgroundColor) : .primary,
                    action: model.toggleMute)
                HUDButton(
                    symbol: "phone.down.fill", label: "End call",
                    fill: Color(nsColor: .systemRed), tint: .white,
                    action: model.hangUp)
            }
            Text(caption(session))
                .font(.system(size: 12))
                .foregroundStyle(.secondary)
                .lineLimit(2)
                .frame(maxWidth: .infinity, minHeight: 30, alignment: .topLeading)
        }
        .padding(12)
        .frame(width: CallHUDPanel.size.width, height: CallHUDPanel.size.height, alignment: .top)
        .background { BannerBackground(radius: 20) }
        .contentShape(.rect(cornerRadius: 20))
        .onTapGesture(perform: openCall)
        .gesture(moveWindow)
        .help("Drag to move. Click to show the call in Ring.")
        .accessibilityElement(children: .contain)
        .accessibilityLabel("Call with \(first.label)")
        .accessibilityAction(named: "Show call in Ring", openCall)
    }

    /// The latest thing said, as in the call screen's live transcript.
    private func caption(_ session: CallSession) -> String {
        guard let line = session.visibleLines.last(where: { $0.style != .event }) else {
            return session.mediaState == .connected ? "Live transcript appears here." : "Connecting…"
        }
        let who = line.speaker.map { $0.uuid == session.myUUID ? "You" : $0.label } ?? "Someone"
        return "\(who): \(line.text)"
    }

    /// Drags the panel with the pointer. Where the drag started inside the card stays under the
    /// pointer, so nothing has to be remembered between drags (a cancelled one leaves nothing behind).
    private var moveWindow: some Gesture {
        DragGesture(minimumDistance: 3, coordinateSpace: .local)
            .onChanged { value in
                guard let panel else { return }
                let mouse = NSEvent.mouseLocation
                panel.setFrameOrigin(NSPoint(
                    x: mouse.x - value.startLocation.x,
                    y: mouse.y - (panel.frame.height - value.startLocation.y)))
            }
            .onEnded { _ in panel?.finishDrag() }
    }

    private var panel: CallHUDPanel? {
        NSApp.windows.lazy.compactMap { $0 as? CallHUDPanel }.first
    }

    private func openCall() {
        model.activeCall?.minimized = false
        MainWindow.show()
    }
}

private struct HUDButton: View {
    var symbol: String
    var label: String
    var fill: Color
    var tint: Color
    var action: () -> Void
    @State private var hovering = false

    var body: some View {
        Button(action: action) {
            Image(systemName: symbol)
                .font(.system(size: 13, weight: .semibold))
                .foregroundStyle(tint)
                .frame(width: 32, height: 32)
                .background(Circle().fill(fill))
                .brightness(hovering ? 0.05 : 0)
        }
        .buttonStyle(.plain)
        .onHover { hovering = $0 }
        .help(label)
        .accessibilityLabel(label)
    }
}
