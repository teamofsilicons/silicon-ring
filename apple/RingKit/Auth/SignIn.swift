import AuthenticationServices
import CryptoKit
import Foundation
import SwiftUI

/// One hosted Silicon Accounts sign-in attempt (PKCE S256, RFC 8252).
struct SignInAttempt: Sendable {
    let verifier: String
    let state: String
    let redirectURI: String
    let authorizeURL: URL

    init(accountsURL: URL, redirectURI: String) {
        verifier = Self.randomURLSafe(bytes: 48)
        state = Self.randomURLSafe(bytes: 24)
        self.redirectURI = redirectURI
        let challenge = Data(SHA256.hash(data: Data(verifier.utf8))).base64URLEncoded

        var components = URLComponents(url: accountsURL, resolvingAgainstBaseURL: false)!
        components.path = (components.path.hasSuffix("/") ? String(components.path.dropLast()) : components.path) + "/authorize"
        components.queryItems = [
            URLQueryItem(name: "response_type", value: "code"),
            URLQueryItem(name: "app_id", value: "ring"),
            URLQueryItem(name: "redirect_uri", value: redirectURI),
            URLQueryItem(name: "scope", value: "profile"),
            URLQueryItem(name: "state", value: state),
            URLQueryItem(name: "code_challenge", value: challenge),
            URLQueryItem(name: "code_challenge_method", value: "S256"),
        ]
        authorizeURL = components.url!
    }

    enum Outcome: Sendable {
        case code(String)
        case cancelled
    }

    /// Validates the redirect and extracts the single-use code.
    func outcome(from callback: URL) throws -> Outcome {
        let items = URLComponents(url: callback, resolvingAgainstBaseURL: false)?.queryItems ?? []
        func value(_ name: String) -> String? { items.first { $0.name == name }?.value }
        if let error = value("error") {
            if error == "access_denied" { return .cancelled }
            throw RingError.api(status: 400, info: APIErrorInfo(code: error, message: value("error_description") ?? "Sign-in failed (\(error))."))
        }
        guard value("state") == state else {
            throw RingError.api(status: 400, info: APIErrorInfo(code: "state_mismatch", message: "The sign-in response didn't match this attempt. Try again."))
        }
        guard let code = value("code"), !code.isEmpty else {
            throw RingError.protocolError("The sign-in response had no code.")
        }
        return .code(code)
    }

    private static func randomURLSafe(bytes count: Int) -> String {
        var bytes = [UInt8](repeating: 0, count: count)
        let status = SecRandomCopyBytes(kSecRandomDefault, count, &bytes)
        precondition(status == errSecSuccess, "SecRandomCopyBytes failed")
        return Data(bytes).base64URLEncoded
    }
}

extension Data {
    var base64URLEncoded: String {
        base64EncodedString()
            .replacingOccurrences(of: "+", with: "-")
            .replacingOccurrences(of: "/", with: "_")
            .replacingOccurrences(of: "=", with: "")
    }
}

extension WebAuthenticationSession {
    /// Opens hosted sign-in in the shared browser session so "Continue as …" works.
    @MainActor
    func ringSignIn(_ attempt: SignInAttempt) async throws -> URL {
        do {
            if #available(iOS 17.4, macOS 14.4, *) {
                return try await authenticate(
                    using: attempt.authorizeURL,
                    callback: .customScheme(AppInfo.callbackScheme),
                    preferredBrowserSession: .shared,
                    additionalHeaderFields: [:])
            } else {
                return try await authenticate(
                    using: attempt.authorizeURL,
                    callbackURLScheme: AppInfo.callbackScheme,
                    preferredBrowserSession: .shared)
            }
        } catch let error as ASWebAuthenticationSessionError where error.code == .canceledLogin {
            throw RingError.cancelled
        }
    }
}
