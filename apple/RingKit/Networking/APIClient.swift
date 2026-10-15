import Foundation

/// Every failure the app can see. API errors keep the server's code, message and hint.
enum RingError: LocalizedError, Sendable {
    case api(status: Int, info: APIErrorInfo)
    case network(String)
    case protocolError(String)
    case notSignedIn
    case cancelled

    var errorDescription: String? {
        switch self {
        case .api(_, let info): info.message
        case .network: "Ring can't reach the server. Check your connection."
        case .protocolError: "Ring got an answer it didn't understand."
        case .notSignedIn: "You're signed out."
        case .cancelled: "Cancelled."
        }
    }

    var recoverySuggestion: String? {
        if case .api(_, let info) = self { return info.hint }
        return nil
    }

    var code: String {
        switch self {
        case .api(_, let info): info.code
        case .network: "network"
        case .protocolError: "protocol"
        case .notSignedIn: "not_signed_in"
        case .cancelled: "cancelled"
        }
    }

    var status: Int? {
        if case .api(let status, _) = self { return status }
        return nil
    }

    /// The session is gone and the app should return to sign-in.
    var isSessionInvalid: Bool {
        status == 401 || code == "unauthenticated" || code == "session_expired"
    }
}

/// Typed access to every endpoint of udd/api.md that a carbon app uses.
struct APIClient: Sendable {
    let baseURL: URL
    let token: String?
    var session: URLSession = .shared

    init(baseURL: URL, token: String?) {
        self.baseURL = baseURL
        self.token = token
    }

    // MARK: Meta and sessions

    func meta() async throws -> Meta {
        try await send("GET", "/v1/meta", authenticated: false)
    }

    func signInCarbon(_ login: CarbonLogin) async throws -> SessionGrant {
        try await send("POST", "/v1/auth/carbon", body: login, authenticated: false)
    }

    func me() async throws -> Me {
        try await send("GET", "/v1/me")
    }

    func signOut() async throws {
        let _: EmptyResponse = try await send("DELETE", "/v1/session")
    }

    // MARK: Devices

    @discardableResult
    func updateCurrentDevice(_ update: DeviceUpdate) async throws -> Device {
        try await send("PUT", "/v1/devices/current", body: update)
    }

    func devices() async throws -> [Device] {
        let items: Items<Device> = try await send("GET", "/v1/devices")
        return items.items
    }

    func patchDevice(_ id: String, _ patch: DevicePatch) async throws -> Device {
        try await send("PATCH", "/v1/devices/\(escape(id))", body: patch)
    }

    func deleteDevice(_ id: String) async throws {
        let _: EmptyResponse = try await send("DELETE", "/v1/devices/\(escape(id))")
    }

    // MARK: People

    func lookup(_ ref: String) async throws -> AccountLookup {
        try await send("GET", "/v1/accounts/\(escape(ref))")
    }

    func recentPeople(query: String = "", limit: Int = 20) async throws -> [RecentPerson] {
        var items = [URLQueryItem(name: "limit", value: String(limit))]
        if !query.isEmpty { items.append(URLQueryItem(name: "q", value: query)) }
        let page: Items<RecentPerson> = try await send("GET", "/v1/people/recent", query: items)
        return page.items
    }

    // MARK: Calls

    func createCall(to ref: String, idempotencyKey: String = UUID().uuidString) async throws -> CreatedCall {
        try await send("POST", "/v1/calls", body: CreateCall(to: ref), idempotencyKey: idempotencyKey)
    }

    func calls(states: [CallState] = [], limit: Int = 50, cursor: String? = nil) async throws -> Page<Call> {
        var items = [URLQueryItem(name: "limit", value: String(limit))]
        if !states.isEmpty { items.append(URLQueryItem(name: "state", value: states.map(\.rawValue).joined(separator: ","))) }
        if let cursor { items.append(URLQueryItem(name: "cursor", value: cursor)) }
        return try await send("GET", "/v1/calls", query: items)
    }

    func call(_ ringid: String) async throws -> Call {
        try await send("GET", "/v1/calls/\(escape(ringid))")
    }

    func accept(_ ringid: String, invitationID: String?) async throws -> CallWithMedia {
        try await send("POST", "/v1/calls/\(escape(ringid))/accept", body: AcceptCall(invitationID: invitationID))
    }

    @discardableResult
    func decline(_ ringid: String, invitationID: String?, reason: String?) async throws -> Call {
        let reason = reason?.trimmingCharacters(in: .whitespacesAndNewlines)
        let body = DeclineCall(invitationID: invitationID, reason: reason?.isEmpty == false ? reason : nil, giveNoReason: false)
        let envelope: CallEnvelope = try await send("POST", "/v1/calls/\(escape(ringid))/decline", body: body)
        return envelope.call
    }

    @discardableResult
    func silence(_ ringid: String, invitationID: String?) async throws -> Call {
        let envelope: CallEnvelope = try await send("POST", "/v1/calls/\(escape(ringid))/silence", body: SilenceCall(invitationID: invitationID))
        return envelope.call
    }

    @discardableResult
    func leave(_ ringid: String) async throws -> Call {
        let envelope: CallEnvelope = try await send("POST", "/v1/calls/\(escape(ringid))/leave", body: EmptyBody())
        return envelope.call
    }

    func invite(_ ringid: String, to ref: String, idempotencyKey: String = UUID().uuidString) async throws -> InvitedCall {
        try await send("POST", "/v1/calls/\(escape(ringid))/invite", body: InviteToCall(to: ref), idempotencyKey: idempotencyKey)
    }

    func media(_ ringid: String) async throws -> CallWithMedia {
        try await send("POST", "/v1/calls/\(escape(ringid))/media", body: EmptyBody())
    }

