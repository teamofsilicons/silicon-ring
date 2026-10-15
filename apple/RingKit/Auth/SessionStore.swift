import Foundation
import Security

/// A signed-in Ring session. Sessions are bound to the server origin that issued them.
struct StoredSession: Codable, Sendable, Equatable {
    var token: String
    var server: URL
    var accountUUID: String
    var accountID: String
    var deviceID: String
    var expiresAt: Date
}

/// Keeps the session token in the Keychain. On iOS the item is readable after first unlock,
/// so a VoIP push can wake the app on a locked phone and still answer the call.
enum SessionStore {
    private static let service = "com.teamofsilicons.ring.session"
    private static let account = "session"

    private static var baseQuery: [String: Any] {
        [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: account,
        ]
    }

    static func load() -> StoredSession? {
        var query = baseQuery
        query[kSecReturnData as String] = true
        query[kSecMatchLimit as String] = kSecMatchLimitOne
        var result: CFTypeRef?
        guard SecItemCopyMatching(query as CFDictionary, &result) == errSecSuccess, let data = result as? Data else { return nil }
        return try? JSONDecoder().decode(StoredSession.self, from: data)
    }

    @discardableResult
    static func save(_ session: StoredSession) -> Bool {
        guard let data = try? JSONEncoder().encode(session) else { return false }
        SecItemDelete(baseQuery as CFDictionary)
        var item = baseQuery
        item[kSecValueData as String] = data
        #if os(iOS)
        item[kSecAttrAccessible as String] = kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly
        #endif
        return SecItemAdd(item as CFDictionary, nil) == errSecSuccess
    }

    static func clear() {
        SecItemDelete(baseQuery as CFDictionary)
    }
}
