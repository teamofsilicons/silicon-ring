import SwiftUI
#if os(macOS)
import AppKit
#endif

enum HomeSection: String, CaseIterable, Identifiable, Hashable {
    case recents, voicemail, settings
    var id: String { rawValue }

    var title: String {
        switch self {
        case .recents: "Recents"
        case .voicemail: "Voicemail"
        case .settings: "Settings"
        }
    }

    var symbol: String {
        switch self {
        case .recents: "clock"
        case .voicemail: "recordingtape"
        case .settings: "gearshape"
        }
    }

    /// The section the home screen opens on: the one the user last chose.
    static var remembered: HomeSection {
        get { HomeSection(rawValue: UserDefaults.standard.string(forKey: "ring.home.section") ?? "") ?? .recents }
        set { UserDefaults.standard.set(newValue.rawValue, forKey: "ring.home.section") }
    }
}

enum Route: Hashable {
    case call(Call)
    case voicemail(Voicemail)
}

/// Sign-in or home, plus the call screen and banners that float above both.
struct RootView: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        Group {
            if model.isSignedIn {
                HomeView()
            } else {
                SignInView()
            }
        }
        // The call screen is an overlay rather than a modal so it can never be blocked by
        // another presentation; HomeView closes its sheets when a call screen appears.
        .overlay {
            if showsCallScreen {
                CallScreen()
                    .transition(.move(edge: .bottom).combined(with: .opacity))
            }
        }
        .animation(.smooth(duration: 0.3), value: showsCallScreen)
        .overlay(alignment: .top) {
            VStack(spacing: 8) {
                if let notice = model.notice {
                    NoticeBanner(notice: notice)
                        .transition(.move(edge: .top).combined(with: .opacity))
                }
            }
            .padding(.top, 8)
            .animation(.smooth(duration: 0.25), value: model.notice)
        }
        #if os(iOS)
        .preferredColorScheme(model.appearance.colorScheme)
        #else
        .onChange(of: model.appearance, initial: true) { _, appearance in
            // App-wide, so the ringing panel and menus follow too.
            NSApp.appearance = switch appearance {
            case .system: nil
            case .light: NSAppearance(named: .aqua)
            case .dark: NSAppearance(named: .darkAqua)
            }
        }
        #endif
        .tint(Palette.accent)
    }

    private var showsCallScreen: Bool {
        guard model.isSignedIn else { return false }
        if let session = model.activeCall, !session.minimized { return true }
        return model.inAppRing != nil
    }
}

struct HomeView: View {
    @Environment(AppModel.self) private var model
    #if os(iOS)
    @Environment(\.horizontalSizeClass) private var sizeClass
    #endif
    @State private var section: HomeSection? = HomeSection.remembered
    @State private var dialer: DialerMode?
    /// The split view's detail stack; choosing a sidebar section starts it again at that section's root.
    @State private var detailPath = NavigationPath()

    var body: some View {
        content
            .background(Palette.background.ignoresSafeArea())
            .sheet(item: $dialer) { mode in
                DialerView(mode: mode)
            }
            .environment(\.openDialer, OpenDialerAction { dialer = $0 })
            .onChange(of: model.activeCall?.ringid) { _, ringid in if ringid != nil { dialer = nil } }
            .onChange(of: model.inAppRing?.invitationID) { _, id in if id != nil { dialer = nil } }
            .onChange(of: model.requestedSection, initial: true) { _, requested in
                guard let requested else { return }
                section = requested
                #if os(iOS)
                if sizeClass == .compact { return }
                #endif
                model.requestedSection = nil
            }
            .onChange(of: section) { _, section in
                HomeSection.remembered = section ?? .recents
                detailPath = NavigationPath()
            }
    }

    @ViewBuilder
    private var content: some View {
        #if os(iOS)
        if sizeClass == .compact {
            VStack(spacing: 0) {
                CallBanners()
                PhoneTabs(dialer: $dialer)
            }
        } else {
            split
        }
        #else
        split
        #endif
    }

    private var split: some View {
        NavigationSplitView {
            Sidebar(selection: $section)
                .navigationSplitViewColumnWidth(min: 200, ideal: 230, max: 280)
        } detail: {
            NavigationStack(path: $detailPath) {
                SectionScreen(section: section ?? .recents)
                    .toolbar {
                        ToolbarItemGroup(placement: Self.newCallPlacement) {
                            #if os(macOS)
                            // Pushes the button to the top-right corner of the detail column.
                            Spacer()
                            #endif
                            Button {
                                dialer = .call
                            } label: {
                                Label("New call", systemImage: "phone")
                                    .labelStyle(.titleAndIcon)
                            }
                            .buttonStyle(RingButtonStyle(kind: .primary, small: true))
                            .keyboardShortcut("n", modifiers: .command)
                        }
                    }
            }
            // The handoff and return-to-call banners belong to the content, below the toolbar.
            .safeAreaInset(edge: .top, spacing: 0) { CallBanners() }
        }
        .navigationSplitViewStyle(.balanced)
    }

