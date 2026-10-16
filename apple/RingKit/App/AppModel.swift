import AVFoundation
import Foundation
import Observation
import UserNotifications

enum Appearance: String, CaseIterable, Identifiable, Sendable {
    case system, light, dark
    var id: String { rawValue }

    var label: String {
        switch self {
        case .system: "System"
        case .light: "Light"
        case .dark: "Dark"
        }
    }
}

/// A short message shown at the top of the window.
struct Notice: Identifiable, Equatable, Sendable {
    enum Style: Sendable { case info, error }
    let id = UUID()
    var message: String
    var style: Style
}

/// The whole app's state: the session, people and history, and the calls on this device.
@MainActor
@Observable
final class AppModel {
    static let shared = AppModel()

    // MARK: Session

    private(set) var session: StoredSession?
    var me: Account?
    var currentDevice: Device?
    var meta: Meta?
    var realtimeStatus: RealtimeConnection.Status = .idle

    var isSignedIn: Bool { session != nil }
    var myUUID: String? { session?.accountUUID }
    var myDeviceID: String? { session?.deviceID }
    var limits: Limits { meta?.limits ?? .standard }
    var accountsURL: URL { meta.flatMap { URL(string: $0.accountsURL) } ?? AppInfo.defaultAccountsURL }

    // MARK: History and settings

    var recents: [Call] = []
    var recentsCursor: String?
    var recentsLoaded = false
    var voicemails: [Voicemail] = []
    var voicemailsCursor: String?
    var voicemailsLoaded = false
    var devices: [Device] = []
    var settings: Settings?
    var voices: [Voice] = []

    // MARK: Calls

    /// Calls that are ringing, active or in voicemail and involve this account, by ringid.
    var liveCalls: [String: Call] = [:]
    /// The call this device carries audio for.
    var activeCall: CallSession?
    /// Calls ringing on this device, by invitation id.
    var rings: [String: IncomingRing] = [:]
    /// Rings the app shows itself (the rest are shown by the system call UI).
    var inAppRingIDs: [String] = []
    var notice: Notice?
    /// "Moved to Mac": shown on the device a call just left.
    var movedNotice: String?
    /// A home section to bring forward, for example after tapping a voicemail notification.
    var requestedSection: HomeSection?

    var appearance: Appearance = Appearance(rawValue: UserDefaults.standard.string(forKey: "ring.appearance") ?? "") ?? .system {
        didSet { UserDefaults.standard.set(appearance.rawValue, forKey: "ring.appearance") }
    }

    // MARK: Platform hooks

    weak var callSystem: CallSystem?
    /// Runs after an interactive sign-in (the Mac turns on its login item).
    @ObservationIgnored var onSignIn: (() -> Void)?
    /// iOS: true while CallKit has activated the audio session. Always true on the Mac.
    var audioActive: Bool
    var voipToken: String? { didSet { if voipToken != oldValue { scheduleDeviceRegistration() } } }
    var apnsToken: String? { didSet { if apnsToken != oldValue { scheduleDeviceRegistration() } } }

    let realtime = RealtimeConnection()
    let media = CallMedia()

    @ObservationIgnored private var registrationTask: Task<Void, Never>?
    @ObservationIgnored private var recentsRefreshTask: Task<Void, Never>?
    @ObservationIgnored private var ringExpiry: [String: Task<Void, Never>] = [:]
    @ObservationIgnored var bootstrapped = false

    init() {
        #if os(iOS)
        audioActive = false
        #else
        audioActive = true
        #endif
        session = SessionStore.load()
        if let session, session.server != ServerConfig.current {
            // A session belongs to the server that issued it.
            SessionStore.clear()
            self.session = nil
        }
        realtime.onEvent = { [weak self] event in self?.handle(event) }
        realtime.onStatus = { [weak self] status in self?.realtimeStatus = status }
        realtime.onUnauthorized = { [weak self] in self?.sessionInvalidated() }
        media.onState = { [weak self] state in self?.activeCall?.mediaState = state }
        media.onSpeakers = { [weak self] identities in self?.activeCall?.speakingIdentities = identities }
        media.onLost = { [weak self] in self?.mediaLost() }
    }

