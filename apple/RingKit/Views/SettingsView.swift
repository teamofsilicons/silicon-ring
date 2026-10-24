import SwiftUI

struct SettingsView: View {
    @Environment(AppModel.self) private var model
    @Environment(\.openURL) private var openURL
    #if DEBUG
    @State private var showServer = false
    #endif
    @State private var confirmSignOut = false
    @State private var showDeleteAccount = false
    @State private var deletingAccount = false
    @State private var deviceName = ""
    @State private var blocksAttempted = false

    var body: some View {
        @Bindable var model = model
        ScrollView {
            VStack(alignment: .leading, spacing: 24) {
                PageTitle(text: "Settings")
                    .padding(.top, 8)
                account
                thisDevice
                devices
                voicemail
                blockedAccounts
                group("Appearance") {
                    Picker("Appearance", selection: $model.appearance) {
                        ForEach(Appearance.allCases) { Text($0.label).tag($0) }
                    }
                    .pickerStyle(.segmented)
                    .labelsHidden()
                    .frame(maxWidth: .infinity)
                    .padding(14)
                }
                #if os(macOS)
                DesktopSettingsSection()
                #endif
                about
                Button(role: .destructive) {
                    confirmSignOut = true
                } label: {
                    Text("Sign out").foregroundStyle(Palette.danger)
                }
                .buttonStyle(RingButtonStyle(kind: .secondary, fullWidth: true))
                .confirmationDialog("Sign out of Ring on this device?", isPresented: $confirmSignOut) {
                    Button("Sign out", role: .destructive) { Task { await model.signOut() } }
                } message: {
                    Text("Calls will stop ringing here until you sign in again.")
                }
            }
            .padding(.horizontal, 16)
            .padding(.bottom, 48)
            .frame(maxWidth: 680)
            .frame(maxWidth: .infinity)
        }
        .background(Palette.background)
        .homeScreenChrome()
        .navigationTitle("Settings")
        #if DEBUG
        .sheet(isPresented: $showServer) { ServerSettingsView() }
        #endif
        .sheet(isPresented: $showDeleteAccount) { DeleteAccountView(working: $deletingAccount) }
        // A sheet covers the call screen and the recording notice (on iOS they are overlays of
        // RootView): make way for them, as HomeView does with the dialer.
        .onChange(of: model.activeCall?.ringid) { _, ringid in if ringid != nil { closeDeleteAccount() } }
        .onChange(of: model.inAppRing?.invitationID) { _, id in if id != nil { closeDeleteAccount() } }
        .onChange(of: model.recordingNoticePlacement) { _, placement in if placement == .window { closeDeleteAccount() } }
        .onChange(of: deletingAccount) { _, working in if !working, somethingWaitsUnderSheet { closeDeleteAccount() } }
        .task {
            await model.refreshDevices()
            await model.refreshSettings()
            deviceName = model.currentDevice?.name ?? ""
            await model.refreshBlocks()
            blocksAttempted = true
        }
    }

    /// Not while the deletion is in flight: it ends every call and ring when it goes through, and
    /// the sheet is where a failure is told. Once it fails, the sheet makes way.
    private func closeDeleteAccount() {
        guard !deletingAccount else { return }
        showDeleteAccount = false
    }

    /// The call screen or the recording notice, which came up while the deletion was in flight.
    private var somethingWaitsUnderSheet: Bool {
        if let session = model.activeCall, !session.minimized { return true }
        return model.inAppRing != nil || model.recordingNoticePlacement == .window
    }

    // MARK: Sections

    private var account: some View {
        group(Labels.Settings.account) {
            if let me = model.me {
                HStack(spacing: 14) {
                    AvatarView(account: me, size: 48)
                    VStack(alignment: .leading, spacing: 4) {
                        Text(me.label).font(RingFont.display(22)).foregroundStyle(Palette.ink)
                        Text(me.id).pixelID(11)
                    }
                    Spacer()
                }
                .padding(14)
                Hairline()
                rowButton("Edit profile on Silicon Accounts", symbol: "arrow.up.right") {
                    openURL(model.accountsURL)
                }
                Hairline().padding(.leading, 14)
                rowButton(Labels.Settings.deleteAccount, symbol: "trash", destructive: true) {
                    showDeleteAccount = true
                }
            }
        }
    }

