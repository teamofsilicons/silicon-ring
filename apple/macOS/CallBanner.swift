import AppKit
import SwiftUI

/// An incoming call on the Mac, shown like FaceTime's: a banner in the top-right corner of the
/// screen in use, with the caller and round Decline and Accept buttons. It floats above every app,
/// on every Space and over full-screen apps, never takes focus from what the person is doing, and
/// is left alone by Stage Manager (a titled window there gets pushed off to the side).
final class CallBannerPanel: NSPanel {
    static let width: CGFloat = 372
    static let margin: CGFloat = 12
    static let spacing: CGFloat = 8

    private let host: NSHostingView<AnyView>

    @MainActor
    init(ring: IncomingRing, model: AppModel, onShowInApp: @escaping () -> Void) {
        let view = CallBannerView(ring: ring, onShowInApp: onShowInApp).environment(model)
        host = FirstClickHostingView(rootView: AnyView(view))
        super.init(
            contentRect: NSRect(x: 0, y: 0, width: Self.width, height: 80),
            styleMask: [.borderless, .nonactivatingPanel],
            backing: .buffered,
            defer: false)
        isFloatingPanel = true
        level = .statusBar
        // canJoinAllApplications: an overlay that joins other apps' full-screen spaces and Stage
        // Manager sets, rather than one of Ring's own windows.
        collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary, .stationary, .ignoresCycle, .canJoinAllApplications]
        hidesOnDeactivate = false
        // Still shown, and still able to appear, while Ring itself is hidden (⌘H, Hide Others).
        canHide = false
        isReleasedWhenClosed = false
        becomesKeyOnlyIfNeeded = true
        isMovable = false
        isOpaque = false
        backgroundColor = .clear
        hasShadow = true
        animationBehavior = .none
        contentView = host
        setContentSize(fittedSize)
        setAccessibilityLabel("Incoming call from \(ring.from.label)")
    }

    override var canBecomeKey: Bool { true }
    override var canBecomeMain: Bool { false }

    var fittedSize: NSSize {
        host.layoutSubtreeIfNeeded()
        return NSSize(width: Self.width, height: max(host.fittingSize.height, 64))
    }

    /// The screen the person is working on: the one under the pointer.
    static var activeScreen: NSScreen? {
        let mouse = NSEvent.mouseLocation
        return NSScreen.screens.first { NSMouseInRect(mouse, $0.frame, false) } ?? NSScreen.main ?? NSScreen.screens.first
    }

    /// Where the banner sits, `above` points from the top of the screen's usable area.
    static func frame(size: NSSize, on screen: NSScreen, above offset: CGFloat) -> NSRect {
        let visible = screen.visibleFrame
        return NSRect(
            x: visible.maxX - size.width - margin,
            y: visible.maxY - margin - offset - size.height,
            width: size.width,
            height: size.height)
    }

    /// Slides in from the right edge, as notifications do.
    func present(at target: NSRect) {
        setFrame(target.offsetBy(dx: target.width / 3, dy: 0), display: false)
        alphaValue = 0
        orderFrontRegardless()
        NSAnimationContext.runAnimationGroup { context in
            context.duration = NSWorkspace.shared.accessibilityDisplayShouldReduceMotion ? 0 : 0.28
            context.timingFunction = CAMediaTimingFunction(controlPoints: 0.2, 0.9, 0.3, 1)
            animator().setFrame(target, display: true)
            animator().alphaValue = 1
        }
    }

    func move(to target: NSRect) {
        NSAnimationContext.runAnimationGroup { context in
            context.duration = NSWorkspace.shared.accessibilityDisplayShouldReduceMotion ? 0 : 0.2
            animator().setFrame(target, display: true)
        }
    }

    func dismiss() {
        ignoresMouseEvents = true
        NSAnimationContext.runAnimationGroup({ context in
            context.duration = NSWorkspace.shared.accessibilityDisplayShouldReduceMotion ? 0 : 0.18
            animator().alphaValue = 0
        }, completionHandler: { [weak self] in
            MainActor.assumeIsolated { self?.close() }
        })
    }
}

/// Buttons in a panel that never becomes active still answer the first click.
private final class FirstClickHostingView: NSHostingView<AnyView> {
    override func acceptsFirstMouse(for event: NSEvent?) -> Bool { true }
}

/// The banner's content: who is calling, and round Decline and Accept buttons. A right-click offers
/// declining with a reason and silencing; clicking anywhere else opens the call in Ring.
struct CallBannerView: View {
    var ring: IncomingRing
    var onShowInApp: () -> Void
    @Environment(AppModel.self) private var model
    /// Set by the first action, so a second click (or one during the accept) does nothing.
    @State private var acted: Action?

