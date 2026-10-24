import SwiftUI

struct VoicemailListView: View {
    @Environment(AppModel.self) private var model
    @State private var playback = AudioPlayback()
    @State private var playingID: String?
    @State private var loadingID: String?
    @State private var confirmBlock: Account?

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 20) {
                PageTitle(text: "Voicemail")
                    .padding(.top, 8)
                if !model.voicemailsLoaded {
                    ProgressView().frame(maxWidth: .infinity).padding(.vertical, 48)
                } else if model.voicemails.isEmpty {
                    EmptyStateView(message: "No voicemail", detail: "When you miss or decline a call, messages land here, transcribed.")
                } else {
                    LazyVStack(spacing: 0) {
                        ForEach(Array(model.voicemails.enumerated()), id: \.element.voicemailID) { index, voicemail in
                            if index > 0 { Hairline().padding(.leading, 30) }
                            row(voicemail)
                                .onAppear {
                                    if index == model.voicemails.count - 1 { Task { await model.loadMoreVoicemails() } }
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
        .refreshable { await model.refreshVoicemails() }
        .homeScreenChrome()
        .navigationTitle("Voicemail")
        .blockConfirmation($confirmBlock) { account in
            Task { await model.block(account) }
        }
        .onDisappear { playback.stop(); playingID = nil }
    }

    private func row(_ voicemail: Voicemail) -> some View {
        HStack(alignment: .top, spacing: 12) {
            Circle()
                .fill(voicemail.read ? .clear : Palette.accent)
                .frame(width: 8, height: 8)
                .padding(.top, 8)
                .accessibilityLabel(voicemail.read ? "" : "Unread")
            NavigationLink(value: Route.voicemail(voicemail)) {
                VStack(alignment: .leading, spacing: 4) {
                    HStack(spacing: 8) {
                        Text(voicemail.from.label)
                            .font(RingFont.ui(16, voicemail.read ? .regular : .medium))
                            .foregroundStyle(Palette.foreground)
                            .lineLimit(1)
                        KindBadge(account: voicemail.from)
                        Spacer(minLength: 4)
                        Text(Format.when(voicemail.createdAt)).pixelLabel(11)
                    }
                    Text([Format.reason(of: voicemail), voicemail.durationMS.map { Format.duration(ms: $0) }].compactMap { $0 }.joined(separator: " · "))
                        .pixelLabel(11)
                    Text(preview(voicemail))
                        .font(RingFont.ui(14))
                        .foregroundStyle(Palette.textSecondary)
                        .lineLimit(2)
                        .frame(maxWidth: .infinity, alignment: .leading)
                }
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)

            Button {
                Task { await togglePlay(voicemail) }
            } label: {
                Group {
                    if loadingID == voicemail.voicemailID {
                        ProgressView().controlSize(.small)
                    } else {
                        Image(systemName: playingID == voicemail.voicemailID && playback.isPlaying ? "pause.fill" : "play.fill")
                            .font(.system(size: 13))
                    }
                }
                .foregroundStyle(Palette.accentText)
                .frame(width: 36, height: 36)
                .background(Circle().fill(Palette.accentSubtle))
                .overlay(Circle().strokeBorder(Palette.accentBorder, lineWidth: 0.5))
            }
            .buttonStyle(PressScaleStyle())
            .disabled(voicemail.audioStatus != .ready)
            .opacity(voicemail.audioStatus == .ready ? 1 : 0.4)
            .accessibilityLabel("Play voicemail from \(voicemail.from.label)")
        }
        .padding(.horizontal, 14)
        .padding(.vertical, 12)
        .contextMenu {
            Button("Call back", systemImage: "phone") { model.placeCall(to: voicemail.from.uuid) }
            Button("Delete", systemImage: "trash", role: .destructive) { Task { await model.delete(voicemail) } }
            Divider()
            PersonSafetyActions(account: voicemail.from, ringid: voicemail.ringid, confirmBlock: $confirmBlock)
        }
    }

    private func preview(_ voicemail: Voicemail) -> String {
        switch voicemail.transcriptStatus {
        case .ready: voicemail.transcript?.isEmpty == false ? voicemail.transcript! : "No words in this message."
        case .pending: voicemail.audioStatus == .recording ? "Recording…" : "Transcribing…"
        case .failed, .unknown: "No transcript."
        }
    }

    private func togglePlay(_ voicemail: Voicemail) async {
        if playingID == voicemail.voicemailID {
            playback.toggle()
            return
        }
        loadingID = voicemail.voicemailID
        defer { loadingID = nil }
        guard let full = try? await model.api.voicemail(voicemail.voicemailID),
              let string = full.audioURL, let url = URL(string: string) else {
            model.show("This voicemail can't be played right now.", style: .error)
            return
        }
        playback.stop()
        playback.load(url)
        playingID = voicemail.voicemailID
        playback.toggle()
        await model.markRead(voicemail)
    }
}

struct VoicemailDetailView: View {
    @State var voicemail: Voicemail
    @Environment(AppModel.self) private var model
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 24) {
                HStack(spacing: 16) {
                    AvatarView(account: voicemail.from, size: 64)
                    VStack(alignment: .leading, spacing: 6) {
                        Text(voicemail.from.label)
                            .font(RingFont.display(28))
                            .foregroundStyle(Palette.ink)
                        HStack(spacing: 8) {
                            Text(voicemail.from.id).pixelID(12)
                            KindBadge(account: voicemail.from)
                        }
                        Text("\(Format.reason(of: voicemail)) · \(Format.timestamp(voicemail.createdAt))").pixelLabel(11)
                    }
                    Spacer()
                }

                Group {
                    if voicemail.audioStatus == .ready, let string = voicemail.audioURL, let url = URL(string: string) {
                        AudioPlayerView(url: url, durationMS: voicemail.durationMS)
                    } else {
                        Text(voicemail.audioStatus == .failed ? "The audio couldn't be saved." : "Preparing the audio…")
                            .font(RingFont.ui(14))
                            .foregroundStyle(Palette.textMuted)
                            .frame(maxWidth: .infinity, alignment: .leading)
                    }
                }
                .padding(14)
                .ringCard()

                VStack(alignment: .leading, spacing: 0) {
                    SectionLabel(text: "Transcript")
                    Text(transcript)
                        .font(RingFont.ui(16))
                        .lineSpacing(4)
                        .foregroundStyle(Palette.foreground)
                        .textSelection(.enabled)
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .padding(14)
                        .ringCard()
                }

                HStack(spacing: 12) {
                    Button {
                        model.placeCall(to: voicemail.from.uuid)
                    } label: {
                        Label("Call back", systemImage: "phone")
                    }
                    .buttonStyle(.ringPrimary)
                    Button(role: .destructive) {
                        Task {
                            await model.delete(voicemail)
                            dismiss()
                        }
                    } label: {
                        Label("Delete", systemImage: "trash")
                    }
                    .buttonStyle(.ringSecondary)
                }
            }
            .padding(16)
            .frame(maxWidth: 760)
            .frame(maxWidth: .infinity)
        }
        .background(Palette.background)
        .navigationTitle(voicemail.from.label)
        #if os(iOS)
        .navigationBarTitleDisplayMode(.inline)
        #endif
        .task {
            if let full = try? await model.api.voicemail(voicemail.voicemailID) {
                voicemail = full
                model.upsert(full)
            }
            await model.markRead(voicemail)
        }
    }

    private var transcript: String {
        switch voicemail.transcriptStatus {
        case .ready: voicemail.transcript?.isEmpty == false ? voicemail.transcript! : "No words in this message."
        case .pending: "Transcribing…"
        case .failed, .unknown: "No transcript is available."
        }
    }
}
