#if DEBUG
import Foundation

/// Debug-only launch arguments for driving Ring on a device nobody is holding, for example from
/// `xcrun devicectl device process launch … -- -RingAutoAnswer YES`. Compiled out of Release.
///
/// - `-RingDevSession rst_… -RingDevAccountUUID … -RingDevAccountID c:… -RingDevDeviceID dev_…`
///   installs a session minted on a development server.
/// - `-RingAutoAnswer YES` answers incoming calls after two seconds.
/// - `-RingAutoHandoff YES` moves this account's call here when another device carries it.
/// - `-RingKeepAwake YES` keeps the screen on while Ring is in the foreground.
/// - `-RingDemo recents|call|incoming|voicemail|dialer` (iPhone and iPad) opens one screen filled
///   with made-up people and calls, for App Store screenshots. It needs no server (DemoMode.swift).
enum DevHooks {
    private static var defaults: UserDefaults { .standard }

    // Launch arguments only reach launches made with them, but a VoIP push wakes the app without
    // any, so each flag is remembered until a later launch passes it again.
    static var autoAnswer: Bool { flag("RingAutoAnswer") }
    static var autoHandoff: Bool { flag("RingAutoHandoff") }
    static var keepAwake: Bool { flag("RingKeepAwake") }
    /// `-RingForceCallKit YES`: ring through CallKit even in the foreground, so the system call
    /// flow (CXAnswer → fulfil → audio session → media) runs on a device nobody is holding.
    static var forceCallKit: Bool { flag("RingForceCallKit") }

    private static func launchValue(_ name: String) -> String? {
        let args = ProcessInfo.processInfo.arguments
        guard let index = args.firstIndex(of: "-\(name)"), index + 1 < args.count else { return nil }
        return args[index + 1]
    }

    private static func flag(_ name: String) -> Bool {
        if let launched = launchValue(name) {
            let on = ["yes", "true", "1"].contains(launched.lowercased())
            defaults.set(on, forKey: "ring.dev.\(name)")
            return on
        }
        return defaults.bool(forKey: "ring.dev.\(name)")
    }

    /// Runs before `AppModel.shared` exists, so an installed session is picked up on load.
    @MainActor
    static func applyLaunchArguments() {
        #if os(iOS) && targetEnvironment(simulator)
        // Simulator only: demo mode replaces the signed-in session, which on a real device would
        // leave its server session and VoIP registration behind.
        if let scene = launchValue("RingDemo").flatMap(DemoScene.init(rawValue:)) {
            DemoMode.start(scene)
            return
        }
        #endif
        // `-RingDevServer https://…` persists like the developer setting, so push-woken launches
        // (which get no arguments) keep talking to the same server.
        if let server = launchValue("RingDevServer").flatMap(ServerConfig.normalize) {
            ServerConfig.override = server == ServerConfig.defaultURL ? nil : server
        }
        if let token = defaults.string(forKey: "RingDevSession"),
           let uuid = defaults.string(forKey: "RingDevAccountUUID"),
           let id = defaults.string(forKey: "RingDevAccountID"),
           let deviceID = defaults.string(forKey: "RingDevDeviceID"),
           SessionStore.load()?.token != token {
            SessionStore.save(StoredSession(
                token: token, server: ServerConfig.current, accountUUID: uuid, accountID: id,
                deviceID: deviceID, expiresAt: Date().addingTimeInterval(30 * 86_400)))
        }
    }

    @MainActor
    static func ringPresented(_ ring: IncomingRing, model: AppModel) {
        guard autoAnswer else { return }
        Task { @MainActor in
            try? await Task.sleep(for: .seconds(2))
            guard model.rings[ring.invitationID] != nil else { return }
            model.answer(ring)
        }
    }

    @MainActor
    static func callChanged(model: AppModel) {
        guard autoHandoff, let elsewhere = model.callOnOtherDevice else { return }
        let ringid = elsewhere.call.ringid
        Task { @MainActor in
            try? await Task.sleep(for: .seconds(3))
            guard let current = model.callOnOtherDevice, current.call.ringid == ringid else { return }
            model.switchHere(current.call)
        }
    }
}
#endif
