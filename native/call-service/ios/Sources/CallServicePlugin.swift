import AVFoundation
import CallKit
import Foundation
import PushKit
import Security
import Tauri
import WebKit

private typealias Reply = (Result<[String: Any], Error>) -> Void
private func failure(_ message: String) -> Error { NSError(domain: "SiliconRing", code: 1, userInfo: [NSLocalizedDescriptionKey: message]) }
private func environmentRealm(_ value: String) throws -> String {
    let realm = value.trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
    guard ["production", "test"].contains(realm) || UUID(uuidString: realm)?.uuidString.lowercased() == realm && realm != "00000000-0000-0000-0000-000000000000" else { throw failure("Choose production, legacy test, or a valid Honeycomb environment UUID.") }
    return realm
}
private func acknowledgesRealm(_ realm: String, _ result: [String: Any]) -> Bool { result["realm"] as? String == realm || result["realm"] == nil && ["production", "test"].contains(realm) }
private func audioLog(_ message: String) {
    #if DEBUG
    NSLog("RingAudio %@", message)
    #endif
}
private func saveSession(_ data: Data?) {
    let query: [String: Any] = [kSecClass as String: kSecClassGenericPassword, kSecAttrService as String: "com.teamofsilicons.ring.session", kSecAttrAccount as String: "ring"]
    SecItemDelete(query as CFDictionary)
    if let data = data {
        var item = query; item[kSecValueData as String] = data
        item[kSecAttrAccessible as String] = kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly
        SecItemAdd(item as CFDictionary, nil)
    }
}
private func savedSession() -> [String: Any]? {
    let query: [String: Any] = [kSecClass as String: kSecClassGenericPassword, kSecAttrService as String: "com.teamofsilicons.ring.session", kSecAttrAccount as String: "ring", kSecReturnData as String: true]
    var result: CFTypeRef?
    guard SecItemCopyMatching(query as CFDictionary, &result) == errSecSuccess, let data = result as? Data else { return nil }
    return (try? JSONSerialization.jsonObject(with: data)) as? [String: Any]
}

private final class RingTransport: NSObject {
    var credentials: [String: Any] = [:]
    var event: ((String, [String: Any]) -> Void)?
    var ready = false
    private var socket: URLSessionWebSocketTask?
    private var pending: [String: Reply] = [:]
    private var waiters: [(Error?) -> Void] = []
    private var connecting = false
    private var enabled = false
    var actor: String { credentials["actor"] as? String ?? "" }
    var device: String { credentials["device_id"] as? String ?? "" }

