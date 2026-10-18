import Foundation
import LiveKit

/// One device's audio in a call's LiveKit room: publishes the microphone, subscribes to every
/// remote audio track, reports who is speaking.
@MainActor
final class CallMedia: NSObject {
    enum State: Equatable {
        case idle, connecting, connected, reconnecting, disconnected
    }

    var onState: ((State) -> Void)?
    /// LiveKit identities currently speaking, including this device's own.
    var onSpeakers: ((Set<String>) -> Void)?
    /// The room dropped and LiveKit gave up reconnecting.
    var onLost: (() -> Void)?

    private(set) var state: State = .idle {
        didSet { if state != oldValue { onState?(state) } }
    }

    private var room: Room?
    private var generation = 0
    /// iOS: the microphone is published only once CallKit has activated the audio session;
    /// enabling it earlier makes LiveKit's engine fail (-3010) and the call drops.
    private var micPending: Bool?
    private var muted = false
    /// What the microphone hears before its track is published (see `Preroll`), and where that
    /// goes, until the microphone is published.
    private var preroll: Preroll?
    private var prerollTo: String?

    /// The instance carrying the current call, told when the audio session becomes active.
    private static weak var current: CallMedia?
    #if os(iOS)
    private static var audioReady = false
    /// Told after the microphone starts or changes: LiveKit's voice processing reconfigures the
    /// audio session then, which drops a speaker override.
    static var microphoneChanged: (@MainActor () -> Void)?
    #else
    private static let audioReady = true
    #endif

    override init() {
        super.init()
        CallMedia.current = self
    }

    /// iOS hands the audio session to CallKit: LiveKit must not configure it, and its engine
    /// stays off until CallKit activates the session.
    static func prepareAudio() {
        #if os(iOS)
        AudioManager.shared.audioSession.isAutomaticConfigurationEnabled = false
        try? AudioManager.shared.setEngineAvailability(.none)
        #endif
    }

    #if os(iOS)
    static func audioSessionActivated() {
        do {
            try AudioManager.shared.setEngineAvailability(.default)
        } catch {
            // Without microphone access LiveKit refuses to enable input; still play the call.
            try? AudioManager.shared.setEngineAvailability(AudioEngineAvailability(isInputAvailable: false, isOutputAvailable: true))
        }
        audioReady = true
        if let current { Task { await current.publishPendingMicrophone() } }
    }

    static func audioSessionDeactivated() {
        audioReady = false
        try? AudioManager.shared.setEngineAvailability(.none)
    }
    #endif

    var localIdentity: String? { room?.localParticipant.identity?.stringValue }

    /// This device's voice feeds the live transcript, the representative and the recording, so it is
    /// published for speech quality: the platform's echo cancellation, noise suppression and gain
    /// control (Apple's voice processing), Opus at 32 kbps rather than LiveKit's 24 kbps speech preset,
    /// redundant audio (RED) against packet loss, and no DTX, which clips the start of words.
    private static var roomOptions: RoomOptions {
        RoomOptions(
            defaultAudioCaptureOptions: AudioCaptureOptions(echoCancellation: true, autoGainControl: true, noiseSuppression: true),
            defaultAudioPublishOptions: AudioPublishOptions(encoding: AudioEncoding(maxBitrate: 32_000), dtx: false, red: true))
    }

    func connect(_ grant: MediaGrant, muted: Bool) async throws {
        await disconnect()
        generation += 1
        let room = Room(delegate: self, connectOptions: ConnectOptions(autoSubscribe: true), roomOptions: Self.roomOptions)
        self.room = room
        state = .connecting
        self.muted = muted
        prerollTo = muted ? nil : grant.prerollTo
        // The microphone starts while the room is joined rather than after: what it hears meanwhile
        // (often the first words after answering) is the pre-roll, which ring-media puts in front
        // of the published track. Joining does not wait for it. On iOS it starts once CallKit has
        // activated the audio session.
        let starting = Self.audioReady ? Task { await self.startPreroll() } : nil
        do {
            try await room.connect(url: grant.url, token: grant.token)
            await starting?.value
            if self.room === room { state = .connected }
            if Self.audioReady {
                try await publishMicrophone(room, muted: muted)
            } else {
                // Remote audio plays as soon as the session is active; the microphone follows.
                micPending = muted
                DevLog.log("microphone waits for the audio session")
            }
        } catch {
            await starting?.value
            if self.room === room {
                self.room = nil
                state = .disconnected
                preroll?.discard()
                preroll = nil
            }
            await room.disconnect()
            throw error
        }
    }