    /// The New call button sits top-right on iPad and Mac.
    private static var newCallPlacement: ToolbarItemPlacement {
        #if os(macOS)
        .automatic
        #else
        .primaryAction
        #endif
    }
}

/// The screen for one home section, with its navigation destinations.
struct SectionScreen: View {
    var section: HomeSection

    var body: some View {
        Group {
            switch section {
            case .recents: RecentsView()
            case .voicemail: VoicemailListView()
            case .settings: SettingsView()
            }
        }
        .navigationDestination(for: Route.self) { route in
            switch route {
            case .call(let call): CallDetailView(call: call)
            case .voicemail(let voicemail): VoicemailDetailView(voicemail: voicemail)
            }
        }
    }
}

struct Sidebar: View {
    @Binding var selection: HomeSection?
    @Environment(AppModel.self) private var model
    @Namespace private var pill

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            HStack(spacing: 10) {
                RingMark(size: 26)
                Wordmark(size: 13)
            }
            .padding(.horizontal, 10)
            .padding(.top, 8)
            .padding(.bottom, 18)

            ForEach(HomeSection.allCases) { item in
                Button {
                    withAnimation(.smooth(duration: 0.25)) { selection = item }
                } label: {
                    HStack(spacing: 10) {
                        Image(systemName: item.symbol)
                            .font(.system(size: 15, weight: .light))
                            .frame(width: 20)
                        Text(item.title)
                            .font(RingFont.ui(15, selection == item ? .medium : .regular))
                        Spacer()
                        if item == .voicemail, model.unreadVoicemails > 0 {
                            Text("\(model.unreadVoicemails)")
                                .font(RingFont.pixel(11))
                                .foregroundStyle(Palette.onPrimary)
                                .padding(.horizontal, 6)
                                .frame(minWidth: 20, minHeight: 18)
                                .background(Capsule().fill(Palette.accent))
                        }
                    }
                    .foregroundStyle(selection == item ? Palette.ink : Palette.textSecondary)
                    .padding(.horizontal, 10)
                    .frame(height: 36)
                    .background {
                        if selection == item {
                            RoundedRectangle(cornerRadius: 8, style: .continuous)
                                .fill(Palette.beige)
                                .matchedGeometryEffect(id: "pill", in: pill)
                        }
                    }
                    .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .accessibilityAddTraits(selection == item ? .isSelected : [])
            }
            Spacer()
            if let me = model.me {
                HStack(spacing: 10) {
                    AvatarView(account: me, size: 28)
                    VStack(alignment: .leading, spacing: 2) {
                        Text(me.label)
                            .font(RingFont.ui(14, .medium))
                            .foregroundStyle(Palette.foreground)
                            .lineLimit(1)
                        Text(me.id).pixelID(10)
                    }
                }
                .padding(10)
            }
        }
        .padding(10)
        .frame(maxHeight: .infinity, alignment: .top)
        .background(Palette.surfaceMuted.ignoresSafeArea())
        #if os(macOS)
        .toolbar(removing: .sidebarToggle)
        #endif
    }
}

#if os(iOS)
struct PhoneTabs: View {
    @Binding var dialer: DialerMode?
    @Environment(AppModel.self) private var model
    @State private var tab: HomeSection = .remembered

    var body: some View {
        TabView(selection: $tab) {
            ForEach(HomeSection.allCases) { item in
                NavigationStack {
                    SectionScreen(section: item)
                        .overlay(alignment: .bottomTrailing) {
                            if item != .settings { newCallButton }
                        }
                }
                .tabItem { Label(item.title, systemImage: item.symbol) }
                .badge(item == .voicemail ? model.unreadVoicemails : 0)
                .tag(item)
            }
        }
        .onChange(of: model.requestedSection, initial: true) { _, requested in
            guard let requested else { return }
            tab = requested
            model.requestedSection = nil
        }
        .onChange(of: tab) { _, tab in HomeSection.remembered = tab }
    }