    func configure(_ values: [String: Any]) throws {
        var values = values
        guard let raw = values["url"] as? String, let url = URL(string: raw), ["wss", "ws"].contains(url.scheme ?? ""), url.user == nil, url.password == nil,
              values["session_token"] is String, values["device_id"] is String, values["actor"] is String else { throw failure("Native calling needs a valid Ring endpoint and authenticated device session.") }
        guard url.scheme == "wss" || ["localhost", "127.0.0.1", "::1", "[::1]"].contains(url.host ?? "") else { throw failure("Use wss:// for remote Ring servers.") }
        guard values["realm"] == nil || values["realm"] is String else { throw failure("Choose production, legacy test, or a valid Honeycomb environment UUID.") }
        let realm = try environmentRealm(values["realm"] as? String ?? "production")
        guard realm == "production" || !(values["test_app_secret"] as? String ?? "").trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else { throw failure("A test app secret is required for test environments.") }
        values["realm"] = realm
        if ["session_token", "device_id", "url", "realm", "org_id", "test_app_secret"].contains(where: { credentials[$0] as? String != values[$0] as? String }) { disconnect(clear: false) }
        credentials = values; enabled = true
        saveSession(try JSONSerialization.data(withJSONObject: values))
        connect { _ in }
    }
    func connect(_ completion: @escaping (Error?) -> Void) {
        if ready { completion(nil); return }
        waiters.append(completion)
        if connecting { return }
        guard enabled, let raw = credentials["url"] as? String, let url = URL(string: raw) else { finishConnect(failure("Sign in to Ring first.")); return }
        connecting = true
        let task = URLSession.shared.webSocketTask(with: url); socket = task; task.resume(); receive(task)
        let realm = credentials["realm"] as? String ?? "production"
        var hello: [String: Any] = ["versions": [1], "client": ["name": "ring-ios", "version": "0.1.5"], "realm": realm, "org_id": credentials["org_id"] ?? ""]
        if realm != "production" { hello["test_app_secret"] = credentials["test_app_secret"] }
        rawRequest("protocol.hello", hello) { result in
            switch result {
            case .failure(let error): self.finishConnect(error)
            case .success(let hello):
                guard acknowledgesRealm(realm, hello) else { self.finishConnect(failure("The server did not confirm the selected environment.")); return }
                self.rawRequest("auth.resume", ["session_token": self.credentials["session_token"] ?? "", "device_id": self.device]) { result in
                    switch result {
                    case .failure(let error): self.finishConnect(error)
                    case .success(let session):
                        guard acknowledgesRealm(realm, session) else { self.finishConnect(failure("The session does not belong to the selected environment.")); return }
                        self.ready = true
                        self.rawRequest("events.subscribe", [:]) { _ in }
                        self.finishConnect(nil)
                        self.event?("connection.restored", [:])
                    }
                }
            }
        }
    }
    private func finishConnect(_ error: Error?) {
        connecting = false
        let callbacks = waiters; waiters.removeAll(); callbacks.forEach { $0(error) }
        if let error = error { event?("error", ["message": error.localizedDescription]); socket?.cancel(with: .goingAway, reason: nil); socket = nil; ready = false }
    }
    func request(_ method: String, _ params: [String: Any] = [:], _ reply: @escaping Reply) {
        connect { error in if let error = error { reply(.failure(error)) } else { self.rawRequest(method, params, reply) } }
    }
    private func rawRequest(_ method: String, _ params: [String: Any], _ reply: @escaping Reply) {
        let id = UUID().uuidString
        pending[id] = reply; send(["id": id, "method": method, "params": params])
        DispatchQueue.main.asyncAfter(deadline: .now() + 15) { if let callback = self.pending.removeValue(forKey: id) { callback(.failure(failure("\(method) timed out. Check call state before retrying."))) } }
    }
    func frame(_ type: String, _ data: [String: Any]) { if ready { send(["type": type, "data": data]) } }
    private func send(_ value: [String: Any]) {
        guard let bytes = try? JSONSerialization.data(withJSONObject: value), let text = String(data: bytes, encoding: .utf8), let socket = socket else { return }
        socket.send(.string(text)) { error in if let error = error { DispatchQueue.main.async { self.event?("error", ["message": error.localizedDescription]) } } }
    }
    private func receive(_ task: URLSessionWebSocketTask) {
        task.receive { result in DispatchQueue.main.async {
            guard self.socket === task else { return }
            switch result {
            case .success(let message):
                var bytes: Data?
                if case .string(let text) = message { bytes = text.data(using: .utf8) }
                else if case .data(let data) = message { bytes = data }
                if let bytes = bytes, let object = (try? JSONSerialization.jsonObject(with: bytes)) as? [String: Any] {
                    if let id = object["id"] as? String, let callback = self.pending.removeValue(forKey: id) {
                        if object["ok"] as? Bool == true { callback(.success(object["result"] as? [String: Any] ?? [:])) }
                        else { callback(.failure(failure((object["error"] as? [String: Any])?["message"] as? String ?? "Ring rejected the request."))) }
                    } else if let type = object["type"] as? String { self.event?(type, object["data"] as? [String: Any] ?? [:]) }
                }
                self.receive(task)
            case .failure(let error):
                self.socket = nil; self.ready = false; self.connecting = false
                let requests = self.pending; self.pending.removeAll(); requests.values.forEach { $0(.failure(error)) }
                self.event?("connection.lost", [:])
                if self.enabled { DispatchQueue.main.asyncAfter(deadline: .now() + 2) { self.connect { _ in } } }
            }
        } }
    }
    func disconnect(clear: Bool) {
        enabled = false; ready = false; connecting = false
        socket?.cancel(with: .normalClosure, reason: nil); socket = nil
        let requests = pending; pending.removeAll(); requests.values.forEach { $0(.failure(failure("Native calling disconnected."))) }
        if clear { credentials = [:]; saveSession(nil) }
    }
}

