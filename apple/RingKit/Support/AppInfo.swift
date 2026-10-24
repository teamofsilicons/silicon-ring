import Foundation
#if canImport(UIKit)
import UIKit
#endif

/// Facts about this build and device that the server needs.
enum AppInfo {
    static var version: String {
        Bundle.main.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String ?? "1.0.0"
    }

    static var build: String {
        Bundle.main.object(forInfoDictionaryKey: "CFBundleVersion") as? String ?? "1"
    }

    static var userAgent: String {
        #if os(iOS)
        "Ring/\(version) (iOS)"
        #else
        "Ring/\(version) (macOS)"
        #endif
    }

    @MainActor
    static var platform: Platform {
        #if os(iOS)
        UIDevice.current.userInterfaceIdiom == .pad ? .ipados : .ios
        #else
        .macos
        #endif
    }

    /// The default name for this device; the user can rename it in Settings.
    @MainActor
    static var deviceName: String {
        #if os(iOS)
        UIDevice.current.name
        #else
        Host.current().localizedName ?? "Mac"
        #endif
    }

    static let sourceURL = URL(string: "https://github.com/teamofsilicons/silicon-ring")!
    static let privacyURL = URL(string: "https://ring.teamofsilicons.com/privacy")!
    static let supportURL = URL(string: "https://ring.teamofsilicons.com/support")!
    static let contactEmail = "lords@teamofsilicons.com"
    static let defaultAccountsURL = URL(string: "https://accounts.teamofsilicons.com")!
    static let nativeRedirectURI = "com.teamofsilicons.ring:/auth/callback"
    static let callbackScheme = "com.teamofsilicons.ring"
}

/// Which Ring server this app talks to. Debug builds point at the developer's LAN server.
enum ServerConfig {
    #if DEBUG
    static let defaultURL = URL(string: "http://192.168.29.75:8765")!
    #else
    static let defaultURL = URL(string: "https://backend.ring.teamofsilicons.com")!
    #endif

    private static let overrideKey = "ring.server.override"

    static var override: URL? {
        get {
            guard let text = UserDefaults.standard.string(forKey: overrideKey) else { return nil }
            return URL(string: text)
        }
        set {
            if let newValue {
                UserDefaults.standard.set(newValue.absoluteString, forKey: overrideKey)
            } else {
                UserDefaults.standard.removeObject(forKey: overrideKey)
            }
        }
    }

    /// Release builds always use the default: the switch is a Debug-only developer setting, and an
    /// override left behind by a Debug build on the same device must not strand a Release one.
    static var current: URL {
        #if DEBUG
        override ?? defaultURL
        #else
        defaultURL
        #endif
    }

    /// Accepts `http(s)://host[:port]` and drops any path.
    static func normalize(_ text: String) -> URL? {
        guard var components = URLComponents(string: text.trimmingCharacters(in: .whitespacesAndNewlines)),
              let scheme = components.scheme?.lowercased(), scheme == "http" || scheme == "https",
              let host = components.host, !host.isEmpty else { return nil }
        components.path = ""
        components.query = nil
        components.fragment = nil
        return components.url
    }

    static func webSocketURL(for base: URL) -> URL? {
        guard var components = URLComponents(url: base, resolvingAgainstBaseURL: false) else { return nil }
        components.scheme = components.scheme == "https" ? "wss" : "ws"
        components.path = "/v1/ws"
        return components.url
    }
}