    private enum Action { case accept, decline }

    private static let reasons = ["Can't talk right now", "In a meeting", "Call you back soon", "Text me instead"]

    var body: some View {
        HStack(spacing: 12) {
            AvatarView(account: ring.from, size: 42)
            VStack(alignment: .leading, spacing: 2) {
                Text(ring.from.label)
                    .font(.system(size: 13, weight: .semibold))
                    .foregroundStyle(.primary)
                    .lineLimit(1)
                Text(acted == .accept ? "Connecting…" : subtitle)
                    .font(.system(size: 12))
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            CallBannerButton(kind: .decline) { act(.decline) { model.decline(ring, reason: nil) } }
                .disabled(acted != nil)
            CallBannerButton(kind: .accept) { act(.accept) { model.answer(ring) } }
                .disabled(acted != nil)
        }
        .padding(.leading, 12)
        .padding(.trailing, 12)
        .padding(.vertical, 12)
        .frame(width: CallBannerPanel.width)
        .contentShape(.rect(cornerRadius: CallBannerView.radius))
        .onTapGesture { if acted == nil { onShowInApp() } }
        .contextMenu {
            if acted == nil {
                Menu("Decline with Message") {
                    ForEach(Self.reasons, id: \.self) { reason in
                        Button(reason) { act(.decline) { model.decline(ring, reason: reason) } }
                    }
                }
                Button("Silence") { act(.decline) { model.silence(ring) } }
                Divider()
                Button("Show in Ring", action: onShowInApp)
            }
        }
        .background { BannerBackground(radius: Self.radius) }
        .accessibilityElement(children: .contain)
        .accessibilityLabel("Incoming call from \(ring.from.label)")
        .accessibilityAction(named: "Silence") { model.silence(ring) }
        .accessibilityAction(named: "Show in Ring", onShowInApp)
    }

    static let radius: CGFloat = 22

    private func act(_ action: Action, _ perform: () -> Void) {
        guard acted == nil else { return }
        acted = action
        perform()
    }

    private var subtitle: String {
        let kind = ring.from.kind == .silicon ? "Ring Audio · Silicon" : "Ring Audio"
        return ring.alreadyOnCall.isEmpty ? kind : "\(kind) · with \(ring.alreadyOnCall.joined(separator: ", "))"
    }
}

/// FaceTime's call buttons: a red circle to decline, a green one to accept.
private struct CallBannerButton: View {
    enum Kind { case accept, decline }
    var kind: Kind
    var action: () -> Void
    @State private var hovering = false

    var body: some View {
        Button(action: action) {
            Image(systemName: kind == .accept ? "phone.fill" : "phone.down.fill")
                .font(.system(size: 15, weight: .semibold))
                .foregroundStyle(.white)
                .frame(width: 36, height: 36)
                .background(Circle().fill(kind == .accept ? Color(nsColor: .systemGreen) : Color(nsColor: .systemRed)))
                .brightness(hovering ? 0.06 : 0)
        }
        .buttonStyle(BannerPressStyle())
        .onHover { hovering = $0 }
        .help(kind == .accept ? "Accept" : "Decline")
        .accessibilityLabel(kind == .accept ? "Accept" : "Decline")
    }
}

private struct BannerPressStyle: ButtonStyle {
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .brightness(configuration.isPressed ? -0.08 : 0)
            .scaleEffect(configuration.isPressed && !reduceMotion ? 0.94 : 1)
            .animation(.easeOut(duration: 0.12), value: configuration.isPressed)
    }
}

/// The glass behind the call banner and the floating call window: the system's own Liquid Glass on
/// macOS 26, so it follows the person's choice of clear or tinted glass (System Settings ›
/// Appearance) and Reduce Transparency, like notifications and widgets; the notification material on
/// earlier systems.
struct BannerBackground: View {
    var radius: CGFloat

    var body: some View {
        if #available(macOS 26, *) {
            Color.clear
                .glassEffect(.regular, in: .rect(cornerRadius: radius))
        } else {
            BehindWindowBlur()
                .clipShape(.rect(cornerRadius: radius))
                .overlay(RoundedRectangle(cornerRadius: radius).strokeBorder(.white.opacity(0.12), lineWidth: 0.5))
        }
    }
}

private struct BehindWindowBlur: NSViewRepresentable {
    func makeNSView(context: Context) -> NSVisualEffectView {
        let view = NSVisualEffectView()
        view.material = .popover
        view.blendingMode = .behindWindow
        view.state = .active
        return view
    }

    func updateNSView(_ view: NSVisualEffectView, context: Context) {}
}