    func transcript(_ ringid: String, afterSeq: Int = 0, limit: Int = 200) async throws -> TranscriptPage {
        try await send("GET", "/v1/calls/\(escape(ringid))/transcript", query: [
            URLQueryItem(name: "after_seq", value: String(afterSeq)),
            URLQueryItem(name: "limit", value: String(limit)),
        ])
    }

    /// The whole transcript, following `next_after_seq` until a short page.
    func fullTranscript(_ ringid: String) async throws -> [TranscriptEntry] {
        var entries: [TranscriptEntry] = []
        var after = 0
        while true {
            let page = try await transcript(ringid, afterSeq: after, limit: 200)
            entries.append(contentsOf: page.items)
            if page.items.count < 200 || page.nextAfterSeq <= after { return entries }
            after = page.nextAfterSeq
        }
    }

    func recording(_ ringid: String) async throws -> Recording {
        try await send("GET", "/v1/calls/\(escape(ringid))/recording")
    }

    // MARK: Voicemail

    func voicemails(limit: Int = 50, cursor: String? = nil) async throws -> Page<Voicemail> {
        var items = [URLQueryItem(name: "limit", value: String(limit))]
        if let cursor { items.append(URLQueryItem(name: "cursor", value: cursor)) }
        return try await send("GET", "/v1/voicemails", query: items)
    }

    func voicemail(_ id: String) async throws -> Voicemail {
        try await send("GET", "/v1/voicemails/\(escape(id))")
    }

    func markVoicemailRead(_ id: String) async throws -> Voicemail {
        try await send("PATCH", "/v1/voicemails/\(escape(id))", body: VoicemailReadPatch(read: true))
    }

    func deleteVoicemail(_ id: String) async throws {
        let _: EmptyResponse = try await send("DELETE", "/v1/voicemails/\(escape(id))")
    }

    // MARK: Settings

    func settings() async throws -> Settings {
        try await send("GET", "/v1/settings")
    }

    func patchSettings(_ patch: JSONValue) async throws -> Settings {
        try await send("PATCH", "/v1/settings", body: patch)
    }

    func voices() async throws -> VoiceList {
        try await send("GET", "/v1/voices")
    }

    // MARK: Transport

    private func escape(_ segment: String) -> String {
        var allowed = CharacterSet.urlPathAllowed
        allowed.remove(charactersIn: "/?#")
        return segment.addingPercentEncoding(withAllowedCharacters: allowed) ?? segment
    }

    private func send<Response: Decodable>(
        _ method: String,
        _ path: String,
        query: [URLQueryItem] = [],
        authenticated: Bool = true,
        idempotencyKey: String? = nil
    ) async throws -> Response {
        try await perform(method, path, query: query, bodyData: nil, authenticated: authenticated, idempotencyKey: idempotencyKey)
    }

    private func send<Response: Decodable, Body: Encodable>(
        _ method: String,
        _ path: String,
        query: [URLQueryItem] = [],
        body: Body,
        authenticated: Bool = true,
        idempotencyKey: String? = nil
    ) async throws -> Response {
        let data = try RingJSON.encoder().encode(body)
        return try await perform(method, path, query: query, bodyData: data, authenticated: authenticated, idempotencyKey: idempotencyKey)
    }

    private func perform<Response: Decodable>(
        _ method: String,
        _ path: String,
        query: [URLQueryItem],
        bodyData: Data?,
        authenticated: Bool,
        idempotencyKey: String?
    ) async throws -> Response {
        guard var components = URLComponents(url: baseURL, resolvingAgainstBaseURL: false) else {
            throw RingError.protocolError("Invalid server URL")
        }
        components.percentEncodedPath = components.percentEncodedPath.trimmingSuffix("/") + path
        if !query.isEmpty { components.queryItems = query }
        guard let url = components.url else { throw RingError.protocolError("Invalid request URL") }

        var request = URLRequest(url: url, timeoutInterval: 30)
        request.httpMethod = method
        request.setValue("application/json", forHTTPHeaderField: "Accept")
        request.setValue(AppInfo.userAgent, forHTTPHeaderField: "User-Agent")
        if authenticated {
            guard let token else { throw RingError.notSignedIn }
            request.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization")
        }
        if let idempotencyKey { request.setValue(idempotencyKey, forHTTPHeaderField: "Idempotency-Key") }
        if let bodyData {
            request.httpBody = bodyData
            request.setValue("application/json", forHTTPHeaderField: "Content-Type")
        }

        let data: Data
        let response: URLResponse
        do {
            (data, response) = try await session.data(for: request)
        } catch let error as URLError where error.code == .cancelled {
            throw RingError.cancelled
        } catch is CancellationError {
            throw RingError.cancelled
        } catch {
            throw RingError.network(error.localizedDescription)
        }
        guard let http = response as? HTTPURLResponse else { throw RingError.protocolError("Not an HTTP response") }

        guard (200..<300).contains(http.statusCode) else {
            if let body = try? RingJSON.decoder().decode(APIErrorBody.self, from: data) {
                throw RingError.api(status: http.statusCode, info: body.error)
            }
            throw RingError.api(status: http.statusCode, info: APIErrorInfo(
                code: "http_\(http.statusCode)",
                message: HTTPURLResponse.localizedString(forStatusCode: http.statusCode).capitalized,
                retryable: http.statusCode >= 500))
        }
        if Response.self == EmptyResponse.self {
            return EmptyResponse() as! Response
        }
        do {
            return try RingJSON.decoder().decode(Response.self, from: data)
        } catch {
            throw RingError.protocolError(String(describing: error))
        }
    }
}

private extension String {
    func trimmingSuffix(_ suffix: String) -> String {
        hasSuffix(suffix) ? String(dropLast(suffix.count)) : self
    }
}
