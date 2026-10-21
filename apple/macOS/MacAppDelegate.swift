import AppKit
import Carbon.HIToolbox
import Observation
import ServiceManagement
import UserNotifications

@MainActor
final class MacAppDelegate: NSObject, NSApplicationDelegate, UNUserNotificationCenterDelegate {
    private var wakeObserver: NSObjectProtocol?
    private var activity: NSObjectProtocol?

    func applicationDidFinishLaunching(_ notification: Notification) {
        MainActor.assumeIsolated {
            let model = AppModel.shared
            CallMedia.prepareAudio()
            MacCallSystem.shared.install(model: model)

            let center = UNUserNotificationCenter.current()
            center.delegate = self
            center.setNotificationCategories([MacCallSystem.ringCategory])

            NSApp.registerForRemoteNotifications()
            model.start()
            keepAwakeWhileListening()

            wakeObserver = NSWorkspace.shared.notificationCenter.addObserver(
                forName: NSWorkspace.didWakeNotification, object: nil, queue: .main
            ) { _ in
                MainActor.assumeIsolated { AppModel.shared.resume() }
            }

            // Ring must keep running with no windows open, or it stops listening for calls.
            ProcessInfo.processInfo.automaticTerminationSupportEnabled = false

            if launchedAsLoginItem && model.isSignedIn {
                // At login Ring starts quietly in the menu bar.
                MainWindow.launchMode = .hide
                NSApp.setActivationPolicy(.accessory)
            } else {
                MainWindow.launchMode = .show
            }
            DispatchQueue.main.async { MainWindow.applyLaunchMode() }
            // Launch handling only concerns the first window; never close one the user opens later.
            DispatchQueue.main.asyncAfter(deadline: .now() + 3) { MainWindow.launchMode = .undecided }
        }
    }

    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool {
        !MainActor.assumeIsolated { DesktopPreferences.keepInMenuBar }
    }

    func applicationShouldHandleReopen(_ sender: NSApplication, hasVisibleWindows flag: Bool) -> Bool {
        if !flag { MainActor.assumeIsolated { MainWindow.show() } }
        return true
    }

    func applicationDidBecomeActive(_ notification: Notification) {
        MainActor.assumeIsolated { AppModel.shared.resume() }
    }

    func application(_ application: NSApplication, didRegisterForRemoteNotificationsWithDeviceToken deviceToken: Data) {
        let token = deviceToken.map { String(format: "%02x", $0) }.joined()
        MainActor.assumeIsolated { AppModel.shared.apnsToken = token }
    }

    func application(_ application: NSApplication, didFailToRegisterForRemoteNotificationsWithError error: Error) {}

    private var launchedAsLoginItem: Bool {
        guard let event = NSAppleEventManager.shared().currentAppleEvent else { return false }
        return event.eventID == kAEOpenApplication
            && event.paramDescriptor(forKeyword: keyAEPropData)?.enumCodeValue == keyAELaunchedAsLogInItem
    }

    /// App Nap would throttle the socket that makes the Mac ring; hold an activity while listening.
    @MainActor
    private func keepAwakeWhileListening() {
        let model = AppModel.shared
        let listening = model.isSignedIn
        if listening, activity == nil {
            activity = ProcessInfo.processInfo.beginActivity(
                options: [.userInitiatedAllowingIdleSystemSleep, .suddenTerminationDisabled],
                reason: "Listening for Ring calls")
        } else if !listening, let current = activity {
            ProcessInfo.processInfo.endActivity(current)
            activity = nil
        }
        withObservationTracking {
            _ = model.isSignedIn
        } onChange: { [weak self] in
            Task { @MainActor in self?.keepAwakeWhileListening() }
        }
    }

    // MARK: Notifications

    nonisolated func userNotificationCenter(_ center: UNUserNotificationCenter, willPresent notification: UNNotification) async -> UNNotificationPresentationOptions {
        let isRemote = notification.request.trigger is UNPushNotificationTrigger
        let type = notification.request.content.userInfo["type"] as? String
        if isRemote && type == "call.incoming" {
            // The server only sends this while it thinks the Mac is offline. Ring is running, so
            // reconnect and let the call banner take over.
            await MainActor.run { AppModel.shared.resume() }
            return []
        }
        if isRemote { await MainActor.run { AppModel.shared.resume() } }
        return [.banner, .list, .sound]
    }

    nonisolated func userNotificationCenter(_ center: UNUserNotificationCenter, didReceive response: UNNotificationResponse) async {
        let info = response.notification.request.content.userInfo
        let action = response.actionIdentifier
        let ringid = info["ringid"] as? String
        let invitationID = info["invitation_id"] as? String
        let type = info["type"] as? String
        await MainActor.run {
            let model = AppModel.shared
            switch action {
            case MacCallSystem.answerAction:
                if let invitationID, let ring = model.rings[invitationID] {
                    model.answer(ring)
                } else if let ringid {
                    model.open(ringid: ringid, invitationID: invitationID, answer: true)
                }
                MainWindow.show()
            case MacCallSystem.declineAction:
                if let invitationID, let ring = model.rings[invitationID] {
                    model.decline(ring, reason: nil)
                } else if let ringid {
                    Task { try? await model.api.decline(ringid, invitationID: invitationID, reason: nil) }
                }
            default:
                if type == "call.incoming", let ringid, model.rings[invitationID ?? ""] == nil {
                    model.open(ringid: ringid, invitationID: invitationID, answer: false)
                }
                MainWindow.show()
                model.openNotification(type: type)
            }
        }
    }
}

/// Mac-only preferences.
@MainActor
enum DesktopPreferences {
    private static let keepKey = "ring.keepInMenuBar"
    private static let loginConfiguredKey = "ring.loginItemConfigured"

    static var keepInMenuBar: Bool {
        get { UserDefaults.standard.object(forKey: keepKey) as? Bool ?? true }
        set { UserDefaults.standard.set(newValue, forKey: keepKey) }
    }

    static var openAtLogin: Bool {
        SMAppService.mainApp.status == .enabled
    }

    static var loginNeedsApproval: Bool {
        SMAppService.mainApp.status == .requiresApproval
    }

    static func setOpenAtLogin(_ enabled: Bool) throws {
        UserDefaults.standard.set(true, forKey: loginConfiguredKey)
        if enabled {
            try SMAppService.mainApp.register()
        } else {
            try SMAppService.mainApp.unregister()
        }
    }

    /// Ring listens for calls from login by default; once the user changes it, their choice stands.
    static func enableLoginItemByDefault() {
        guard !UserDefaults.standard.bool(forKey: loginConfiguredKey) else { return }
        UserDefaults.standard.set(true, forKey: loginConfiguredKey)
        try? SMAppService.mainApp.register()
    }
}
