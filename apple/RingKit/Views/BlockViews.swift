import SwiftUI

/// Block or Unblock, and Report, for one account: in context menus and the call details menu.
/// Blocking asks first, through `blockConfirmation` on a view around it.
struct PersonSafetyActions: View {
    var account: Account
    /// The call this was about, for the report.
    var ringid: String?
    @Binding var confirmBlock: Account?
    @Environment(AppModel.self) private var model
    @Environment(\.openURL) private var openURL

    var body: some View {
        if model.isBlocked(account) {
            Button(Labels.Safety.unblock(account.label), systemImage: "hand.raised.slash") {
                Task { await model.unblock(account) }
            }
        } else {
            Button(Labels.Safety.block(account.label), systemImage: "hand.raised", role: .destructive) {
                confirmBlock = account
            }
        }
        Button(Labels.Safety.report(account.label), systemImage: "exclamationmark.bubble") {
            model.report(account, ringid: ringid, openURL: openURL)
        }
    }
}

extension View {
    /// Asks before blocking `account` while it is set, then runs `perform` (by default, blocks it).
    func blockConfirmation(
        _ account: Binding<Account?>,
        confirmTitle: String = Labels.Safety.confirmBlock,
        perform: @escaping (Account) -> Void
    ) -> some View {
        confirmationDialog(
            Text(account.wrappedValue.map { Labels.Safety.blockQuestion($0.label) } ?? "Block?"),
            isPresented: Binding(get: { account.wrappedValue != nil }, set: { if !$0 { account.wrappedValue = nil } }),
            titleVisibility: .visible,
            presenting: account.wrappedValue
        ) { blocked in
            Button(confirmTitle, role: .destructive) { perform(blocked) }
            Button(Labels.cancel, role: .cancel) {}
        } message: { _ in
            Text("Their calls won't ring your devices, and they can't leave you voicemail or add you to a call. They aren't told. You can unblock them in Settings.")
        }
    }
}

extension AppModel {
    /// Opens an email to Team of Silicons about an account, with its id and the call filled in.
    func report(_ account: Account, ringid: String?, openURL: OpenURLAction) {
        guard let url = ReportMail.url(account: account, ringid: ringid, reporter: me) else { return }
        openURL(url) { [weak self] accepted in
            guard !accepted else { return }
            MainActor.assumeIsolated {
                self?.show("To report \(account.id), email \(AppInfo.contactEmail).", style: .error)
            }
        }
    }
}

/// The `mailto:` link behind Report.
enum ReportMail {
    static func url(account: Account, ringid: String?, reporter: Account?) -> URL? {
        var lines = ["Reported account: \(account.id)", "Account uuid: \(account.uuid)"]
        if account.hasDisplayName { lines.append("Name shown: \(account.label)") }
        if let ringid { lines.append("Call (ringid): \(ringid)") }
        if let reporter { lines.append("Reported by: \(reporter.id)") }
        lines += ["", "What happened:", ""]
        let subject = "Report: \(account.id) on Ring"
        // RFC 6068: every reserved character is percent-encoded, and line breaks are CRLF.
        let query = "subject=\(encode(subject))&body=\(encode(lines.joined(separator: "\r\n")))"
        return URL(string: "mailto:\(AppInfo.contactEmail)?\(query)")
    }

    private static let unreserved = CharacterSet(charactersIn: "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-._~")

    private static func encode(_ text: String) -> String {
        text.addingPercentEncoding(withAllowedCharacters: unreserved) ?? ""
    }
}
