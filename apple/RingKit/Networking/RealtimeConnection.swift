import Foundation
import Network

/// The realtime WebSocket (`/v1/ws`): keepalive pings, reconnect with backoff, and a liveness check.
/// Callers reconcile with the REST API whenever `ready` arrives.
@MainActor
final class RealtimeConnection {
    enum Status: Equatable {
        case idle
        case connecting
        case connected
        case waiting
    }

    var onEvent: ((RealtimeEvent) -> Void)?
    var onStatus: ((Status) -> Void)?
    var onUnauthorized: (() -> Void)?

    private(set) var status: Status = .idle {
        didSet { if status != oldValue { onStatus?(status) } }
    }

    private var credentials: (server: URL, token: String)?
    private var socket: URLSessionWebSocketTask?
    private var loop: Task<Void, Never>?
    private var failures = 0
    private var lastReceived = Date()
    private let pathMonitor = NWPathMonitor()
    private var pathSatisfied = true

    private static let pingInterval: Duration = .seconds(25)
    private static let staleAfter: TimeInterval = 70

    init() {
        pathMonitor.pathUpdateHandler = { [weak self] path in
            let satisfied = path.status == .satisfied
            Task { @MainActor in self?.networkChanged(satisfied: satisfied) }
        }
        pathMonitor.start(queue: DispatchQueue(label: "com.teamofsilicons.ring.path"))
    }

    func start(server: URL, token: String) {
        if let credentials, credentials.server == server, credentials.token == token, status != .idle { return }
        credentials = (server, token)
        failures = 0
        restart()
    }

    func stop() {
        credentials = nil
        loop?.cancel()
        loop = nil
        socket?.cancel(with: .goingAway, reason: nil)
        socket = nil
        status = .idle
    }

    /// Drops any half-dead socket and connects again right away (wake, foreground, network change).
    func reconnectNow() {
        guard credentials != nil else { return }
        failures = 0
        restart()
    }

    /// Reconnects only if the socket is not already healthy.
    func ensureConnected() {
        guard credentials != nil else { return }
        if status == .connected && Date().timeIntervalSince(lastReceived) < Self.staleAfter { return }
        reconnectNow()
    }

    private func networkChanged(satisfied: Bool) {
        let wasSatisfied = pathSatisfied
        pathSatisfied = satisfied
        if satisfied && (!wasSatisfied || status != .connected) { reconnectNow() }
    }

    private func restart() {
        loop?.cancel()
        socket?.cancel(with: .goingAway, reason: nil)
        socket = nil
        loop = Task { [weak self] in await self?.run() }
    }

    private func run() async {
        while !Task.isCancelled, let credentials {
            status = .connecting
            let connected = await connectOnce(server: credentials.server, token: credentials.token)
            if Task.isCancelled || self.credentials == nil { return }
            failures = connected ? 1 : failures + 1
            status = .waiting
            let base = min(30.0, pow(2.0, Double(max(0, failures - 1))))
            let delay = base * Double.random(in: 0.8...1.2)
            try? await Task.sleep(for: .seconds(delay))
        }
    }

    /// Runs one socket until it closes. Returns whether it ever got the `ready` frame.
    private func connectOnce(server: URL, token: String) async -> Bool {
        guard let url = ServerConfig.webSocketURL(for: server) else { return false }
        var request = URLRequest(url: url, timeoutInterval: 20)
        request.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization")
        request.setValue(AppInfo.userAgent, forHTTPHeaderField: "User-Agent")
        let socket = URLSession.shared.webSocketTask(with: request)
        self.socket = socket
        socket.resume()
        lastReceived = Date()

        var gotReady = false
        let keepalive = Task { [weak self] in
            while !Task.isCancelled {
                try? await Task.sleep(for: Self.pingInterval)
                guard let self, !Task.isCancelled else { return }
                if Date().timeIntervalSince(self.lastReceived) > Self.staleAfter {
                    socket.cancel(with: .goingAway, reason: nil)
                    return
                }
                try? await socket.send(.string(#"{"type":"ping"}"#))
            }
        }
        defer { keepalive.cancel() }

        while !Task.isCancelled {
            let message: URLSessionWebSocketTask.Message
            do {
                message = try await socket.receive()
            } catch {
                // A refused handshake, or ring-server's 4401 close when this session is revoked.
                let refused = (socket.response as? HTTPURLResponse).map { $0.statusCode == 401 || $0.statusCode == 403 } ?? false
                if refused || socket.closeCode.rawValue == 4401 {
                    onUnauthorized?()
                    credentials = nil
                    status = .idle
                    return gotReady
                }
                break
            }
            lastReceived = Date()
            let data: Data
            switch message {
            case .string(let text): data = Data(text.utf8)
            case .data(let bytes): data = bytes
            @unknown default: continue
            }
            guard let event = try? RealtimeEvent.decode(data) else { continue }
            if case .ready = event {
                gotReady = true
                failures = 0
                status = .connected
            }
            if case .sessionRevoked = event {
                onEvent?(event)
                socket.cancel(with: .normalClosure, reason: nil)
                credentials = nil
                status = .idle
                return gotReady
            }
            onEvent?(event)
        }
        socket.cancel(with: .goingAway, reason: nil)
        return gotReady
    }
}
