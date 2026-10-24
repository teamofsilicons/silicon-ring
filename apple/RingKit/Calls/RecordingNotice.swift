import Foundation

/// The notice that Ring records and transcribes calls, and which providers process the audio.
/// Each device asks once, right after sign-in and again before a call until it has agreed, and
/// remembers the answer.
enum RecordingNotice {
    /// Raise this when the notice's wording changes enough that people should agree again.
    static let version = 1
    private static let acceptedKey = "ring.recordingNotice.version"
    /// The version last put to the person signed in here and answered with "Not now".
    private static let offeredKey = "ring.recordingNotice.offered"

    static var isAccepted: Bool {
        UserDefaults.standard.integer(forKey: acceptedKey) >= version
    }

    static func accept() {
        UserDefaults.standard.set(version, forKey: acceptedKey)
    }

    /// Whether this sign-in has seen the current notice; the one after sign-in shows only until then.
    static var wasOffered: Bool {
        UserDefaults.standard.integer(forKey: offeredKey) >= version
    }

    static func markOffered() {
        UserDefaults.standard.set(version, forKey: offeredKey)
    }

    /// Signing out: the next sign-in shows the notice again, unless this device has agreed.
    static func resetOffer() {
        UserDefaults.standard.removeObject(forKey: offeredKey)
    }
}

/// The recording notice on screen, and the call action waiting for it.
struct RecordingNoticeRequest: Identifiable, Equatable, Sendable {
    enum Action: Equatable, Sendable {
        /// Right after sign-in, before any call: nothing waits.
        case welcome
        /// Calling this id.
        case place(String)
        /// Answering this ring in the app.
        case answer(IncomingRing)
        /// Moving this call here from another device.
        case switchHere(Call)
        /// A call this device already carries, connected before it agreed (answered from the system
        /// call screen, or with a newer notice). Nothing waits: the call and its audio go on, and
        /// the notice shows on its call screen above the controls.
        case answered

        var ring: IncomingRing? {
            if case .answer(let ring) = self { return ring }
            return nil
        }

        var isAnswered: Bool {
            if case .answered = self { return true }
            return false
        }
    }

    let id = UUID()
    var action: Action
}

/// Where the recording notice shows.
enum RecordingNoticePlacement: Equatable, Sendable {
    /// Over the whole window; nothing underneath responds until it is answered.
    case window
    /// On the screen of the live call it is about, over the people and transcript, leaving the
    /// call controls (mute, end) usable.
    case callScreen
}
