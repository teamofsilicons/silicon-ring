import Foundation

/// Why a call this device was carrying (or ringing for) ended, in system-call-UI terms.
enum CallEndCause: Sendable {
    case remoteEnded
    case answeredElsewhere
    case declinedElsewhere
    case unanswered
    case failed
}

/// The platform's call integration: CallKit on iPhone and iPad, the ringing panel on the Mac.
/// AppModel drives it; it calls back into AppModel for actions the system UI performs.
@MainActor
protocol CallSystem: AnyObject {
    /// The platform still shows a pending ring of its own that the app's ringtone belongs to (the
    /// Mac's call banners), so the app must not stop the ringtone yet.
    var keepsRinging: Bool { get }

    /// Starts ringing for an incoming call. Returns true when the platform shows its own ringing UI,
    /// false when the app should show its in-app incoming screen.
    func ring(_ ring: IncomingRing) -> Bool

    /// Ringing stopped without this device answering.
    func stopRinging(_ ring: IncomingRing, cause: CallEndCause)

    /// Nobody answered a call to this account. The server also sends a "Missed call" alert push,
    /// so a platform only needs this where that push cannot arrive.
    func missedCall(from caller: Account, ringid: String)

    /// Answers through the system call UI, which then calls back into `performAnswer`, so the system
    /// knows the call was answered (CallKit logs it in Recents). Returns false when the app should
    /// answer by itself.
    func answer(_ ring: IncomingRing) -> Bool

    /// This device started carrying a call: outgoing, answered in-app, or moved here.
    func began(_ session: CallSession)
    /// The call this device carries was answered.
    func connected(_ session: CallSession)
    /// The call ended by something other than a hang-up the system itself performed.
    func ended(_ session: CallSession, cause: CallEndCause)

    /// Routes a user hang-up or mute through the system. Returns true when the system will call back.
    func requestHangUp(_ session: CallSession) -> Bool
    func requestMute(_ session: CallSession, muted: Bool) -> Bool

    /// A call action waits for the recording notice, which shows in the app's window: bring it
    /// forward, with `ring` on its incoming screen when the action was answering it.
    func showRecordingNotice(for ring: IncomingRing?)
}

extension CallSystem {
    var keepsRinging: Bool { false }
    /// iPhone and iPad: call actions that wait for the notice start in the app, which is in front.
    func showRecordingNotice(for ring: IncomingRing?) {}
}