    private func startPreroll() async {
        guard prerollTo != nil, preroll == nil else { return }
        let generation = generation
        let started = await Preroll.start(options: Self.roomOptions.defaultAudioCaptureOptions)
        // The call may have ended, or another begun, while the microphone started.
        guard generation == self.generation, prerollTo != nil else {
            started?.discard()
            return
        }
        preroll = started
        #if os(iOS)
        if started != nil { Self.microphoneChanged?() }
        #endif
    }

    /// Publishes exactly one microphone track; a muted participant keeps it published but muted.
    private func publishMicrophone(_ room: Room, muted: Bool) async throws {
        let muted = muted || self.muted
        if let preroll, let prerollTo, !muted {
            // The track the pre-roll was heard on, so ring-media can line the two up.
            self.prerollTo = nil
            await preroll.open(room, to: prerollTo)
            try await room.localParticipant.publish(audioTrack: preroll.track)
            preroll.published()
        } else {
            preroll?.discard()
            preroll = nil
            try await room.localParticipant.setMicrophone(enabled: true)
            if muted { try await room.localParticipant.setMicrophone(enabled: false) }
        }
        #if os(iOS)
        Self.microphoneChanged?()
        #endif
    }

    private func publishPendingMicrophone() async {
        guard let room, let muted = micPending else { return }
        micPending = nil
        if !muted { await startPreroll() }
        do {
            try await publishMicrophone(room, muted: muted)
            DevLog.log("microphone published after audio session activation")
        } catch {
            DevLog.log("microphone publish failed: \(error)")
        }
    }

    func setMuted(_ muted: Bool) async {
        self.muted = muted
        // Nothing more is heard once muted, pre-roll included.
        if muted { preroll?.stop() }
        if micPending != nil {
            micPending = muted
            return
        }
        guard let room else { return }
        _ = try? await room.localParticipant.setMicrophone(enabled: !muted)
        #if os(iOS)
        Self.microphoneChanged?()
        #endif
    }

    func disconnect() async {
        micPending = nil
        prerollTo = nil
        preroll?.discard()
        preroll = nil
        guard let room else { return }
        self.room = nil
        generation += 1
        state = .idle
        await room.disconnect()
    }

    #if os(macOS)
    var outputDevices: [AudioDevice] { AudioManager.shared.outputDevices }

    var outputDeviceID: String { AudioManager.shared.outputDevice.deviceId }

    func selectOutput(_ device: AudioDevice) {
        AudioManager.shared.outputDevice = device
    }
    #endif

    private func handleConnection(_ connectionState: ConnectionState, room: Room) {
        guard room === self.room else { return }
        switch connectionState {
        case .connected: state = .connected
        case .reconnecting: state = .reconnecting
        case .connecting: state = .connecting
        case .disconnected, .disconnecting:
            if state == .connected || state == .reconnecting {
                self.room = nil
                state = .disconnected
                onLost?()
            }
        }
    }
}

extension CallMedia: RoomDelegate {
    nonisolated func room(_ room: Room, didUpdateSpeakingParticipants participants: [LiveKit.Participant]) {
        let identities = Set(participants.compactMap { $0.identity?.stringValue })
        Task { @MainActor [weak self] in
            guard let self, room === self.room else { return }
            self.onSpeakers?(identities)
        }
    }

    nonisolated func room(_ room: Room, didUpdateConnectionState connectionState: ConnectionState, from oldConnectionState: ConnectionState) {
        Task { @MainActor [weak self] in
            self?.handleConnection(connectionState, room: room)
        }
    }

    nonisolated func room(_ room: Room, participant: LocalParticipant, remoteDidSubscribeTrack publication: LocalTrackPublication) {
        Task { @MainActor [weak self] in
            guard let self, room === self.room else { return }
            self.preroll?.subscribed()
        }
    }
}

