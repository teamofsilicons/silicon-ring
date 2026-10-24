#if DEBUG && os(iOS)
import Foundation

/// Answers demo mode's API requests from `DemoFixtures`, inside the app. The origin does not exist,
/// so nothing leaves the device and the realtime socket never connects: the app shows exactly what
/// these answers say. Anything the demo has no answer for gets a 404.
final class DemoServer: URLProtocol {
    static let origin = URL(string: "https://demo.ring.invalid")!

    /// Set once, before the class is registered.
    nonisolated(unsafe) static var fixtures: DemoFixtures?

    override class func canInit(with request: URLRequest) -> Bool {
        guard let url = request.url, url.scheme == "https" || url.scheme == "http" else { return false }
        return url.host == origin.host
    }

    override class func canonicalRequest(for request: URLRequest) -> URLRequest { request }

    override func startLoading() {
        guard let url = request.url else { return }
        let (status, body) = Self.answer(request)
        let response = HTTPURLResponse(url: url, statusCode: status, httpVersion: "HTTP/1.1",
                                       headerFields: ["Content-Type": "application/json"])!
        client?.urlProtocol(self, didReceive: response, cacheStoragePolicy: .notAllowed)
        client?.urlProtocol(self, didLoad: body)
        client?.urlProtocolDidFinishLoading(self)
    }

    override func stopLoading() {}

    private static func answer(_ request: URLRequest) -> (Int, Data) {
        guard let fixtures, let url = request.url else { return notFound() }
        let parts = url.path.split(separator: "/").map(String.init)
        guard parts.first == "v1", parts.count <= 4 else { return notFound() }
        func part(_ index: Int) -> String? { index < parts.count ? parts[index] : nil }
        let query = URLComponents(url: url, resolvingAgainstBaseURL: false)?.queryItems ?? []
        func value(_ name: String) -> String? { query.first { $0.name == name }?.value }

        switch (request.httpMethod ?? "GET", part(1), part(2), part(3)) {
        case ("GET", "meta"?, nil, nil):
            return ok(fixtures.meta)
        case ("GET", "me"?, nil, nil):
            return ok(Me(account: fixtures.me, device: fixtures.device))
        case ("PUT", "devices"?, "current"?, nil), ("PATCH", "devices"?, _?, nil):
            return ok(fixtures.device)
        case ("GET", "devices"?, nil, nil):
            return ok(ItemsBody(items: [fixtures.device]))
        case ("GET", "calls"?, nil, nil):
            guard let states = value("state") else { return ok(PageBody(items: fixtures.calls)) }
            let wanted = Set(states.split(separator: ",").map(String.init))
            return ok(PageBody(items: fixtures.calls.filter { wanted.contains($0.state.rawValue) }))
        case ("GET", "calls"?, let ringid?, nil):
            guard let call = fixtures.calls.first(where: { $0.ringid == ringid }) else { return notFound() }
            return ok(call)
        case ("GET", "calls"?, _?, "transcript"?):
            return ok(TranscriptBody())
        case ("GET", "voicemails"?, nil, nil):
            return ok(PageBody(items: fixtures.voicemails))
        case ("GET", "voicemails"?, let id?, nil), ("PATCH", "voicemails"?, let id?, nil):
            guard var voicemail = fixtures.voicemails.first(where: { $0.voicemailID == id }) else { return notFound() }
            if request.httpMethod == "PATCH" { voicemail.read = true }
            return ok(voicemail)
        case ("GET", "people"?, "recent"?, nil):
            // Like ring-server: a case-insensitive match on the id or the display name.
            let needle = value("q")?.trimmingCharacters(in: .whitespaces).lowercased() ?? ""
            let people = fixtures.people.filter { person in
                needle.isEmpty || person.account.id.contains(needle) || person.account.label.lowercased().contains(needle)
            }
            return ok(ItemsBody(items: people))
        case ("GET", "accounts"?, let ref?, nil):
            let known = [fixtures.me] + fixtures.people.map(\.account)
            guard let account = known.first(where: { $0.id == ref.lowercased() || $0.uuid == ref.lowercased() }) else {
                return notFound(code: "account_not_found")
            }
            return ok(AccountLookup(account: account, onRing: true))
        case ("GET", "blocks"?, nil, nil):
            return ok(ItemsBody<Account>(items: []))
        case ("GET", "settings"?, nil, nil):
            return ok(fixtures.settings)
        case ("GET", "voices"?, nil, nil):
            return ok(VoiceList(items: fixtures.meta.voices, default: fixtures.meta.defaultVoice))
        default:
            return notFound()
        }
    }

    private static func ok<T: Encodable>(_ value: T) -> (Int, Data) {
        guard let data = try? RingJSON.encoder().encode(value) else { return notFound() }
        return (200, data)
    }

    private static func notFound(code: String = "not_found") -> (Int, Data) {
        let body = ["error": APIErrorInfo(code: code, message: "That isn't part of the demo.")]
        return (404, (try? RingJSON.encoder().encode(body)) ?? Data())
    }
}

private struct ItemsBody<Item: Encodable>: Encodable {
    var items: [Item]
}

private struct PageBody<Item: Encodable>: Encodable {
    var items: [Item]
    var nextCursor: String?

    enum CodingKeys: String, CodingKey {
        case items
        case nextCursor = "next_cursor"
    }
}

private struct TranscriptBody: Encodable {
    var items: [String] = []
    var nextAfterSeq = 0

    enum CodingKeys: String, CodingKey {
        case items
        case nextAfterSeq = "next_after_seq"
    }
}
#endif