    private var newCallButton: some View {
        Button {
            dialer = .call
        } label: {
            Image(systemName: "phone")
                .font(.system(size: 22, weight: .regular))
                .foregroundStyle(Palette.onPrimary)
                .frame(width: 58, height: 58)
                .background(Circle().fill(Palette.accent))
                .overlay(Circle().strokeBorder(Palette.accentBorder, lineWidth: 0.5))
                .shadow(color: .black.opacity(0.16), radius: 10, y: 6)
        }
        .buttonStyle(PressScaleStyle())
        .padding(20)
        .accessibilityLabel("New call")
    }
}
#endif

// MARK: - Opening the dialer from anywhere

enum DialerMode: Identifiable, Hashable {
    case call
    case invite
    var id: Self { self }
}

struct OpenDialerAction {
    var open: (DialerMode) -> Void
    func callAsFunction(_ mode: DialerMode) { open(mode) }
}

extension EnvironmentValues {
    @Entry var openDialer = OpenDialerAction { _ in }
}

// MARK: - Banners above the home screen

/// The handoff banner, the "moved to…" notice, and the way back to a minimized call.
struct CallBanners: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        VStack(spacing: 0) {
            if let session = model.activeCall, session.isLive, session.minimized {
                ReturnToCallBanner(session: session)
            } else if let other = model.callOnOtherDevice {
                HandoffBanner(call: other.call, device: other.device, moved: model.movedNotice)
            } else if let moved = model.movedNotice {
                bannerRow {
                    Text(moved).font(RingFont.ui(14, .medium)).foregroundStyle(Palette.foreground)
                    Spacer()
                }
            }
        }
        .animation(.smooth(duration: 0.25), value: model.callOnOtherDevice?.call.ringid)
    }

    private func bannerRow<Content: View>(@ViewBuilder content: () -> Content) -> some View {
        HStack(spacing: 10) { content() }
            .padding(.horizontal, 16)
            .padding(.vertical, 10)
            .background(Palette.beige)
            .overlay(alignment: .bottom) { Hairline(color: Palette.borderStrong) }
    }
}

struct HandoffBanner: View {
    var call: Call
    var device: DeviceRef
    /// "Moved to Pixel 8", right after the call left this device.
    var moved: String?
    @Environment(AppModel.self) private var model

    var body: some View {
        let other = call.others(me: model.myUUID).first?.label ?? "someone"
        HStack(spacing: 10) {
            Image(systemName: device.platform?.symbol ?? "iphone")
                .font(.system(size: 15, weight: .light))
                .foregroundStyle(Palette.textSecondary)
            // "On a call with TOS on Pixel 8 · Switch to this iPhone"
            Text(moved ?? "On a call with \(other) on \(model.deviceLabel(device))")
                .font(RingFont.ui(14))
                .foregroundStyle(Palette.foreground)
                .lineLimit(2)
            Spacer(minLength: 8)
            Button("Switch to this \(AppInfo.platform.label)") {
                model.switchHere(call)
            }
            .buttonStyle(RingButtonStyle(kind: .primary, small: true))
        }
        .padding(.horizontal, 16)
        .padding(.vertical, 10)
        .background(Palette.beige)
        .overlay(alignment: .bottom) { Hairline(color: Palette.borderStrong) }
    }
}

struct ReturnToCallBanner: View {
    var session: CallSession

    var body: some View {
        Button {
            withAnimation(.smooth) { session.minimized = false }
        } label: {
            HStack(spacing: 10) {
                Circle().fill(Palette.success).frame(width: 8, height: 8)
                Text("On call with \(session.counterpart.label)")
                    .font(RingFont.ui(14, .medium))
                    .foregroundStyle(Palette.foreground)
                Spacer()
                CallTimer(since: session.answeredAt)
                    .pixelLabel(12, color: Palette.textSecondary)
                Text("Return").font(RingFont.ui(14, .medium)).foregroundStyle(Palette.accentText)
            }
            .padding(.horizontal, 16)
            .padding(.vertical, 10)
            .background(Palette.beige)
            .overlay(alignment: .bottom) { Hairline(color: Palette.borderStrong) }
        }
        .buttonStyle(.plain)
    }
}

/// The pixel-font call timer, counting from `answered_at`.
struct CallTimer: View {
    var since: Date?

    var body: some View {
        if let since {
            TimelineView(.periodic(from: since, by: 1)) { timeline in
                Text(Format.duration(seconds: Int(max(0, timeline.date.timeIntervalSince(since)))))
                    .monospacedDigit()
            }
        } else {
            Text("0:00")
        }
    }
}