    private var thisDevice: some View {
        group("This device") {
            HStack(spacing: 10) {
                Text("Name").font(RingFont.ui(15)).foregroundStyle(Palette.foreground)
                TextField(AppInfo.deviceName, text: $deviceName)
                    .font(RingFont.ui(15))
                    .multilineTextAlignment(.trailing)
                    .textFieldStyle(.plain)
                    .foregroundStyle(Palette.textSecondary)
                    .onSubmit { Task { await model.renameDevice(deviceName) } }
            }
            .padding(.horizontal, 14)
            .frame(minHeight: 48)
            Hairline().padding(.leading, 14)
            SettingToggle(
                title: "Ring on this device",
                detail: "Incoming calls ring here as well as on your other devices.",
                isOn: Binding(
                    get: { model.currentDevice?.ringEnabled ?? true },
                    set: { enabled in Task { await model.setRingEnabled(enabled) } }))
        }
    }

    private var devices: some View {
        group("Devices") {
            if model.devices.isEmpty {
                Text("Loading…").font(RingFont.ui(14)).foregroundStyle(Palette.textMuted).padding(14)
            }
            ForEach(Array(model.devices.enumerated()), id: \.element.deviceID) { index, device in
                if index > 0 { Hairline().padding(.leading, 50) }
                HStack(spacing: 12) {
                    Image(systemName: device.platform?.symbol ?? "questionmark.circle")
                        .font(.system(size: 18, weight: .light))
                        .foregroundStyle(Palette.textSecondary)
                        .frame(width: 24)
                    VStack(alignment: .leading, spacing: 3) {
                        HStack(spacing: 6) {
                            Text(device.label).font(RingFont.ui(15, .medium)).foregroundStyle(Palette.foreground)
                            if device.current { Badge(text: "This device") }
                        }
                        Text(deviceMeta(device)).pixelLabel(10)
                    }
                    Spacer()
                    if !device.current {
                        Button("Revoke") { Task { await model.revoke(device) } }
                            .buttonStyle(RingButtonStyle(kind: .secondary, small: true))
                    }
                }
                .padding(.horizontal, 14)
                .padding(.vertical, 10)
            }
        }
    }

    private func deviceMeta(_ device: Device) -> String {
        var parts: [String] = [device.online ? "Online" : "Offline"]
        if !device.ringEnabled { parts.append("Ringing off") }
        if let seen = device.lastSeenAt, !device.online { parts.append("Seen \(Format.when(seen))") }
        if let version = device.appVersion { parts.append("v\(version)") }
        return parts.joined(separator: " · ")
    }

    @ViewBuilder
    private var voicemail: some View {
        group("Voicemail") {
            if let settings = model.settings {
                SettingToggle(
                    title: "Take messages",
                    detail: "Callers hear your greeting and can leave a voicemail.",
                    isOn: Binding(
                        get: { settings.voicemail.enabled },
                        set: { enabled in Task { await model.patchSettings(SettingsPatch.voicemailEnabled(enabled)) } }))
                if !model.voices.isEmpty {
                    Hairline().padding(.leading, 14)
                    HStack {
                        Text("Greeting voice").font(RingFont.ui(15)).foregroundStyle(Palette.foreground)
                        Spacer()
                        Picker("Greeting voice", selection: Binding(
                            get: { settings.voice },
                            set: { voice in Task { await model.patchSettings(SettingsPatch.voice(voice)) } }
                        )) {
                            ForEach(model.voices) { voice in Text(voice.name).tag(voice.id) }
                        }
                        .labelsHidden()
                        .pickerStyle(.menu)
                    }
                    .padding(.horizontal, 14)
                    .frame(minHeight: 48)
                }
                ForEach([VoicemailReason.busy, .declined, .missed], id: \.self) { reason in
                    Hairline().padding(.leading, 14)
                    GreetingEditor(reason: reason, text: settings.voicemail.greetings[reason], limit: model.limits.greeting,
                                   ownerName: model.me?.label ?? "You") { value in
                        Task { await model.patchSettings(SettingsPatch.greeting(reason, value)) }
                    }
                }
            } else {
                Text("Loading…").font(RingFont.ui(14)).foregroundStyle(Palette.textMuted).padding(14)
            }
        }
    }

