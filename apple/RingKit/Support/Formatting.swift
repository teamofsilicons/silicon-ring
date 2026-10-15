import Foundation

enum Format {
    /// `0:42`, `12:05`, `1:02:09`.
    static func duration(ms: Int) -> String {
        duration(seconds: max(0, ms / 1000))
    }

    static func duration(seconds total: Int) -> String {
        let hours = total / 3600
        let minutes = (total % 3600) / 60
        let seconds = total % 60
        if hours > 0 { return String(format: "%d:%02d:%02d", hours, minutes, seconds) }
        return String(format: "%d:%02d", minutes, seconds)
    }

    /// `14:03` today, `Yesterday`, a weekday this week, otherwise a short date.
    static func when(_ date: Date, now: Date = Date()) -> String {
        let calendar = Calendar.current
        if calendar.isDateInToday(date) {
            return date.formatted(date: .omitted, time: .shortened)
        }
        if calendar.isDateInYesterday(date) { return "Yesterday" }
        if let days = calendar.dateComponents([.day], from: calendar.startOfDay(for: date), to: calendar.startOfDay(for: now)).day, days < 7 {
            return date.formatted(.dateTime.weekday(.wide))
        }
        return date.formatted(date: .abbreviated, time: .omitted)
    }

    static func timestamp(_ date: Date) -> String {
        date.formatted(date: .abbreviated, time: .shortened)
    }

    static func clock(_ date: Date) -> String {
        date.formatted(.dateTime.hour().minute().second())
    }

    /// The pixel-font meta line of a Recents row: when, then duration or outcome.
    static func outcome(of call: Call, me: String?) -> String {
        switch call.state {
        case .ringing: return "Ringing"
        case .active: return "On call"
        case .voicemail: return "Voicemail"
        case .ended, .unknown: break
        }
        if let ms = call.durationMS, call.answeredAt != nil { return duration(ms: ms) }
        let mine = call.invitations.first { $0.to.uuid == me }
        switch call.endReason {
        case .declined:
            if call.direction == .incoming { return "Declined" }
            if let reason = call.invitations.first(where: { $0.state == .declined })?.reason, !reason.isEmpty { return "Declined · \(reason)" }
            return "Declined"
        case .missed:
            if call.voicemailID != nil { return call.direction == .incoming ? "Missed · voicemail" : "Voicemail" }
            return call.direction == .incoming ? "Missed" : "No answer"
        case .busy: return call.direction == .incoming ? "Missed · busy" : "Busy"
        case .cancelled: return call.direction == .incoming && mine?.state != .declined ? "Missed" : "Cancelled"
        case .failed: return "Failed"
        case .serverRestart: return "Dropped"
        case .completed, .unknown, .none: return call.voicemailID != nil ? "Voicemail" : "Ended"
        }
    }

    /// Whether a Recents row shows as missed (danger colour).
    static func isMissed(_ call: Call, me: String?) -> Bool {
        guard call.direction == .incoming, call.answeredAt == nil, call.state == .ended else { return false }
        if let mine = call.invitations.first(where: { $0.to.uuid == me }) {
            return mine.state == .missed || mine.state == .cancelled || mine.state == .busy
        }
        return call.endReason == .missed || call.endReason == .cancelled || call.endReason == .busy
    }

    static func reason(of voicemail: Voicemail) -> String {
        switch voicemail.reason {
        case .missed: return "Missed"
        case .busy: return "Busy"
        case .declined:
            if let reason = voicemail.declineReason, !reason.isEmpty { return "Declined · \(reason)" }
            return "Declined"
        case .unknown: return "Voicemail"
        }
    }
}
