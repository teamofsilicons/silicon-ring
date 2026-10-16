import AppKit
import SwiftUI

@main
struct RingMacApp: App {
    @NSApplicationDelegateAdaptor(MacAppDelegate.self) private var appDelegate

    init() {
        #if DEBUG
        DevHooks.applyLaunchArguments()
        #endif
    }

    var body: some Scene {
        Window("Ring", id: MainWindow.id) {
            RootView()
                .environment(AppModel.shared)
                .frame(minWidth: 720, minHeight: 520)
                .background(MainWindowObserver())
                .background(OpenWindowCapture())
        }
        .defaultSize(width: 980, height: 680)
        .windowToolbarStyle(.unified(showsTitle: false))
        .commands {
            CommandGroup(replacing: .newItem) {}
        }

        MenuBarExtra {
            MenuBarContent()
                .environment(AppModel.shared)
        } label: {
            MenuBarLabel()
        }
        .menuBarExtraStyle(.menu)
    }
}

/// Opens or focuses the main window from AppKit code (the ringing panel, notifications).
@MainActor
enum MainWindow {
    enum LaunchMode { case undecided, show, hide }

    static let id = "main"
    static var open: (() -> Void)?
    /// Set by the app delegate: a login-item launch stays in the menu bar, any other launch shows
    /// the window even when SwiftUI's scene restoration would leave it closed.
    static var launchMode = LaunchMode.undecided

    static var window: NSWindow? {
        NSApp.windows.first { $0.identifier?.rawValue == id || $0.title == "Ring" && $0.canBecomeMain }
    }

    static func show() {
        NSApp.setActivationPolicy(.regular)
        if let window {
            window.makeKeyAndOrderFront(nil)
        } else {
            open?()
        }
        NSApp.activate()
    }

    /// Applies the launch mode once both the delegate has decided and `openWindow` is available.
    static func applyLaunchMode() {
        switch launchMode {
        case .undecided:
            return
        case .show:
            guard open != nil else { return }
            launchMode = .undecided
            if window?.isVisible != true { show() }
        case .hide:
            guard let window else { return }
            launchMode = .undecided
            window.close()
            NSApp.setActivationPolicy(.accessory)
        }
    }
}

/// The menu bar icon; also captures `openWindow` for AppKit callers, since it is always on screen.
private struct MenuBarLabel: View {
    @Environment(\.openWindow) private var openWindow

    private static let icon: NSImage = {
        let image = NSImage(named: "RingMarkTemplate") ?? NSImage()
        image.size = NSSize(width: 18, height: 18)
        image.isTemplate = true
        return image
    }()

    var body: some View {
        Image(nsImage: Self.icon)
            .onAppear {
                MainWindow.open = { openWindow(id: MainWindow.id) }
                MainWindow.applyLaunchMode()
            }
    }
}

/// Keeps an `openWindow` action for AppKit callers once the main window has appeared.
private struct OpenWindowCapture: View {
    @Environment(\.openWindow) private var openWindow

    var body: some View {
        Color.clear.onAppear {
            MainWindow.open = { openWindow(id: MainWindow.id) }
        }
    }
}

/// Hides the Dock icon when the window closes and Ring keeps running in the menu bar.
private struct MainWindowObserver: NSViewRepresentable {
    func makeNSView(context: Context) -> NSView {
        let view = NSView()
        DispatchQueue.main.async {
            guard let window = view.window else { return }
            window.identifier = NSUserInterfaceItemIdentifier(MainWindow.id)
            window.isRestorable = false
            // Paper all the way up: the titlebar shows the window background, as on teamofsilicons.com.
            window.titlebarAppearsTransparent = true
            window.styleMask.insert(.fullSizeContentView)
            window.backgroundColor = NSColor(named: "background")
            context.coordinator.observe(window)
            MainWindow.applyLaunchMode()
        }
        return view
    }

    func updateNSView(_ nsView: NSView, context: Context) {}

    func makeCoordinator() -> Coordinator { Coordinator() }

    @MainActor
    final class Coordinator {
        private var token: NSObjectProtocol?

        func observe(_ window: NSWindow) {
            token = NotificationCenter.default.addObserver(forName: NSWindow.willCloseNotification, object: window, queue: .main) { _ in
                MainActor.assumeIsolated {
                    if DesktopPreferences.keepInMenuBar {
                        NSApp.setActivationPolicy(.accessory)
                    }
                }
            }
        }
    }
}

struct MenuBarContent: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        if !model.isSignedIn {
            Text("Signed out")
            Button("Sign in…") { MainWindow.show() }
        } else {
            Text(statusText)
            if let ring = model.rings.values.first {
                Divider()
                Text("\(ring.from.label) is calling")
                Button("Answer") { model.answer(ring) }
                Button("Decline") { model.decline(ring, reason: nil) }
                Button("Silence") { model.silence(ring) }
            }
            if let session = model.activeCall, session.isLive {
                Divider()
                Text("On call with \(session.counterpart.label)")
                Button(session.muted ? "Unmute" : "Mute") { model.toggleMute() }
                Button("End call") { model.hangUp() }
                Button("Show call") {
                    session.minimized = false
                    MainWindow.show()
                }
            } else if let other = model.callOnOtherDevice {
                Divider()
                Text("On a call with \(other.call.others(me: model.myUUID).first?.label ?? "someone") on \(model.deviceLabel(other.device))")
                Button("Switch to this Mac") { model.switchHere(other.call) }
            }
            Divider()
            Button("Open Ring") { MainWindow.show() }
                .keyboardShortcut("o")
        }
        Divider()
        Button("Quit Ring") { NSApp.terminate(nil) }
            .keyboardShortcut("q")
    }

    private var statusText: String {
        switch model.realtimeStatus {
        case .connected: model.currentDevice?.ringEnabled == false ? "Ringing is off on this Mac" : "Listening for calls"
        case .connecting: "Connecting…"
        case .waiting: "Offline, retrying…"
        case .idle: "Not connected"
        }
    }
}
