import SwiftUI

/// After sign-in, and before a call until this device agrees: Ring records and transcribes calls,
/// and which providers process the audio. "Agree and continue" carries on with the call; "Not now"
/// drops it. Over a call already on (`.callScreen`), it covers the people and the transcript and
/// leaves the call controls below it usable.
struct RecordingNoticeView: View {
    var request: RecordingNoticeRequest
    var placement: RecordingNoticePlacement = .window
    @Environment(AppModel.self) private var model
    @Environment(\.openURL) private var openURL

    var body: some View {
        ZStack {
            Color.black.opacity(0.4)
                .ignoresSafeArea(edges: placement == .window ? .all : [.top, .horizontal])
                .contentShape(Rectangle())
                // Taps outside the card don't reach the screen underneath.
                .onTapGesture {}
                .accessibilityHidden(true)
            ViewThatFits(in: .vertical) {
                card.padding(16)
                ScrollView { card.padding(16) }
                    .scrollIndicators(.hidden)
            }
            .frame(maxWidth: 480)
        }
    }

    private var card: some View {
        VStack(alignment: .leading, spacing: 16) {
            HStack(spacing: 8) {
                Circle().fill(Palette.danger).frame(width: 8, height: 8)
                Text(Labels.Recording.indicator).pixelLabel(11)
            }
            .accessibilityHidden(true)
            Text(Labels.Recording.noticeTitle)
                .font(RingFont.display(26))
                .foregroundStyle(Palette.ink)
                .accessibilityAddTraits(.isHeader)
            if let context {
                Text(context)
                    .font(RingFont.ui(15, .medium))
                    .foregroundStyle(Palette.foreground)
                    .fixedSize(horizontal: false, vertical: true)
            }
            paragraph("Every call is recorded and transcribed, so the people on it can read it and play it back later. Everyone on a call sees the same transcript.")
            VStack(alignment: .leading, spacing: 10) {
                item("text.bubble", "Deepgram transcribes what people say.")
                item("waveform", "When a silicon is on the call, OpenAI produces its voice and hears the call.")
            }
            Text("Some places require everyone's consent to record a call. Let the people you call know.")
                .font(RingFont.ui(13))
                .foregroundStyle(Palette.textMuted)
                .fixedSize(horizontal: false, vertical: true)
            Button {
                openURL(AppInfo.privacyURL)
            } label: {
                HStack(spacing: 6) {
                    Text(Labels.Recording.privacyPolicy)
                    Image(systemName: "arrow.up.right").font(.system(size: 12, weight: .light))
                }
            }
            .buttonStyle(RingButtonStyle(kind: .ghost, small: true))
            .padding(.leading, -10)
            buttons
                .padding(.top, 4)
        }
        .padding(20)
        .frame(maxWidth: .infinity, alignment: .leading)
        .ringRaised(radius: 16)
        .accessibilityElement(children: .contain)
        // Over a call, VoiceOver can still reach the call controls.
        .accessibilityAddTraits(placement == .window ? .isModal : [])
    }

    /// For a call that connected before this device agreed.
    private var context: String? {
        guard request.action.isAnswered else { return nil }
        return placement == .callScreen
            ? "This call is recorded and transcribed, like every Ring call."
            : "Your last call was recorded and transcribed, like every Ring call."
    }

    @ViewBuilder
    private var buttons: some View {
        if Metrics.isTouch {
            VStack(spacing: 10) {
                agree.buttonStyle(RingButtonStyle(kind: .primary, fullWidth: true))
                notNow.buttonStyle(RingButtonStyle(kind: .ghost, fullWidth: true))
            }
        } else {
            HStack(spacing: 10) {
                Spacer()
                notNow.buttonStyle(.ringGhost)
                    .keyboardShortcut(.cancelAction)
                agree.buttonStyle(.ringPrimary)
                    .keyboardShortcut(.defaultAction)
            }
        }
    }

    private var agree: some View {
        Button(Labels.Recording.agree) { model.agreeToRecordingNotice() }
    }

    private var notNow: some View {
        Button(Labels.Recording.notNow) { model.dismissRecordingNotice() }
            .accessibilityHint(notNowHint)
    }

    private var notNowHint: String {
        switch request.action {
        case .welcome: "Asks again before your first call"
        case .place: "Doesn't place the call"
        case .answer: "Doesn't answer the call"
        case .switchHere: "Leaves the call on your other device"
        case .answered: placement == .callScreen ? "Closes the notice. The call goes on." : "Closes the notice"
        }
    }

    private func paragraph(_ text: String) -> some View {
        Text(text)
            .font(RingFont.ui(15))
            .foregroundStyle(Palette.foreground)
            .fixedSize(horizontal: false, vertical: true)
    }

    private func item(_ symbol: String, _ text: String) -> some View {
        HStack(alignment: .firstTextBaseline, spacing: 10) {
            Image(systemName: symbol)
                .font(.system(size: 13, weight: .light))
                .foregroundStyle(Palette.accentText)
                .frame(width: 18)
            Text(text)
                .font(RingFont.ui(15))
                .foregroundStyle(Palette.textSecondary)
                .fixedSize(horizontal: false, vertical: true)
        }
    }
}

/// On every call screen: the call is recorded and transcribed.
struct RecordingIndicator: View {
    var onMoment = false

    var body: some View {
        HStack(spacing: 6) {
            Circle().fill(Palette.danger).frame(width: 6, height: 6)
            Text(Labels.Recording.indicator).pixelLabel(10, color: onMoment ? Palette.momentMuted : Palette.textMuted)
        }
        .accessibilityElement(children: .ignore)
        .accessibilityLabel("This call is recorded and transcribed")
    }
}
