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
    }

    static func audioSessionDeactivated() {
        try? AudioManager.shared.setEngineAvailability(.none)
    }
    #endif

    var localIdentity: String? { room?.localParticipant.identity?.stringValue }

    func connect(_ grant: MediaGrant, muted: Bool) async throws {
        await disconnect()
        generation += 1
        let room = Room(delegate: self, connectOptions: ConnectOptions(autoSubscribe: true), roomOptions: RoomOptions())
        self.room = room
        state = .connecting
        do {
            try await room.connect(url: grant.url, token: grant.token)
            // Publish exactly one microphone track; a muted participant keeps it published but muted.
            try await room.localParticipant.setMicrophone(enabled: true)
            if muted { try await room.localParticipant.setMicrophone(enabled: false) }
            if self.room === room { state = .connected }
        } catch {
            if self.room === room {
                self.room = nil
                state = .disconnected
            }
            await room.disconnect()
            throw error
        }
    }

    func setMuted(_ muted: Bool) async {
        guard let room else { return }
        _ = try? await room.localParticipant.setMicrophone(enabled: !muted)
    }

    func disconnect() async {
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
