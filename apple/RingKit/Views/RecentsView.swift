import SwiftUI

struct RecentsView: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 20) {
                PageTitle(text: "Recents")
                    .padding(.top, 8)
                if !model.recentsLoaded {
                    ProgressView()
                        .frame(maxWidth: .infinity)
                        .padding(.vertical, 48)
                } else if model.recents.isEmpty {
                    EmptyStateView(message: "No calls yet", detail: "Call a carbon or a silicon by their id, like c:alice or si:scout.")
                } else {
                    LazyVStack(spacing: 0) {
                        ForEach(Array(model.recents.enumerated()), id: \.element.ringid) { index, call in
                            if index > 0 { Hairline().padding(.leading, 64) }
                            RecentRow(call: call)
                                .onAppear {
                                    if index == model.recents.count - 1 { Task { await model.loadMoreRecents() } }
                                }
                        }
                    }
                    .ringCard()
                }
            }
            .padding(.horizontal, 16)
            .padding(.bottom, 96)
            .frame(maxWidth: 760)
            .frame(maxWidth: .infinity)
        }
        .background(Palette.background)
        .refreshable { await model.refreshRecents() }
        .homeScreenChrome()
        .navigationTitle("Recents")
    }
}

struct RecentRow: View {
    var call: Call
    @Environment(AppModel.self) private var model

    var body: some View {
        let person = call.counterpart(me: model.myUUID)
        let missed = Format.isMissed(call, me: model.myUUID)
        HStack(spacing: 12) {
            NavigationLink(value: Route.call(call)) {
                HStack(spacing: 12) {
                    AvatarView(account: person, size: 40)
                    VStack(alignment: .leading, spacing: 4) {
                        HStack(spacing: 8) {
                            Text(person.label)
                                .font(RingFont.ui(16, .medium))
                                .foregroundStyle(missed ? Palette.danger : Palette.foreground)
                                .lineLimit(1)
                            KindBadge(account: person)
                        }
                        HStack(spacing: 6) {
                            Image(systemName: directionSymbol(missed: missed))
                                .font(.system(size: 11, weight: .regular))
                                .foregroundStyle(missed ? Palette.danger : Palette.textMuted)
                            Text("\(Format.when(call.createdAt)) · \(Format.outcome(of: call, me: model.myUUID))")
                                .pixelLabel(11, color: missed ? Palette.danger : Palette.textMuted)
                                .lineLimit(1)
                        }
                    }
                    Spacer(minLength: 4)
                }
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)

            Button {
                model.placeCall(to: person.uuid)
            } label: {
                Image(systemName: "phone")
                    .font(.system(size: 16, weight: .light))
                    .foregroundStyle(Palette.accentText)
                    .frame(width: 40, height: 40)
                    .contentShape(Circle())
            }
            .buttonStyle(PressScaleStyle())
            .accessibilityLabel("Call \(person.label)")
        }
        .padding(.horizontal, 14)
        .padding(.vertical, 10)
        .accessibilityElement(children: .contain)
    }

    private func directionSymbol(missed: Bool) -> String {
        if missed { return "phone.arrow.down.left" }
        return call.direction == .incoming ? "arrow.down.left" : "arrow.up.right"
    }
}

// MARK: - Call details

struct CallDetailView: View {
    @State var call: Call
    @Environment(AppModel.self) private var model
    @State private var entries: [TranscriptEntry] = []
    @State private var recording: Recording?
    @State private var loading = true