private final class NativeAudio {
    private let transport: RingTransport
    private var engine: AVAudioEngine?
    private var player: AVAudioPlayerNode?
    private var buffered = Data()
    private var sequence = 0
    private var offset = 0
    private var lastOutput = -1
    private var loggedCapture = false
    private var loggedOutput = false
    private var loggedNonzeroOutput = false
    private var queuedOutput = 0
    private var capturingSince = ProcessInfo.processInfo.systemUptime
    private var starting = false
    private var startingRing = ""
    private var startingVoicemailId: String?
    private var generation = 0
    private var callKitActive = false
    private var startWaiters: [Reply] = []
    private(set) var streamId = ""
    private(set) var ringid = ""
    private(set) var voicemail = false
    private(set) var muted = false
    init(_ transport: RingTransport) { self.transport = transport }
    func prepareCall() throws {
        let session = AVAudioSession.sharedInstance()
        // Configure before fulfilling the CallKit action; CallKit activates it.
        try session.setCategory(.playAndRecord, mode: .voiceChat, options: [.allowBluetooth, .defaultToSpeaker])
        try session.setPreferredSampleRate(24000); try session.setPreferredIOBufferDuration(0.02)
    }
    func requestPermission() {
        AVAudioSession.sharedInstance().requestRecordPermission { allowed in audioLog("microphone permission granted=\(allowed)") }
    }
    func start(_ ring: String, voicemailId: String?, reply: @escaping Reply) {
        if ringid == ring && !streamId.isEmpty && voicemail == (voicemailId != nil) { reply(.success(["stream_id": streamId])); return }
        if starting {
            guard startingRing == ring && startingVoicemailId == voicemailId else { reply(.failure(failure("Another audio attachment is still starting."))); return }
            startWaiters.append(reply); return
        }
        generation += 1; let attempt = generation
        starting = true; startingRing = ring; startingVoicemailId = voicemailId; startWaiters.append(reply)
        let finished: Reply = { result in
            guard self.generation == attempt else { return }
            self.starting = false; self.startingRing = ""; self.startingVoicemailId = nil
            let callbacks = self.startWaiters; self.startWaiters.removeAll(); callbacks.forEach { $0(result) }
        }
        AVAudioSession.sharedInstance().requestRecordPermission { allowed in DispatchQueue.main.async {
            audioLog("start microphone permission granted=\(allowed)")
            guard self.generation == attempt else { return }
            guard allowed else { finished(.failure(failure("Allow microphone access in iOS Settings to join audio."))); return }
            self.stop({ _ in
                guard self.generation == attempt else { return }
                var params: [String: Any] = ["ringid": ring, "device_id": self.transport.device, "purpose": voicemailId == nil ? "call" : "voicemail"]
                if let id = voicemailId { params["voicemail_id"] = id }
                self.transport.request("media.attach", params) { result in
                    guard self.generation == attempt else {
                        if case .success(let response) = result, let id = response["stream_id"] as? String { self.transport.request("media.detach", ["stream_id": id]) { _ in } }
                        return
                    }
                    do {
                        let response = try result.get()
                        guard let id = response["stream_id"] as? String else { throw failure("Media attachment did not return a stream.") }
                        self.streamId = id; self.ringid = ring; self.voicemail = voicemailId != nil; self.sequence = 0; self.offset = 0; self.muted = false; self.lastOutput = -1; self.loggedCapture = false; self.loggedOutput = false; self.loggedNonzeroOutput = false; self.capturingSince = ProcessInfo.processInfo.systemUptime
                        audioLog("media attached; CallKit active=\(self.callKitActive)")
                        try self.openAudio(); finished(.success(response))
                    } catch { audioLog("audio start failed: \(error.localizedDescription)"); self.stop({ _ in }, cancelStart: false); finished(.failure(error)) }
                }
            }, cancelStart: false)
        } }
    }
    private func openAudio() throws {
        // CallKit owns activation for calls, including lock-screen answers.
        // Private voicemail recordings activate their own audio session.
        if !voicemail && !callKitActive { return }
        let session = AVAudioSession.sharedInstance()
        try prepareCall()
        if voicemail { try session.setActive(true) }
        let engine = AVAudioEngine(), player = AVAudioPlayerNode()
        self.engine = engine; self.player = player
        let output = AVAudioFormat(standardFormatWithSampleRate: 24000, channels: 1)!
        engine.attach(player); engine.connect(player, to: engine.mainMixerNode, format: output)
        let input = engine.inputNode, source = input.outputFormat(forBus: 0)
        guard let format = AVAudioFormat(commonFormat: .pcmFormatInt16, sampleRate: 24000, channels: 1, interleaved: true), let converter = AVAudioConverter(from: source, to: format) else { throw failure("The device microphone cannot supply Ring audio.") }
        let expectedStream = streamId
        input.installTap(onBus: 0, bufferSize: 1024, format: source) { inputBuffer, _ in
            let capacity = AVAudioFrameCount(ceil(Double(inputBuffer.frameLength) * 24000 / source.sampleRate) + 1)
            guard let converted = AVAudioPCMBuffer(pcmFormat: format, frameCapacity: capacity) else { return }
            var supplied = false; var conversionError: NSError?
            converter.convert(to: converted, error: &conversionError) { _, state in
                if supplied { state.pointee = .noDataNow; return nil }
                supplied = true; state.pointee = .haveData; return inputBuffer
            }
            guard conversionError == nil, let samples = converted.int16ChannelData?[0] else { return }
            let bytes = Data(bytes: samples, count: Int(converted.frameLength) * 2)
            DispatchQueue.main.async { if self.streamId == expectedStream { self.capture(bytes) } }
        }
        try engine.start(); player.play(); audioLog("engine started; input sample rate=\(source.sampleRate), channels=\(source.channelCount)")
    }
    private func capture(_ bytes: Data) {
        guard !streamId.isEmpty, !muted, transport.ready else { return }
        if !loggedCapture && !bytes.isEmpty { loggedCapture = true; audioLog("microphone PCM received") }
        buffered.append(bytes)
        while buffered.count >= 960 {
            let frame = buffered.prefix(960); buffered.removeFirst(960)
            offset = max(offset, Int((ProcessInfo.processInfo.systemUptime - capturingSince) * 1000))
            transport.frame("media.audio", ["stream_id": streamId, "seq": sequence, "offset_ms": offset, "audio_base64": frame.base64EncodedString()])
            let rms: Double = frame.withUnsafeBytes { raw in
                let samples = raw.bindMemory(to: Int16.self); return sqrt(samples.reduce(0.0) { $0 + pow(Double($1) / 32768, 2) } / 480)
            }
            if rms > 0.018 { transport.frame("media.speech", ["stream_id": streamId, "seq": sequence, "start_ms": offset, "end_ms": offset + 20, "confidence": min(1, rms * 10)]) }
            sequence += 1; offset += 20
        }
    }
    func output(_ data: [String: Any]) {
        guard data["stream_id"] as? String == streamId, let encoded = data["audio_base64"] as? String, let bytes = Data(base64Encoded: encoded), bytes.count == 960,
              let seq = data["seq"] as? Int, seq > lastOutput, let player = player else { return }
        lastOutput = seq
        if !loggedOutput { loggedOutput = true; audioLog("remote PCM received") }
        let format = AVAudioFormat(standardFormatWithSampleRate: 24000, channels: 1)!
        guard let buffer = AVAudioPCMBuffer(pcmFormat: format, frameCapacity: 480), let samples = buffer.floatChannelData?[0] else { return }
        buffer.frameLength = 480
        var peak = 0
        for index in 0..<480 { let value = Int16(bitPattern: UInt16(bytes[index * 2]) | UInt16(bytes[index * 2 + 1]) << 8); samples[index] = Float(value) / 32768; peak = max(peak, abs(Int(value))) }
        if peak > 50 && !loggedNonzeroOutput {
            loggedNonzeroOutput = true
            let session = AVAudioSession.sharedInstance()
            audioLog("nonzero remote PCM peak=\(peak); playing=\(player.isPlaying); engine=\(engine?.isRunning ?? false); playerVolume=\(player.volume); mixerVolume=\(engine?.mainMixerNode.outputVolume ?? 0); systemVolume=\(session.outputVolume); outputPorts=\(session.currentRoute.outputs.map { $0.portType.rawValue }.joined(separator: ","))")
        }
        if queuedOutput >= 15 { player.stop(); queuedOutput = 0; player.play() }
        queuedOutput += 1
        player.scheduleBuffer(buffer) { DispatchQueue.main.async { self.queuedOutput = max(0, self.queuedOutput - 1) } }
    }
    func mute(_ value: Bool, reply: @escaping Reply) {
        guard !streamId.isEmpty else { reply(.failure(failure("No native microphone is attached."))); return }
        transport.request("media.state", ["stream_id": streamId, "muted": value]) { result in
            if case .success = result { self.muted = value; self.buffered.removeAll() }
            reply(result)
        }
    }
    func activate() { callKitActive = true; audioLog("CallKit audio activated"); if !streamId.isEmpty && engine == nil { do { try openAudio() } catch { audioLog("activation failed: \(error.localizedDescription)"); transport.event?("error", ["message": error.localizedDescription]) } } }
    func deactivate() {
        audioLog("CallKit audio deactivated")
        callKitActive = false
        engine?.inputNode.removeTap(onBus: 0); player?.stop(); engine?.stop(); engine = nil; player = nil; buffered.removeAll(); queuedOutput = 0
    }
    func stop(_ reply: @escaping Reply, cancelStart: Bool = true) {
        if cancelStart {
            generation += 1; starting = false; startingRing = ""; startingVoicemailId = nil
            let callbacks = startWaiters; startWaiters.removeAll(); callbacks.forEach { $0(.failure(failure("Audio attachment was cancelled."))) }
        }
        let id = streamId; let last = sequence - 1; let privateAudio = voicemail
        streamId = ""; ringid = ""; voicemail = false; engine?.inputNode.removeTap(onBus: 0); player?.stop(); engine?.stop(); engine = nil; player = nil; buffered.removeAll(); queuedOutput = 0
        if id.isEmpty || !transport.ready { reply(.success(["complete": id.isEmpty || !privateAudio])); return }
        var params: [String: Any] = ["stream_id": id]
        if privateAudio { params["last_seq"] = max(0, last) }
        transport.request("media.detach", params, reply)
    }
}