    var api: APIClient {
        APIClient(baseURL: session?.server ?? ServerConfig.current, token: session?.token)
    }

    // MARK: Lifecycle

    /// Starts listening for calls and loads what the UI needs. Safe to call repeatedly.
    func start() {
        guard let session else { return }
        realtime.start(server: session.server, token: session.token)
        guard !bootstrapped else { return }
        bootstrapped = true
        Task {
            await refreshMe()
            await nameDeviceIfGeneric()
            scheduleDeviceRegistration()
            async let meta: Void = loadMeta()
            async let recents: Void = refreshRecents()
            async let voicemails: Void = refreshVoicemails()
            _ = await (meta, recents, voicemails)
        }
    }

    /// Foreground, wake or network change: make sure the socket is alive and state is fresh.
    func resume() {
        guard isSignedIn else { return }
        realtime.ensureConnected()
        Task { await reconcile() }
    }

    // MARK: Sign-in

    func beginSignIn() async throws -> SignInAttempt {
        let client = APIClient(baseURL: ServerConfig.current, token: nil)
        let meta = try await client.meta()
        self.meta = meta
        let accounts = URL(string: meta.accountsURL) ?? AppInfo.defaultAccountsURL
        let redirect = meta.redirectURIs.native.isEmpty ? AppInfo.nativeRedirectURI : meta.redirectURIs.native
        return SignInAttempt(accountsURL: accounts, redirectURI: redirect)
    }

    func completeSignIn(_ attempt: SignInAttempt, callback: URL) async throws {
        guard case .code(let code) = try attempt.outcome(from: callback) else { throw RingError.cancelled }
        let server = ServerConfig.current
        let client = APIClient(baseURL: server, token: nil)
        let grant = try await client.signInCarbon(CarbonLogin(
            code: code, codeVerifier: attempt.verifier, redirectURI: attempt.redirectURI, label: AppInfo.deviceName))
        let stored = StoredSession(
            token: grant.sessionToken, server: server, accountUUID: grant.account.uuid,
            accountID: grant.account.id, deviceID: grant.deviceID, expiresAt: grant.expiresAt)
        SessionStore.save(stored)
        session = stored
        me = grant.account
        bootstrapped = false
        start()
        onSignIn?()
        await requestPermissions()
    }

    func signOut() async {
        let client = api
        if let session = activeCall, session.isLive { await performHangUp(session) }
        await endEverythingLocally()
        realtime.stop()
        session = nil
        SessionStore.clear()
        clearAccountState()
        try? await client.signOut()
    }

    /// The server no longer accepts this session.
    func sessionInvalidated() {
        guard session != nil else { return }
        session = nil
        SessionStore.clear()
        realtime.stop()
        Task {
            await endEverythingLocally()
            clearAccountState()
            show("You were signed out. Sign in again to keep receiving calls.", style: .error)
        }
    }

    private func clearAccountState() {
        me = nil
        currentDevice = nil
        recents = []
        recentsCursor = nil
        recentsLoaded = false
        voicemails = []
        voicemailsCursor = nil
        voicemailsLoaded = false
        devices = []
        settings = nil
        liveCalls = [:]
        rings = [:]
        inAppRingIDs = []
        bootstrapped = false
    }

    /// Changes the server this app talks to. Signs out first, because sessions are bound to their server.
    func setServerOverride(_ url: URL?) async {
        guard (url ?? ServerConfig.defaultURL) != ServerConfig.current else { return }
        if isSignedIn { await signOut() }
        ServerConfig.override = url
        meta = nil
    }

    func requestPermissions() async {
        if AVAudioApplication.shared.recordPermission == .undetermined {
            _ = await AVAudioApplication.requestRecordPermission()
        }
        _ = try? await UNUserNotificationCenter.current().requestAuthorization(options: [.alert, .sound, .badge])
    }

    // MARK: Device registration

    func scheduleDeviceRegistration() {
        guard isSignedIn else { return }
        registrationTask?.cancel()
        registrationTask = Task {
            try? await Task.sleep(for: .milliseconds(400))
            guard !Task.isCancelled else { return }
            await registerDevice()
        }
    }