    @ViewBuilder
    private var blockedAccounts: some View {
        group(Labels.Settings.blocked) {
            if !model.blocked.isEmpty {
                ForEach(Array(model.blocked.enumerated()), id: \.element.uuid) { index, account in
                    if index > 0 { Hairline().padding(.leading, 58) }
                    HStack(spacing: 12) {
                        AvatarView(account: account, size: 32)
                        VStack(alignment: .leading, spacing: 2) {
                            Text(account.label).font(RingFont.ui(15, .medium)).foregroundStyle(Palette.foreground).lineLimit(1)
                            Text(account.id).pixelID(10)
                        }
                        Spacer()
                        Button(Labels.Settings.unblock) { Task { await model.unblock(account) } }
                            .buttonStyle(RingButtonStyle(kind: .secondary, small: true))
                            .accessibilityLabel("Unblock \(account.label)")
                    }
                    .padding(.horizontal, 14)
                    .padding(.vertical, 10)
                }
            } else {
                Text(blockedPlaceholder)
                    .font(RingFont.ui(14))
                    .foregroundStyle(Palette.textMuted)
                    .fixedSize(horizontal: false, vertical: true)
                    .padding(14)
            }
        }
    }

    private var blockedPlaceholder: String {
        if model.blockedLoaded {
            "No one is blocked. You can block someone from their calls in Recents."
        } else if blocksAttempted {
            "Ring couldn't load the people you blocked."
        } else {
            "Loading…"
        }
    }

    private var about: some View {
        group(Labels.Settings.about) {
            HStack {
                Text("Version").font(RingFont.ui(15)).foregroundStyle(Palette.foreground)
                Spacer()
                Text("\(AppInfo.version) (\(AppInfo.build))").pixelLabel(12, color: Palette.textSecondary)
            }
            .padding(.horizontal, 14)
            .frame(minHeight: 48)
            .contentShape(Rectangle())
            #if DEBUG
            // The developer server switch; Release builds always use the production server.
            .onLongPressGesture(minimumDuration: 1.0) { showServer = true }
            #endif
            Hairline().padding(.leading, 14)
            rowButton(Labels.Settings.privacyPolicy, symbol: "arrow.up.right") { openURL(AppInfo.privacyURL) }
            Hairline().padding(.leading, 14)
            rowButton(Labels.Settings.support, symbol: "arrow.up.right") { openURL(AppInfo.supportURL) }
            Hairline().padding(.leading, 14)
            rowButton("Source", symbol: "arrow.up.right") { openURL(AppInfo.sourceURL) }
            Hairline().padding(.leading, 14)
            HStack(spacing: 10) {
                RingMark(size: 22)
                Text("Ring sends no telemetry.").font(RingFont.ui(14)).foregroundStyle(Palette.textSecondary)
                Spacer()
            }
            .padding(14)
        }
    }

    // MARK: Pieces

    private func group<Content: View>(_ title: String, @ViewBuilder content: () -> Content) -> some View {
        VStack(alignment: .leading, spacing: 0) {
            SectionLabel(text: title)
            VStack(alignment: .leading, spacing: 0) { content() }
                .frame(maxWidth: .infinity, alignment: .leading)
                .ringCard()
        }
    }

    private func rowButton(_ title: String, symbol: String, destructive: Bool = false, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            HStack {
                Text(title).font(RingFont.ui(15)).foregroundStyle(destructive ? Palette.danger : Palette.accentText)
                Spacer()
                Image(systemName: symbol).font(.system(size: 13, weight: .light)).foregroundStyle(Palette.textMuted)
            }
            .padding(.horizontal, 14)
            .frame(minHeight: 48)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
    }
}

/// A settings row with a title, a muted explanation and a trailing switch.
struct SettingToggle: View {
    var title: String
    var detail: String?
    @Binding var isOn: Bool

    var body: some View {
        HStack(spacing: 12) {
            VStack(alignment: .leading, spacing: 2) {
                Text(title).font(RingFont.ui(15)).foregroundStyle(Palette.foreground)
                if let detail {
                    Text(detail).font(RingFont.ui(13)).foregroundStyle(Palette.textMuted)
                        .fixedSize(horizontal: false, vertical: true)
                }
            }
            Spacer(minLength: 8)
            Toggle(title, isOn: $isOn)
                .labelsHidden()
                .toggleStyle(.switch)
                .tint(Palette.accent)
        }
        .padding(.horizontal, 14)
        .padding(.vertical, 10)
        .accessibilityElement(children: .combine)
    }
}