    var body: some View {
        let person = call.counterpart(me: model.myUUID)
        ScrollView {
            VStack(alignment: .leading, spacing: 24) {
                header(person)
                participants
                if let recording, recording.status == .ready, let string = recording.url, let url = URL(string: string) {
                    VStack(alignment: .leading, spacing: 0) {
                        SectionLabel(text: "Recording")
                        AudioPlayerView(url: url, durationMS: recording.durationMS)
                            .padding(14)
                            .ringCard()
                    }
                } else if call.recording.status == .processing || call.recording.status == .recording {
                    VStack(alignment: .leading, spacing: 0) {
                        SectionLabel(text: "Recording")
                        Text("Processing the recording…")
                            .font(RingFont.ui(14))
                            .foregroundStyle(Palette.textMuted)
                            .padding(14)
                            .frame(maxWidth: .infinity, alignment: .leading)
                            .ringCard()
                    }
                }
                timeline
                transcript
            }
            .padding(16)
            .frame(maxWidth: 760)
            .frame(maxWidth: .infinity)
        }
        .background(Palette.background)
        .navigationTitle(person.label)
        #if os(iOS)
        .navigationBarTitleDisplayMode(.inline)
        #endif
        .task { await load() }
    }

    private func header(_ person: Account) -> some View {
        HStack(alignment: .center, spacing: 16) {
            AvatarView(account: person, size: 64)
            VStack(alignment: .leading, spacing: 6) {
                Text(person.label)
                    .font(RingFont.display(28))
                    .foregroundStyle(Palette.ink)
                HStack(spacing: 8) {
                    Text(person.id).pixelID(12)
                    KindBadge(account: person)
                }
                Text("\(Format.timestamp(call.createdAt)) · \(Format.outcome(of: call, me: model.myUUID))")
                    .pixelLabel(11, color: Format.isMissed(call, me: model.myUUID) ? Palette.danger : Palette.textMuted)
            }
            Spacer()
            Button {
                model.placeCall(to: person.uuid)
            } label: {
                Label("Call", systemImage: "phone")
            }
            .buttonStyle(.ringPrimary)
        }
    }

    private var participants: some View {
        VStack(alignment: .leading, spacing: 0) {
            SectionLabel(text: "People")
            VStack(spacing: 0) {
                let people = call.others(me: model.myUUID)
                ForEach(Array(people.enumerated()), id: \.element.uuid) { index, account in
                    if index > 0 { Hairline().padding(.leading, 56) }
                    HStack(spacing: 12) {
                        AvatarView(account: account, size: 32)
                        VStack(alignment: .leading, spacing: 2) {
                            Text(account.label).font(RingFont.ui(15, .medium)).foregroundStyle(Palette.foreground)
                            Text(account.id).pixelID(10)
                        }
                        Spacer()
                        KindBadge(account: account)
                        if let invitation = call.invitations.last(where: { $0.to.uuid == account.uuid }) {
                            Badge(text: invitationLabel(invitation))
                        }
                    }
                    .padding(.horizontal, 14)
                    .padding(.vertical, 10)
                }
            }
            .ringCard()
        }
    }

    private func invitationLabel(_ invitation: Invitation) -> String {
        switch invitation.state {
        case .accepted: "Answered"
        case .declined: "Declined"
        case .missed: "Missed"
        case .cancelled: "Cancelled"
        case .busy: "Busy"
        case .ringing: "Ringing"
        case .unknown: "—"
        }
    }

    @ViewBuilder
    private var timeline: some View {
        let events = entries.filter { $0.kind != .speech && !$0.kind.isPrivate }
        if !events.isEmpty {
            VStack(alignment: .leading, spacing: 0) {
                SectionLabel(text: "Timeline")
                VStack(alignment: .leading, spacing: 0) {
                    ForEach(Array(events.enumerated()), id: \.element.seq) { index, entry in
                        HStack(alignment: .top, spacing: 12) {
                            VStack(spacing: 0) {
                                Rectangle().fill(index == 0 ? .clear : Palette.borderStrong).frame(width: 0.5, height: 8)
                                Circle().fill(dotColor(entry)).frame(width: 7, height: 7)
                                Rectangle().fill(index == events.count - 1 ? .clear : Palette.borderStrong).frame(width: 0.5)
                                    .frame(maxHeight: .infinity)
                            }
                            .frame(width: 8)
                            VStack(alignment: .leading, spacing: 2) {
                                Text(Format.clock(entry.at)).pixelLabel(10)
                                Text(timelineText(entry))
                                    .font(RingFont.ui(14))
                                    .foregroundStyle(Palette.foreground)
                                    .fixedSize(horizontal: false, vertical: true)
                            }
                            .padding(.bottom, 12)
                        }
                    }
                }
                .padding(14)
                .ringCard()
            }
        }
    }