    /// iOS only reports "iPhone" or "iPad" as the device name; once signed in, use the owner's name.
    private func nameDeviceIfGeneric() async {
        #if os(iOS)
        guard let device = currentDevice, let name = device.name, ["iPhone", "iPad"].contains(name),
              let first = me?.displayName?.split(separator: " ").first else { return }
        let key = "ring.device.named.\(device.deviceID)"
        guard !UserDefaults.standard.bool(forKey: key) else { return }
        UserDefaults.standard.set(true, forKey: key)
        if let renamed = try? await api.patchDevice(device.deviceID, DevicePatch(name: "\(first)'s \(name)")) {
            currentDevice = renamed
        }
        #endif
    }

    private func registerDevice() async {
        var push: [PushRegistration] = []
        let environment = Provisioning.apnsEnvironment
        if let voipToken { push.append(PushRegistration(kind: .apnsVoip, token: voipToken, environment: environment)) }
        if let apnsToken { push.append(PushRegistration(kind: .apns, token: apnsToken, environment: environment)) }
        let update = DeviceUpdate(platform: AppInfo.platform, appVersion: AppInfo.version, push: push.isEmpty ? nil : push)
        do {
            currentDevice = try await api.updateCurrentDevice(update)
        } catch {
            handle(error, silently: true)
        }
    }

    // MARK: Loading

    func loadMeta() async {
        if let meta = try? await api.meta() {
            self.meta = meta
            voices = meta.voices
        }
    }

    func refreshMe() async {
        do {
            let me = try await api.me()
            self.me = me.account
            currentDevice = me.device
        } catch {
            handle(error, silently: true)
        }
    }

    func refreshRecents() async {
        do {
            let page = try await api.calls(limit: 50)
            // Realtime events that arrived while the page was loading are newer than it: keep the
            // live copy of a call that is still on, an ending the page predates, and live calls
            // the page does not have yet.
            let ended = Dictionary(recents.filter { $0.state == .ended }.map { ($0.ringid, $0) }, uniquingKeysWith: { first, _ in first })
            var items = page.items.map { call in call.state.isLive ? ended[call.ringid] ?? liveCalls[call.ringid] ?? call : call }
            // Ended is final, so the page also settles a live copy whose ending this app missed.
            for call in page.items where call.state == .ended { liveCalls[call.ringid] = nil }
            let listed = Set(items.map(\.ringid))
            items += liveCalls.values.filter { !listed.contains($0.ringid) }
            recents = items.sorted { $0.createdAt > $1.createdAt }
            recentsCursor = page.nextCursor
            recentsLoaded = true
        } catch {
            handle(error, silently: recentsLoaded)
        }
    }

    func loadMoreRecents() async {
        guard let cursor = recentsCursor else { return }
        recentsCursor = nil
        do {
            let page = try await api.calls(limit: 50, cursor: cursor)
            let known = Set(recents.map(\.ringid))
            recents.append(contentsOf: page.items.filter { !known.contains($0.ringid) })
            recentsCursor = page.nextCursor
        } catch {
            recentsCursor = cursor
            handle(error, silently: true)
        }
    }

    func scheduleRecentsRefresh() {
        recentsRefreshTask?.cancel()
        recentsRefreshTask = Task {
            try? await Task.sleep(for: .seconds(1.5))
            guard !Task.isCancelled else { return }
            await refreshRecents()
        }
    }

    func refreshVoicemails() async {
        do {
            let page = try await api.voicemails(limit: 50)
            voicemails = page.items
            voicemailsCursor = page.nextCursor
            voicemailsLoaded = true
        } catch {
            handle(error, silently: voicemailsLoaded)
        }
    }

    func loadMoreVoicemails() async {
        guard let cursor = voicemailsCursor else { return }
        voicemailsCursor = nil
        do {
            let page = try await api.voicemails(limit: 50, cursor: cursor)
            let known = Set(voicemails.map(\.voicemailID))
            voicemails.append(contentsOf: page.items.filter { !known.contains($0.voicemailID) })
            voicemailsCursor = page.nextCursor
        } catch {
            voicemailsCursor = cursor
            handle(error, silently: true)
        }
    }

    var unreadVoicemails: Int { voicemails.filter { !$0.read }.count }