/// One voicemail greeting with its reset. `nil` means Ring's built-in greeting.
struct GreetingEditor: View {
    var reason: VoicemailReason
    var text: String?
    var limit: Int
    /// Ring's built-in greetings say the owner's name, as ring-server speaks them.
    var ownerName: String
    var save: (String?) -> Void
    @State private var draft = ""
    @FocusState private var focused: Bool

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack {
                Text(title).pixelLabel(11)
                Spacer()
                if text != nil {
                    Button("Reset") { draft = ""; save(nil) }
                        .buttonStyle(RingButtonStyle(kind: .ghost, small: true))
                }
            }
            TextField("", text: $draft, prompt: Text(placeholder).foregroundStyle(Palette.textMuted), axis: .vertical)
                .font(RingFont.ui(15))
                .foregroundStyle(Palette.foreground)
                .textFieldStyle(.plain)
                .lineLimit(1...5)
                .focused($focused)
                .onSubmit(commit)
                .onChange(of: focused) { _, isFocused in if !isFocused { commit() } }
            if focused {
                Text("\(draft.unicodeScalars.count)/\(limit)")
                    .pixelLabel(10, color: draft.unicodeScalars.count > limit ? Palette.danger : Palette.textMuted)
            }
        }
        .padding(14)
        .onAppear { draft = text ?? "" }
        .onChange(of: text) { _, new in if !focused { draft = new ?? "" } }
    }

    private var title: String {
        switch reason {
        case .busy: "Busy greeting"
        case .declined: "Declined greeting"
        case .missed: "Missed greeting"
        case .unknown: "Greeting"
        }
    }

    private var placeholder: String {
        switch reason {
        case .busy: "Default: \(ownerName) is on another call. Leave a message after the beep."
        case .declined: "Default: \(ownerName) can't take your call right now. Leave a message after the beep."
        case .missed: "Default: \(ownerName) isn't available right now. Leave a message after the beep."
        case .unknown: ""
        }
    }

    private func commit() {
        let trimmed = draft.trimmingCharacters(in: .whitespacesAndNewlines)
        guard trimmed.unicodeScalars.count <= limit else { return }
        let value: String? = trimmed.isEmpty ? nil : trimmed
        if value != text { save(value) }
    }
}

/// Confirms deleting the Ring account, and says what is deleted and what stays.
struct DeleteAccountView: View {
    /// While the deletion is in flight; Settings keeps the sheet up until it is over.
    @Binding var working: Bool
    @Environment(AppModel.self) private var model
    @Environment(\.dismiss) private var dismiss
    @Environment(\.openURL) private var openURL
    @Environment(\.colorScheme) private var colorScheme
    @State private var error: String?

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 16) {
                Text(Labels.DeleteAccount.title).font(RingFont.display(24)).foregroundStyle(Palette.ink)
                VStack(alignment: .leading, spacing: 8) {
                    paragraph("This permanently deletes:")
                    item("Your call history, with its transcripts and recordings")
                    item("Voicemails left for you")
                    item("Your greetings, settings and the people you blocked")
                    item("Your devices. They are all signed out, and any call you're on ends.")
                }
                paragraph("The people you talked with keep their copies of the calls you shared. A call is deleted for everyone once nobody on it still uses Ring.")
                paragraph("Ring doesn't delete your Silicon Account. Silicon Accounts is the separate service you sign in with, like Sign in with Google, and other apps may use it too. If you sign in to Ring with it again later, you start with an empty Ring account.")
                paragraph("To delete your Silicon Account as well, do that at \(accountsHost).")
                Button {
                    openURL(accountSettingsURL)
                } label: {
                    HStack(spacing: 6) {
                        Text("Open \(accountsHost)")
                        Image(systemName: "arrow.up.right").font(.system(size: 12, weight: .light))
                    }
                }
                .buttonStyle(RingButtonStyle(kind: .ghost, small: true))
                .padding(.leading, -10)
                if let error {
                    Text(error)
                        .font(RingFont.ui(14))
                        .foregroundStyle(Palette.danger)
                        .fixedSize(horizontal: false, vertical: true)
                }
                if Metrics.isTouch {
                    VStack(spacing: 10) {
                        deleteButton.buttonStyle(RingButtonStyle(kind: .danger, fullWidth: true))
                        cancelButton.buttonStyle(RingButtonStyle(kind: .ghost, fullWidth: true))
                    }
                    .padding(.top, 4)
                } else {
                    HStack {
                        Spacer()
                        cancelButton.buttonStyle(.ringGhost)
                        deleteButton.buttonStyle(RingButtonStyle(kind: .danger))
                    }
                }
            }
            .padding(20)
        }
        #if os(macOS)
        // A sheet takes its content's ideal size, and a ScrollView has no ideal height of its own.
        .frame(minWidth: 420, idealWidth: 460, minHeight: 500, idealHeight: 580)
        #endif
        .background(Palette.background.ignoresSafeArea())
        .interactiveDismissDisabled(working)
        .presentationDetents([.large])
    }

    private var cancelButton: some View {
        Button(Labels.cancel) { dismiss() }
            .disabled(working)
    }

    private var deleteButton: some View {
        Button {
            Task { await delete() }
        } label: {
            HStack(spacing: 8) {
                if working {
                    ProgressView().controlSize(.small).tint(Palette.onStatus(colorScheme))
                }
                Text(Labels.DeleteAccount.confirm)
            }
        }
        .disabled(working)
    }

    /// Where a Silicon Account is deleted.
    private var accountSettingsURL: URL { model.accountsURL.appending(path: "settings") }

    private var accountsHost: String { model.accountsURL.host() ?? "accounts.teamofsilicons.com" }

    private func paragraph(_ text: String) -> some View {
        Text(text)
            .font(RingFont.ui(15))
            .foregroundStyle(Palette.foreground)
            .fixedSize(horizontal: false, vertical: true)
    }

    private func item(_ text: String) -> some View {
        HStack(alignment: .firstTextBaseline, spacing: 8) {
            Text("•").foregroundStyle(Palette.textMuted)
            Text(text)
                .foregroundStyle(Palette.textSecondary)
                .fixedSize(horizontal: false, vertical: true)
        }
        .font(RingFont.ui(15))
    }

    private func delete() async {
        working = true
        error = nil
        defer { working = false }
        do {
            try await model.deleteAccount()
            dismiss()
        } catch let failure as RingError {
            if failure.isSessionInvalid {
                model.handle(failure)
                return
            }
            if case .cancelled = failure { return }
            error = [failure.errorDescription, failure.recoverySuggestion].compactMap { $0 }.joined(separator: " ")
        } catch {
            self.error = error.localizedDescription
        }
    }
}