    private func dotColor(_ entry: TranscriptEntry) -> Color {
        switch entry.kind {
        case .answered, .joined: Palette.success
        case .declined, .missed, .busy: Palette.danger
        case .voicemail: Palette.accent
        default: Palette.textMuted
        }
    }

    private func timelineText(_ entry: TranscriptEntry) -> String {
        let who = entry.actor.map { $0.uuid == model.myUUID ? "You" : $0.label } ?? "Ring"
        switch entry.kind {
        case .callStarted: return "\(who) started the call"
        case .answered:
            if let after = entry.data["after_ms"]?.doubleValue { return "\(who) answered after \(Format.duration(ms: Int(after)))" }
            return "\(who) answered"
        case .joined: return "\(who) joined"
        case .left: return "\(who) left"
        case .invited:
            let invitee = entry.data["invitee"].flatMap { $0["display_name"]?.stringValue ?? $0["id"]?.stringValue } ?? "someone"
            return "\(who) invited \(invitee)"
        case .declined:
            if let reason = entry.data["reason"]?.stringValue, !reason.isEmpty { return "\(who) declined · \(reason)" }
            return "\(who) declined"
        case .missed: return "\(who) missed the call"
        case .busy: return "\(who) was busy"
        case .voicemail: return "\(who) left a voicemail"
        case .callEnded: return "Call ended"
        case .note: return entry.text ?? "Note"
        default: return entry.text ?? entry.kind.rawValue
        }
    }

    @ViewBuilder
    private var transcript: some View {
        let speech = entries.filter { $0.kind == .speech }
        VStack(alignment: .leading, spacing: 0) {
            SectionLabel(text: "Transcript")
            Group {
                if loading {
                    ProgressView().frame(maxWidth: .infinity).padding(24)
                } else if speech.isEmpty {
                    Text("Nothing was said on this call.")
                        .font(RingFont.ui(14))
                        .foregroundStyle(Palette.textMuted)
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .padding(14)
                } else {
                    VStack(alignment: .leading, spacing: 14) {
                        ForEach(speech) { entry in
                            VStack(alignment: .leading, spacing: 4) {
                                HStack(spacing: 8) {
                                    Text(entry.actor.map { $0.uuid == model.myUUID ? "You" : $0.label } ?? "Someone")
                                        .pixelLabel(11)
                                    if let offset = entry.offsetMS {
                                        Text(Format.duration(ms: offset)).pixelLabel(10, color: Palette.textMuted.opacity(0.7))
                                    }
                                }
                                Text(entry.text ?? "")
                                    .font(RingFont.ui(16))
                                    .lineSpacing(4)
                                    .foregroundStyle(Palette.foreground)
                                    .textSelection(.enabled)
                                    .fixedSize(horizontal: false, vertical: true)
                            }
                        }
                    }
                    .padding(14)
                    .frame(maxWidth: .infinity, alignment: .leading)
                }
            }
            .ringCard()
        }
    }

    private func load() async {
        defer { loading = false }
        if let fresh = try? await model.api.call(call.ringid) { call = fresh }
        if let all = try? await model.api.fullTranscript(call.ringid) { entries = all }
        if call.recording.status == .ready || call.recording.status == .processing {
            recording = try? await model.api.recording(call.ringid)
        }
    }
}

extension EntryKind {
    var isPrivate: Bool { self == .delegation || self == .thinking || self == .commentary }
}