final class CallServicePlugin: Plugin, PKPushRegistryDelegate, CXProviderDelegate {
    private let transport = RingTransport()
    private lazy var audio = NativeAudio(transport)
    private let controller = CXCallController()
    private let provider: CXProvider
    private var registry: PKPushRegistry?
    private var calls: [String: UUID] = [:]
    private var pendingCalls = Set<String>()
    private var pushToken = ""
    override init() {
        let configuration = CXProviderConfiguration(localizedName: "Ring")
        configuration.supportsVideo = false
        configuration.maximumCallGroups = 20
        configuration.maximumCallsPerCallGroup = 1
        configuration.supportedHandleTypes = [.generic]
        provider = CXProvider(configuration: configuration)
        super.init()
        provider.setDelegate(self, queue: .main)
        transport.event = { [weak self] type, data in self?.received(type, data) }
        DispatchQueue.main.async {
            let registry = PKPushRegistry(queue: .main); registry.delegate = self; registry.desiredPushTypes = [.voIP]; self.registry = registry
            if let session = savedSession() { try? self.transport.configure(session) }
        }
    }
    @objc func configure(_ invoke: Invoke) throws {
        let payload = try JSONSerialization.jsonObject(with: Data(invoke.getRawArgs().utf8)) as? [String: Any] ?? [:]
        try transport.configure(payload)
        registerPush()
        // The first grant must happen in the foreground, before a locked-phone answer.
        DispatchQueue.main.async { self.audio.requestPermission() }
        invoke.resolve(["mobile": true, "native_audio": true, "platform": "ios", "push_configured": !pushToken.isEmpty])
    }
    @objc func control(_ invoke: Invoke) throws {
        let payload = try JSONSerialization.jsonObject(with: Data(invoke.getRawArgs().utf8)) as? [String: Any] ?? [:]
        let reply: Reply = { result in switch result { case .success(let value): invoke.resolve(value.mapValues { $0 as Any? }); case .failure(let error): invoke.reject(error.localizedDescription) } }
        switch payload["action"] as? String {
        case "restore": invoke.resolve((savedSession() ?? [:]).mapValues { $0 as Any? })
        case "start": guard let ring = payload["ringid"] as? String else { invoke.reject("A call ID is required."); return }; audio.start(ring, voicemailId: payload["voicemail_id"] as? String, reply: reply)
        case "stop": audio.stop(reply)
        case "mute": audio.mute(payload["muted"] as? Bool ?? false, reply: reply)
        case "logout": audio.stop { _ in }; transport.disconnect(clear: true); for uuid in calls.values { provider.reportCall(with: uuid, endedAt: Date(), reason: .remoteEnded) }; calls.removeAll(); invoke.resolve()
        default: invoke.resolve(["stream_id": audio.streamId, "muted": audio.muted, "mobile": true])
        }
    }
    private func received(_ type: String, _ data: [String: Any]) {
        if type == "media.audio" { audio.output(data); return }
        if type == "stream.error" { let error = data["error"] as? [String: Any] ?? data; audioLog("media frame rejected: \(error["code"] as? String ?? "unknown")"); return }
        if type == "connection.lost" { audio.stop { _ in }; return }
        if type == "connection.restored" { registerPush(); transport.request("calls.list", ["state": "active"]) { if case .success(let value) = $0 { for call in value["items"] as? [[String: Any]] ?? [] { self.reconcile(call) } } }; return }
        guard let ring = data["ringid"] as? String else { return }
        if type.hasPrefix("call.") || type.hasPrefix("participant.") {
            transport.request("calls.get", ["ringid": ring]) { if case .success(let call) = $0 { self.reconcile(call) } }
        }
    }
    private func reconcile(_ call: [String: Any]) {
        guard let ring = call["ringid"] as? String, let state = call["state"] as? String else { return }
        let offers = call["invitations"] as? [[String: Any]] ?? []
        if offers.contains(where: { $0["target"] as? String == transport.actor && $0["state"] as? String == "pending" && $0["silenced"] as? Bool != true }) {
            let caller = call["caller"] as? String ?? "Incoming call"
            reportIncoming(ring, caller: caller, name: caller, completion: {})
            return
        }
        if offers.contains(where: { $0["target"] as? String == transport.actor && $0["state"] as? String == "pending" && $0["silenced"] as? Bool == true }) {
            if let uuid = calls.removeValue(forKey: ring) { provider.reportCall(with: uuid, endedAt: Date(), reason: .unanswered) }; pendingCalls.remove(ring); return
        }
        pendingCalls.remove(ring)
        let participants = call["participants"] as? [[String: Any]] ?? []
        let here = participants.contains { $0["actor"] as? String == transport.actor && $0["device_id"] as? String == transport.device && ($0["left_at"] == nil || $0["left_at"] is NSNull) }
        if state == "active" && here {
            if calls[ring] == nil {
                let uuid = UUID(); calls[ring] = uuid
                controller.request(CXTransaction(action: CXStartCallAction(call: uuid, handle: CXHandle(type: .generic, value: call["target"] as? String ?? "Ring")))) { _ in }
            }
            if let uuid = calls[ring] { provider.reportOutgoingCall(with: uuid, connectedAt: Date()) }
            if !audio.voicemail { audio.start(ring, voicemailId: nil) { _ in } }
        } else if state == "ended" || !here && state != "ringing" {
            if audio.ringid == ring { audio.stop { _ in } }
            if let uuid = calls.removeValue(forKey: ring) { provider.reportCall(with: uuid, endedAt: Date(), reason: .remoteEnded) }
        }
    }
    private func reportIncoming(_ ring: String, caller: String, name: String, completion: @escaping () -> Void) {
        if calls[ring] != nil { completion(); return }
        let uuid = UUID(); calls[ring] = uuid; pendingCalls.insert(ring)
        let update = CXCallUpdate(); update.remoteHandle = CXHandle(type: .generic, value: caller); update.localizedCallerName = name; update.hasVideo = false; update.supportsGrouping = false
        provider.reportNewIncomingCall(with: uuid, update: update) { error in if error != nil { self.calls.removeValue(forKey: ring) }; completion() }
    }
    private func registerPush() {
        #if DEBUG
        let environment = "sandbox"
        #else
        let environment = "production"
        #endif
        if !pushToken.isEmpty && transport.ready { transport.request("devices.update", ["device_id": transport.device, "push_platform": "apns_voip", "push_token": pushToken, "push_environment": environment]) { _ in } }
    }
    func pushRegistry(_ registry: PKPushRegistry, didUpdate pushCredentials: PKPushCredentials, for type: PKPushType) { pushToken = pushCredentials.token.map { String(format: "%02x", $0) }.joined(); registerPush() }
    func pushRegistry(_ registry: PKPushRegistry, didInvalidatePushTokenFor type: PKPushType) { pushToken = ""; if transport.ready { transport.request("devices.update", ["device_id": transport.device, "push_token": NSNull(), "push_platform": NSNull()]) { _ in } } }
    func pushRegistry(_ registry: PKPushRegistry, didReceiveIncomingPushWith payload: PKPushPayload, for type: PKPushType, completion: @escaping () -> Void) {
        let data = payload.dictionaryPayload
        guard let ring = data["ringid"] as? String else { completion(); return }
        reportIncoming(ring, caller: data["caller"] as? String ?? "Ring", name: data["display_name"] as? String ?? "Incoming Ring call", completion: completion)
        transport.connect { error in
            guard error == nil else { return }
            self.transport.request("calls.get", ["ringid": ring]) { result in
                if case .success(let call) = result { self.reconcile(call) }
                else if let uuid = self.calls.removeValue(forKey: ring) { self.provider.reportCall(with: uuid, endedAt: Date(), reason: .remoteEnded); self.pendingCalls.remove(ring) }
            }
        }
    }
    func providerDidReset(_ provider: CXProvider) { audio.stop { _ in }; calls.removeAll(); pendingCalls.removeAll() }
    func provider(_ provider: CXProvider, perform action: CXAnswerCallAction) {
        guard let ring = calls.first(where: { $0.value == action.callUUID })?.key else { action.fail(); return }
        transport.request("calls.accept", ["ringid": ring, "device_id": transport.device]) { result in
            switch result {
            case .success:
                do { try self.audio.prepareCall() } catch { audioLog("CallKit session setup failed: \(error.localizedDescription)"); action.fail(); return }
                self.pendingCalls.remove(ring); action.fulfill(); self.audio.start(ring, voicemailId: nil) { _ in }
            case .failure: action.fail()
            }
        }
    }
    func provider(_ provider: CXProvider, perform action: CXStartCallAction) {
        do { try audio.prepareCall(); action.fulfill() } catch { audioLog("CallKit session setup failed: \(error.localizedDescription)"); action.fail() }
    }
    func provider(_ provider: CXProvider, perform action: CXEndCallAction) {
        guard let ring = calls.first(where: { $0.value == action.callUUID })?.key else { action.fulfill(); return }
        let method = pendingCalls.contains(ring) ? "calls.decline" : "calls.cut"
        transport.request(method, ["ringid": ring, "give_no_reason": true]) { result in
            switch result { case .success: self.audio.stop { _ in }; self.calls.removeValue(forKey: ring); self.pendingCalls.remove(ring); action.fulfill(); case .failure: action.fail() }
        }
    }
    func provider(_ provider: CXProvider, perform action: CXSetMutedCallAction) { audio.mute(action.isMuted) { result in switch result { case .success: action.fulfill(); case .failure: action.fail() } } }
    func provider(_ provider: CXProvider, didActivate audioSession: AVAudioSession) { audio.activate() }
    func provider(_ provider: CXProvider, didDeactivate audioSession: AVAudioSession) { audio.deactivate() }
}

@_cdecl("init_plugin_call_service")
func initPlugin() -> Plugin { CallServicePlugin() }