/// The microphone's pre-roll: what it hears from the moment the call is answered, while the call's
/// audio connects and before the published track reaches anyone. It streams to ring-media
/// (`MediaGrant.prerollTo`) as LiveKit pre-connect audio (16-bit little-endian PCM), and ring-media
/// puts it in front of the live track, so the first words reach the recording, the transcript and
/// a silicon's representative.
@MainActor
private final class Preroll {
    /// LiveKit's topic for pre-connect audio, which ring-media reads.
    static let topic = "lk.agent.pre-connect-audio-buffer"
    /// Full speech bandwidth at half the bytes of 48 kHz, which matters on a mobile uplink.
    static let sampleRate = 24000
    /// After the track is first subscribed the pre-roll runs on this long, so it overlaps the live
    /// audio ring-media lines it up with.
    static let tail: Duration = .milliseconds(600)
    /// It ends this long after publishing in any case: LiveKit does not tell a publisher that the
    /// (hidden) recorder subscribed.
    static let limit: Duration = .seconds(4)
    /// The most kept while connecting (15 s), and roughly how much goes out at a time (0.1 s).
    static let maxBytes = sampleRate * 2 * 15
    static let sendBytes = sampleRate * 2 / 10

    let track: LocalAudioTrack
    private let recorder: LocalAudioTrackRecorder
    private var pending = Data()
    private var writer: ByteStreamWriter?
    private var sending: Task<Void, Never>?
    private var isPublished = false
    private var stopped = false

    private init(track: LocalAudioTrack, recorder: LocalAudioTrackRecorder) {
        self.track = track
        self.recorder = recorder
    }

    /// Starts a microphone track and listens to it; nil when the microphone cannot start.
    static func start(options: AudioCaptureOptions) async -> Preroll? {
        let track = await LocalAudioTrack.createTrack(options: options)
        let recorder = LocalAudioTrackRecorder(track: track, format: .pcmFormatInt16, sampleRate: sampleRate)
        let stream: LocalAudioTrackRecorder.Stream
        do {
            stream = try await recorder.start()
        } catch {
            DevLog.log("pre-roll could not start: \(error)")
            try? await track.stop()
            return nil
        }
        let preroll = Preroll(track: track, recorder: recorder)
        Task { [weak preroll] in
            for await data in stream {
                preroll?.capture(data)
            }
        }
        return preroll
    }

    /// Opens the stream to `identity`. Done before the track is published, so ring-media expects the
    /// pre-roll by the time the track arrives.
    func open(_ room: Room, to identity: String) async {
        guard !stopped else { return }
        do {
            writer = try await room.localParticipant.streamBytes(options: StreamByteOptions(
                topic: Self.topic,
                attributes: ["sampleRate": "\(Self.sampleRate)", "channels": "1"],
                destinationIdentities: [LiveKit.Participant.Identity(from: identity)]))
        } catch {
            DevLog.log("pre-roll could not open: \(error)")
            stop()
            return
        }
        send(closing: stopped)
    }

    /// The track went out: the pre-roll ends once it has been heard.
    func published() {
        isPublished = true
        Task { [weak self] in
            try? await Task.sleep(for: Self.limit)
            self?.stop()
        }
    }

    func subscribed() {
        Task { [weak self] in
            try? await Task.sleep(for: Self.tail)
            self?.stop()
        }
    }

    /// Stops listening; whatever was heard is still sent, then the stream closes.
    func stop() {
        guard !stopped else { return }
        stopped = true
        recorder.stop()
        send(closing: true)
    }

    /// Stops, and releases the microphone if its track was never published.
    func discard() {
        stop()
        guard !isPublished else { return }
        let track = track
        Task { try? await track.stop() }
    }

    private func capture(_ data: Data) {
        guard !stopped else { return }
        pending.append(data)
        if writer == nil {
            if pending.count > Self.maxBytes { pending.removeFirst(pending.count - Self.maxBytes) }
        } else if pending.count >= Self.sendBytes {
            send(closing: false)
        }
    }

    private func send(closing: Bool) {
        guard let writer else { return }
        let bytes = pending
        pending = Data()
        let previous = sending
        sending = Task {
            await previous?.value
            if !bytes.isEmpty { try? await writer.write(bytes) }
            if closing { try? await writer.close() }
        }
    }
}
