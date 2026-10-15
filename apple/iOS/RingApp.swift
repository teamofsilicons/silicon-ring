import SwiftUI
import UIKit
import UserNotifications

@main
struct RingApp: App {
    @UIApplicationDelegateAdaptor(AppDelegate.self) private var appDelegate
    @Environment(\.scenePhase) private var scenePhase

    var body: some Scene {
        WindowGroup {
            RootView()
                .environment(AppModel.shared)
                .onOpenURL { _ in
                    // Sign-in callbacks are delivered to ASWebAuthenticationSession; nothing else uses the scheme yet.
                }
        }
        .onChange(of: scenePhase) { _, phase in
            let model = AppModel.shared
            switch phase {
            case .active:
                model.resume()
            case .background:
                // A call ringing in-app keeps ringing through CallKit once the app leaves the screen.
                model.handOffInAppRingsToSystem()
            default:
                break
            }
        }
    }
}

@MainActor
final class AppDelegate: NSObject, UIApplicationDelegate, UNUserNotificationCenterDelegate {
    func application(_ application: UIApplication, didFinishLaunchingWithOptions launchOptions: [UIApplication.LaunchOptionsKey: Any]? = nil) -> Bool {
        MainActor.assumeIsolated {
            let model = AppModel.shared
            CallMedia.prepareAudio()
            CallKitBridge.shared.install(model: model)
            UNUserNotificationCenter.current().delegate = self
            model.start()
            application.registerForRemoteNotifications()
        }
        return true
    }

    func application(_ application: UIApplication, didRegisterForRemoteNotificationsWithDeviceToken deviceToken: Data) {
        let token = deviceToken.map { String(format: "%02x", $0) }.joined()
        MainActor.assumeIsolated { AppModel.shared.apnsToken = token }
    }

    func application(_ application: UIApplication, didFailToRegisterForRemoteNotificationsWithError error: Error) {}

    // Missed calls and voicemail arrive as ordinary alerts; show them even in the foreground.
    nonisolated func userNotificationCenter(_ center: UNUserNotificationCenter, willPresent notification: UNNotification) async -> UNNotificationPresentationOptions {
        [.banner, .list, .sound]
    }

    nonisolated func userNotificationCenter(_ center: UNUserNotificationCenter, didReceive response: UNNotificationResponse) async {
        let type = response.notification.request.content.userInfo["type"] as? String
        await MainActor.run {
            AppModel.shared.openNotification(type: type)
        }
    }
}
