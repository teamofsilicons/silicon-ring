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

    /// The instance carrying the current call, told when the audio session becomes active.
    private static weak var current: CallMedia?
    #if os(iOS)
    private static var audioReady = false
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
        do {
            try await room.connect(url: grant.url, token: grant.token)
            if self.room === room { state = .connected }
            if Self.audioReady {
                try await publishMicrophone(room, muted: muted)
            } else {
                // Remote audio plays as soon as the session is active; the microphone follows.
                micPending = muted
                DevLog.log("microphone waits for the audio session")
            }
        } catch {
            if self.room === room {
                self.room = nil
                state = .disconnected
            }
            await room.disconnect()
            throw error
        }
    }

    /// Publishes exactly one microphone track; a muted participant keeps it published but muted.
    private func publishMicrophone(_ room: Room, muted: Bool) async throws {
        try await room.localParticipant.setMicrophone(enabled: true)
        if muted { try await room.localParticipant.setMicrophone(enabled: false) }
    }

    private func publishPendingMicrophone() async {
        guard let room, let muted = micPending else { return }
        micPending = nil
        do {
            try await publishMicrophone(room, muted: muted)
            DevLog.log("microphone published after audio session activation")
        } catch {
            DevLog.log("microphone publish failed: \(error)")
        }
    }

    func setMuted(_ muted: Bool) async {
        if micPending != nil {
            micPending = muted
            return
        }
        guard let room else { return }
        _ = try? await room.localParticipant.setMicrophone(enabled: !muted)
    }

    func disconnect() async {
        micPending = nil
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
}
