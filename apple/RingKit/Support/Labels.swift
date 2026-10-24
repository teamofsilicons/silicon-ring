/// Words on screen that the App Review notes and the App Store listing (deploy/appstore/) quote. The
/// app is their source: each is written once, here, and the views use it. Change one here, then
/// update the quotes to match.
///
/// Section headings (`SectionLabel`) and the recording indicator are drawn in capitals; the words
/// are as written here, which is also what VoiceOver reads.
enum Labels {
    /// Closes the delete account screen and the Block confirmation without doing anything.
    static let cancel = "Cancel"

    enum Settings {
        static let account = "Account"
        /// The row in Account that opens the delete account screen.
        static let deleteAccount = "Delete Ring account…"
        static let blocked = "Blocked"
        static let unblock = "Unblock"
        static let about = "About"
        /// The rows in About, and the links under the sign-in screen.
        static let privacyPolicy = "Privacy Policy"
        static let support = "Support"
    }

    enum DeleteAccount {
        static let title = "Delete your Ring account?"
        static let confirm = "Delete Ring account"
    }

    enum Recording {
        static let noticeTitle = "Ring records calls"
        static let agree = "Agree and continue"
        static let notNow = "Not now"
        /// The notice's link, the same words as the Settings row.
        static let privacyPolicy = Settings.privacyPolicy
        /// On the outgoing and in-call screens.
        static let indicator = "Recorded · Transcribed"
    }

    enum Safety {
        static func block(_ name: String) -> String { "Block \(name)…" }
        static func unblock(_ name: String) -> String { "Unblock \(name)" }
        static func report(_ name: String) -> String { "Report \(name)…" }
        static func blockQuestion(_ name: String) -> String { "Block \(name)?" }
        /// Confirms blocking from Recents and a call's page.
        static let confirmBlock = "Block"
        /// On the in-app incoming call screen, next to Silence.
        static let incomingBlock = "Block"
        /// Confirms Block on the incoming call screen.
        static let declineAndBlock = "Decline and Block"
    }
}