#if DEBUG
/// The developer setting, in Debug builds only: which Ring server this app talks to.
struct ServerSettingsView: View {
    @Environment(AppModel.self) private var model
    @Environment(\.dismiss) private var dismiss
    @State private var text = ServerConfig.current.absoluteString
    @State private var invalid = false

    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            Text("Ring server").font(RingFont.display(24)).foregroundStyle(Palette.ink)
            Text("Developer setting. Changing the server signs you out, because sessions belong to the server that issued them.")
                .font(RingFont.ui(14))
                .foregroundStyle(Palette.textSecondary)
                .fixedSize(horizontal: false, vertical: true)
            TextField("https://backend.ring.teamofsilicons.com", text: $text)
                .font(RingFont.ui(15))
                .textFieldStyle(.plain)
                .autocorrectionDisabled()
                #if os(iOS)
                .textInputAutocapitalization(.never)
                .keyboardType(.URL)
                #endif
                .padding(.horizontal, 12)
                .frame(height: Metrics.controlHeight)
                .background(RoundedRectangle(cornerRadius: 8, style: .continuous).fill(Palette.surfaceMuted))
                .overlay(RoundedRectangle(cornerRadius: 8, style: .continuous).strokeBorder(invalid ? Palette.danger : Palette.borderStrong, lineWidth: 0.5))
            Text("Default: \(ServerConfig.defaultURL.absoluteString)").pixelLabel(10)
            HStack {
                Button("Use default") {
                    Task {
                        await model.setServerOverride(nil)
                        dismiss()
                    }
                }
                .buttonStyle(.ringSecondary)
                Spacer()
                Button("Cancel") { dismiss() }.buttonStyle(.ringGhost)
                Button("Save") {
                    guard let url = ServerConfig.normalize(text) else {
                        invalid = true
                        return
                    }
                    Task {
                        await model.setServerOverride(url == ServerConfig.defaultURL ? nil : url)
                        dismiss()
                    }
                }
                .buttonStyle(.ringPrimary)
            }
        }
        .padding(20)
        .frame(minWidth: 380)
        .background(Palette.background.ignoresSafeArea())
        .presentationDetents([.medium])
    }
}
#endif
