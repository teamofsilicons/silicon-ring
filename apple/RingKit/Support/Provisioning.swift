import Foundation

/// Reads the APNs environment from the embedded provisioning profile, so the server sends to
/// the gateway that matches how this build was signed (development → sandbox).
enum Provisioning {
    static let apnsEnvironment: PushEnvironment = {
        #if os(iOS)
        let url = Bundle.main.url(forResource: "embedded", withExtension: "mobileprovision")
        let key = "aps-environment"
        #else
        let url: URL? = Bundle.main.bundleURL.appendingPathComponent("Contents/embedded.provisionprofile")
        let key = "com.apple.developer.aps-environment"
        #endif
        if let url, let value = entitlement(key, profileURL: url) {
            return value == "development" ? .sandbox : .production
        }
        #if DEBUG
        return .sandbox
        #else
        return .production
        #endif
    }()

    private static func entitlement(_ key: String, profileURL: URL) -> String? {
        guard let profile = try? Data(contentsOf: profileURL),
              let start = profile.range(of: Data("<?xml".utf8))?.lowerBound ?? profile.range(of: Data("<plist".utf8))?.lowerBound,
              let end = profile.range(of: Data("</plist>".utf8), in: start..<profile.endIndex)?.upperBound,
              let plist = try? PropertyListSerialization.propertyList(from: profile[start..<end], format: nil) as? [String: Any],
              let entitlements = plist["Entitlements"] as? [String: Any] else { return nil }
        return entitlements[key] as? String
    }
}