    func markRead(_ voicemail: Voicemail) async {
        guard !voicemail.read else { return }
        if let updated = try? await api.markVoicemailRead(voicemail.voicemailID) { upsert(updated) }
    }

    func delete(_ voicemail: Voicemail) async {
        do {
            try await api.deleteVoicemail(voicemail.voicemailID)
            voicemails.removeAll { $0.voicemailID == voicemail.voicemailID }
        } catch {
            handle(error)
        }
    }

    func upsert(_ voicemail: Voicemail) {
        if let index = voicemails.firstIndex(where: { $0.voicemailID == voicemail.voicemailID }) {
            var merged = voicemail
            if merged.audioURL == nil { merged.audioURL = voicemails[index].audioURL }
            voicemails[index] = merged
        } else if voicemail.to.uuid == myUUID {
            voicemails.insert(voicemail, at: 0)
        }
    }

    func refreshDevices() async {
        do {
            devices = try await api.devices().sorted { ($0.current ? 0 : 1, $0.label) < ($1.current ? 0 : 1, $1.label) }
            if let current = devices.first(where: \.current) { currentDevice = current }
        } catch {
            handle(error, silently: true)
        }
    }

    func setRingEnabled(_ enabled: Bool) async {
        guard let device = currentDevice else { return }
        currentDevice?.ringEnabled = enabled
        do {
            currentDevice = try await api.patchDevice(device.deviceID, DevicePatch(ringEnabled: enabled))
            await refreshDevices()
        } catch {
            currentDevice?.ringEnabled = !enabled
            handle(error)
        }
    }

    func renameDevice(_ name: String) async {
        let trimmed = name.trimmingCharacters(in: .whitespacesAndNewlines)
        guard let device = currentDevice, !trimmed.isEmpty, trimmed != device.name else { return }
        guard trimmed.unicodeScalars.count <= 80 else {
            show("Device names can be up to 80 characters.", style: .error)
            return
        }
        do {
            currentDevice = try await api.patchDevice(device.deviceID, DevicePatch(name: trimmed))
            await refreshDevices()
        } catch {
            handle(error)
        }
    }

    func revoke(_ device: Device) async {
        do {
            try await api.deleteDevice(device.deviceID)
            devices.removeAll { $0.deviceID == device.deviceID }
        } catch {
            handle(error)
        }
    }

    func refreshSettings() async {
        do {
            settings = try await api.settings()
            if voices.isEmpty, let list = try? await api.voices() { voices = list.items }
        } catch {
            handle(error, silently: true)
        }
    }

    func patchSettings(_ patch: JSONValue) async {
        do {
            settings = try await api.patchSettings(patch)
        } catch {
            handle(error)
        }
    }

    // MARK: Notices

    func show(_ message: String, style: Notice.Style = .info) {
        let notice = Notice(message: message, style: style)
        self.notice = notice
        Task {
            try? await Task.sleep(for: .seconds(5))
            if self.notice?.id == notice.id { self.notice = nil }
        }
    }

    func handle(_ error: Error, silently: Bool = false) {
        if let error = error as? RingError {
            if error.isSessionInvalid {
                sessionInvalidated()
                return
            }
            if case .cancelled = error { return }
            if !silently { show(error.errorDescription ?? "Something went wrong.", style: .error) }
        } else if !silently {
            show(error.localizedDescription, style: .error)
        }
    }

    // MARK: Rings

    var inAppRing: IncomingRing? {
        inAppRingIDs.lazy.compactMap { self.rings[$0] }.first
    }

    func scheduleExpiry(for ring: IncomingRing) {
        guard let expiresAt = ring.expiresAt else { return }
        ringExpiry[ring.invitationID]?.cancel()
        ringExpiry[ring.invitationID] = Task {
            let delay = max(1, expiresAt.timeIntervalSinceNow + 2)
            try? await Task.sleep(for: .seconds(delay))
            guard !Task.isCancelled, let ring = self.rings[ring.invitationID] else { return }
            self.stopRing(ring, cause: .unanswered)
        }
    }

    func cancelExpiry(_ invitationID: String) {
        ringExpiry[invitationID]?.cancel()
        ringExpiry[invitationID] = nil
    }
}
