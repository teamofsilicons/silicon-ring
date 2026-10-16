import SwiftUI

/// Whichever call screen this device needs: incoming, outgoing or in-call.
struct CallScreen: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        ZStack {
            if let session = model.activeCall {
                if session.role == .outgoing && session.call.answeredAt == nil && session.phase != .active {
                    OutgoingCallView(session: session)
                } else {
                    InCallView(session: session)
                }
            }
            if let ring = model.inAppRing {
                IncomingCallView(ring: ring)
                    .transition(.move(edge: .bottom).combined(with: .opacity))
                    .zIndex(1)
            }
        }
        .animation(.smooth(duration: 0.3), value: model.inAppRing?.invitationID)
        #if os(iOS)
        // The incoming screen is ink in both themes; a dark-on-ink status bar would vanish.
        .statusBarHidden(model.inAppRing != nil)
        #endif
        #if os(macOS)
        .frame(minWidth: 420, minHeight: 560)
        #endif
    }
}

// MARK: - Incoming

struct IncomingCallView: View {
    var ring: IncomingRing
    @Environment(AppModel.self) private var model
    /// The Mac's ringing panel uses the compact desktop layout.
    var compact = false
    @State private var declining = false
    @State private var reason = ""
    @FocusState private var reasonFocused: Bool

    private static let reasons = ["Can't talk right now", "In a meeting", "Call you back soon", "Text me instead"]

    var body: some View {
        VStack(spacing: 0) {
            if !compact { Spacer(minLength: 24) }
            identity
            Spacer(minLength: compact ? 16 : 24)
            if declining { declineReasons.transition(.opacity.combined(with: .move(edge: .bottom))) }
            controls
                .padding(.top, declining ? 16 : 0)
            Button("Silence") { model.silence(ring) }
                .font(RingFont.ui(15))
                .foregroundStyle(Palette.momentMuted)
                .buttonStyle(.plain)
                .padding(.vertical, compact ? 10 : 16)
                .accessibilityHint("Stops ringing on all your devices without declining")
        }
        .padding(.horizontal, compact ? 20 : 24)
        .padding(.top, compact ? 22 : 0)
        .frame(width: compact ? 340 : nil)
        .frame(maxWidth: compact ? nil : .infinity, maxHeight: compact ? nil : .infinity)
        .background(Palette.momentBackground.ignoresSafeArea())
        .animation(.smooth(duration: 0.25), value: declining)
    }

    private var identity: some View {
        VStack(spacing: compact ? 10 : 16) {
            ZStack {
                ExpandingRings(color: Palette.momentForeground)
                    .frame(width: avatarSize, height: avatarSize)
                AvatarView(account: ring.from, size: avatarSize, onMoment: true)
            }
            .frame(width: avatarSize * 1.9, height: avatarSize * 1.9)
            VStack(spacing: 6) {
                Text(ring.from.label)
                    .font(RingFont.display(compact ? 32 : 36))
                    .foregroundStyle(Palette.momentForeground)
                    .multilineTextAlignment(.center)
                    .lineLimit(2)
                    .minimumScaleFactor(0.6)
                if ring.from.hasDisplayName {
                    Text(ring.from.id).pixelID(13, color: Palette.momentMuted)
                }
            }
            HStack(spacing: 8) {
                KindBadge(account: ring.from, onMoment: true)
                Text("Is calling").pixelLabel(12, color: Palette.momentMuted)
            }
            if !ring.alreadyOnCall.isEmpty {
                Text("With \(ring.alreadyOnCall.joined(separator: ", "))")
                    .pixelLabel(11, color: Palette.momentMuted)
                    .multilineTextAlignment(.center)
            }
        }
        .accessibilityElement(children: .combine)
        .accessibilityLabel("\(ring.from.label) is calling")
    }

    private var avatarSize: CGFloat { compact ? 88 : 120 }

    private var declineReasons: some View {
        VStack(spacing: 10) {
            FlowLayout(spacing: 8) {
                ForEach(Self.reasons, id: \.self) { text in
                    Button(text) { model.decline(ring, reason: text) }
                        .font(RingFont.ui(14))
                        .foregroundStyle(Palette.momentForeground)
                        .padding(.horizontal, 12)
                        .frame(height: 34)
                        .background(Capsule().fill(Palette.momentSurface))
                        .overlay(Capsule().strokeBorder(Palette.momentMuted.opacity(0.35), lineWidth: 0.5))
                        .buttonStyle(PressScaleStyle())
                }
            }
            HStack(spacing: 8) {
                TextField("", text: $reason, prompt: Text("Something else…").foregroundStyle(Palette.momentMuted))
                    .font(RingFont.ui(15))
                    .foregroundStyle(Palette.momentForeground)
                    .textFieldStyle(.plain)
                    .focused($reasonFocused)
                    .submitLabel(.send)
                    .onSubmit(sendReason)
                    .padding(.horizontal, 12)
                    .frame(height: 40)
                    .background(RoundedRectangle(cornerRadius: 8, style: .continuous).fill(Palette.momentSurface))
                Button(action: sendReason) {
                    Image(systemName: "arrow.up")
                        .font(.system(size: 15, weight: .medium))
                        .foregroundStyle(Palette.momentBackground)
                        .frame(width: 40, height: 40)
                        .background(Circle().fill(Palette.momentForeground))
                }
                .buttonStyle(PressScaleStyle())
                .disabled(reason.trimmingCharacters(in: .whitespaces).isEmpty)
                .accessibilityLabel("Decline with this reason")
            }
            Text("Tap decline again to decline without a reason")
                .pixelLabel(10, color: Palette.momentMuted)
        }
        .frame(maxWidth: 420)
    }

    private func sendReason() {
        // The server counts Unicode code points, not characters.
        let trimmed = reason.trimmingCharacters(in: .whitespacesAndNewlines)
        let text = String(String.UnicodeScalarView(trimmed.unicodeScalars.prefix(model.limits.declineReason)))
        guard !text.isEmpty else { return }
        model.decline(ring, reason: text)
    }

    private func declineTapped() {
        if declining {
            model.decline(ring, reason: nil)
        } else {
            declining = true
        }
    }

    @ViewBuilder
    private var controls: some View {
        if Metrics.isTouch {
            HStack(spacing: 18) {
                RoundCallButton(symbol: "phone.down", label: "Decline", kind: .danger, size: 64, action: declineTapped)
                SlideToAnswer { model.answer(ring) }
            }
            .frame(maxWidth: 460)
        } else {
            HStack(spacing: 12) {
                Button(action: declineTapped) {
                    Label("Decline", systemImage: "phone.down")
                }
                .buttonStyle(RingButtonStyle(kind: .danger, fullWidth: true))
                Button {
                    model.answer(ring)
                } label: {
                    Label("Accept", systemImage: "phone")
                }
                .buttonStyle(RingButtonStyle(kind: .success, fullWidth: true))
                .keyboardShortcut(.defaultAction)
            }
        }
    }
}

/// A real slide-to-answer control: drag the cream thumb to the end of the track.
struct SlideToAnswer: View {
    var onAnswer: () -> Void
    @State private var offset: CGFloat = 0
    @State private var answered = false
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    private let thumb: CGFloat = 56
    private let inset: CGFloat = 4

    var body: some View {
        GeometryReader { geometry in
            let travel = max(0, geometry.size.width - thumb - inset * 2)
            ZStack(alignment: .leading) {
                Capsule().fill(Palette.momentSurface)
                ShimmerText(text: "slide to answer", animate: !reduceMotion && offset == 0)
                    .frame(maxWidth: .infinity)
                    .padding(.leading, thumb)
                    .opacity(travel > 0 ? 1 - Double(offset / travel) * 1.4 : 1)
                Circle()
                    .fill(Palette.momentForeground)
                    .frame(width: thumb, height: thumb)
                    .overlay(
                        Image(systemName: "phone.fill")
                            .font(.system(size: 22, weight: .regular))
                            .foregroundStyle(Palette.success)
                    )
                    .shadow(color: .black.opacity(0.25), radius: 6, y: 2)
                    .offset(x: inset + offset)
                    .gesture(
                        DragGesture(minimumDistance: 0)
                            .onChanged { value in
                                guard !answered else { return }
                                offset = min(max(0, value.translation.width), travel)
                            }
                            .onEnded { _ in
                                guard !answered else { return }
                                if offset > travel * 0.8 {
                                    answered = true
                                    withAnimation(.smooth(duration: 0.15)) { offset = travel }
                                    #if os(iOS)
                                    UIImpactFeedbackGenerator(style: .medium).impactOccurred()
                                    #endif
                                    onAnswer()
                                } else {
                                    withAnimation(.spring(response: 0.35, dampingFraction: 0.8)) { offset = 0 }
                                }
                            }
                    )
            }
        }
        .frame(height: thumb + inset * 2)
        .accessibilityElement()
        .accessibilityLabel("Slide to answer")
        .accessibilityAddTraits(.isButton)
        .accessibilityAction { onAnswer() }
    }
}

private struct ShimmerText: View {
    var text: String
    var animate: Bool

    var body: some View {
        TimelineView(.animation(paused: !animate)) { timeline in
            let phase = (timeline.date.timeIntervalSinceReferenceDate / 2.4).truncatingRemainder(dividingBy: 1)
            Text(text)
                .font(RingFont.ui(17))
                .foregroundStyle(Palette.momentMuted)
                .overlay {
                    if animate {
                        LinearGradient(
                            stops: [
                                .init(color: .clear, location: max(0, phase - 0.2)),
                                .init(color: Palette.momentForeground, location: phase),
                                .init(color: .clear, location: min(1, phase + 0.2)),
                            ],
                            startPoint: .leading, endPoint: .trailing)
                        .mask(Text(text).font(RingFont.ui(17)))
                    }
                }
        }
    }
}

/// Wraps chips onto as many lines as they need.
struct FlowLayout: Layout {
    var spacing: CGFloat = 8

    func sizeThatFits(proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) -> CGSize {
        let rows = arrange(subviews, width: proposal.width ?? .infinity)
        let height = rows.map(\.height).reduce(0, +) + spacing * CGFloat(max(0, rows.count - 1))
        let width = rows.map(\.width).max() ?? 0
        return CGSize(width: proposal.width ?? width, height: height)
    }

    func placeSubviews(in bounds: CGRect, proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) {
        var y = bounds.minY
        for row in arrange(subviews, width: bounds.width) {
            var x = bounds.minX + (bounds.width - row.width) / 2
            for index in row.indices {
                let size = subviews[index].sizeThatFits(.unspecified)
                subviews[index].place(at: CGPoint(x: x, y: y), proposal: ProposedViewSize(size))
                x += size.width + spacing
            }
            y += row.height + spacing
        }
    }

    private struct Row {
        var indices: [Int] = []
        var width: CGFloat = 0
        var height: CGFloat = 0
    }

    private func arrange(_ subviews: Subviews, width: CGFloat) -> [Row] {
        var rows: [Row] = [Row()]
        for index in subviews.indices {
            let size = subviews[index].sizeThatFits(.unspecified)
            let needed = rows[rows.count - 1].indices.isEmpty ? size.width : rows[rows.count - 1].width + spacing + size.width
            if needed > width && !rows[rows.count - 1].indices.isEmpty {
                rows.append(Row())
            }
            var row = rows[rows.count - 1]
            row.width = row.indices.isEmpty ? size.width : row.width + spacing + size.width
            row.height = max(row.height, size.height)
            row.indices.append(index)
            rows[rows.count - 1] = row
        }
        return rows
    }
}

// MARK: - Outgoing

struct OutgoingCallView: View {
    var session: CallSession
    @Environment(AppModel.self) private var model

    var body: some View {
        let person = session.counterpart
        VStack(spacing: 0) {
            Spacer(minLength: 24)
            VStack(spacing: 16) {
                ZStack {
                    if session.phase == .voicemail {
                        SpeakingPulse(active: true, color: Palette.accent)
                            .frame(width: 120, height: 120)
                    } else if session.isLive {
                        ExpandingRings(color: Palette.ink)
                            .frame(width: 120, height: 120)
                    }
                    AvatarView(account: person, size: 120)
                }
                .frame(width: 230, height: 230)
                VStack(spacing: 6) {
                    Text(person.label)
                        .font(RingFont.display(Metrics.isTouch ? 36 : 44))
                        .foregroundStyle(Palette.ink)
                        .multilineTextAlignment(.center)
                        .lineLimit(2)
                        .minimumScaleFactor(0.6)
                    if person.hasDisplayName {
                        Text(person.id).pixelID(13)
                    }
                }
                KindBadge(account: person)
                Text(status)
                    .pixelLabel(13, color: session.phase == .voicemail ? Palette.accentText : Palette.textSecondary)
                    .multilineTextAlignment(.center)
                    .padding(.top, 8)
                    .contentTransition(.opacity)
                    .animation(.easeOut(duration: 0.15), value: status)
            }
            .padding(.horizontal, 24)
            Spacer(minLength: 24)
            if session.phase == .voicemail {
                Button("Done") { model.hangUp() }
                    .buttonStyle(RingButtonStyle(kind: .primary))
                    .frame(width: 160)
                    .padding(.bottom, 24)
                    .accessibilityHint("Hangs up and sends your message")
            }
            HStack(spacing: 36) {
                RoundCallButton(symbol: session.muted ? "mic.slash.fill" : "mic.slash", label: session.muted ? "Unmute" : "Mute",
                                kind: session.muted ? .toggled : .normal, onMoment: false) { model.toggleMute() }
                    .disabled(!session.isLive)
                RoundCallButton(symbol: "phone.down.fill", label: session.isLive ? "End" : "Close", kind: .danger, onMoment: false) {
                    model.hangUp()
                }
                AudioRouteButton(onMoment: false)
            }
            .padding(.bottom, 40)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background(Palette.background.ignoresSafeArea())
    }

    private var status: String {
        switch session.phase {
        case .calling: "Calling…"
        case .ringing:
            if session.outgoingInvitation?.state == .busy {
                "Busy"
            } else if session.mediaState == .connected {
                "Ringing"
            } else {
                // The callee's devices are being reached while this device joins the room.
                "Calling…"
            }
        case .voicemail: "Leave a message after the beep"
        case .active: "Connected"
        case .ended: session.endSummary ?? "Call ended"
        }
    }
}

// MARK: - In call

struct InCallView: View {
    var session: CallSession
    @Environment(AppModel.self) private var model
    @State private var inviting = false

    var body: some View {
        VStack(spacing: 0) {
            header
            TranscriptView(lines: session.visibleLines, myUUID: session.myUUID)
                .frame(maxWidth: 720)
                .frame(maxHeight: .infinity)
                .padding(.horizontal, 20)
            controls
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background(Palette.background.ignoresSafeArea())
        .sheet(isPresented: $inviting) {
            DialerView(mode: .invite)
        }
    }

    private var header: some View {
        VStack(spacing: 18) {
            HStack {
                Button {
                    withAnimation(.smooth) { session.minimized = true }
                } label: {
                    Image(systemName: "chevron.down")
                        .font(.system(size: 17, weight: .light))
                        .foregroundStyle(Palette.textSecondary)
                        .frame(width: 44, height: 44)
                }
                .buttonStyle(.plain)
                .accessibilityLabel("Minimize call")
                .opacity(session.isLive ? 1 : 0)
                Spacer()
                Group {
                    if session.isLive {
                        CallTimer(since: session.answeredAt)
                    } else {
                        Text(session.endSummary ?? "Call ended")
                    }
                }
                .pixelLabel(13, color: session.isLive ? Palette.textSecondary : Palette.danger)
                Spacer()
                mediaIndicator
                    .frame(width: 44, height: 44)
            }
            .padding(.horizontal, 8)

            let people = session.others
            HStack(alignment: .top, spacing: people.count > 2 ? 18 : 32) {
                ForEach(people.prefix(4), id: \.uuid) { person in
                    let size: CGFloat = people.count == 1 ? 104 : (people.count == 2 ? 84 : 64)
                    VStack(spacing: 10) {
                        ZStack {
                            SpeakingPulse(active: session.speakingUUIDs.contains(person.uuid), color: Palette.accent)
                                .frame(width: size, height: size)
                            AvatarView(account: person, size: size)
                        }
                        VStack(spacing: 4) {
                            Text(person.label)
                                .font(RingFont.display(people.count == 1 ? 30 : 20))
                                .foregroundStyle(Palette.ink)
                                .lineLimit(1)
                                .minimumScaleFactor(0.7)
                            // The id under the name (`si:tos`, `c:alice`); a nameless account already shows it.
                            if person.hasDisplayName {
                                Text(person.id)
                                    .pixelID(people.count == 1 ? 12 : 11)
                                    .lineLimit(1)
                                    .truncationMode(.middle)
                            }
                        }
                        if person.isSilicon { KindBadge(account: person) }
                    }
                    .frame(maxWidth: 200)
                    .accessibilityElement(children: .combine)
                }
            }
            .padding(.horizontal, 16)
        }
        .padding(.top, 8)
        .padding(.bottom, 12)
    }

    @ViewBuilder
    private var mediaIndicator: some View {
        switch session.mediaState {
        case .connecting, .reconnecting:
            ProgressView().controlSize(.small)
                .accessibilityLabel("Connecting audio")
        case .disconnected:
            Image(systemName: "wifi.exclamationmark")
                .foregroundStyle(Palette.warning)
                .accessibilityLabel("Audio disconnected")
        default:
            Color.clear
        }
    }

    private var controls: some View {
        HStack(spacing: Metrics.isTouch ? 26 : 32) {
            RoundCallButton(symbol: session.muted ? "mic.slash.fill" : "mic.slash", label: session.muted ? "Unmute" : "Mute",
                            kind: session.muted ? .toggled : .normal, size: 60, onMoment: false) { model.toggleMute() }
            AudioRouteButton(onMoment: false, size: 60)
            RoundCallButton(symbol: "person.badge.plus", label: "Add", size: 60, onMoment: false) { inviting = true }
            RoundCallButton(symbol: "phone.down.fill", label: session.isLive ? "End" : "Close", kind: .danger, size: 60, onMoment: false) {
                model.hangUp()
            }
        }
        .padding(.top, 12)
        .padding(.bottom, 32)
    }
}

/// The live transcript: lines enter at the bottom, move up, and fade toward the top.
struct TranscriptView: View {
    var lines: [TranscriptLine]
    var myUUID: String

    var body: some View {
        ScrollViewReader { proxy in
            ScrollView {
                VStack(alignment: .leading, spacing: 16) {
                    Spacer(minLength: 120)
                    if lines.isEmpty {
                        Text("The transcript appears here as people speak.")
                            .font(RingFont.ui(15))
                            .foregroundStyle(Palette.textMuted)
                            .frame(maxWidth: .infinity)
                    }
                    ForEach(lines) { line in
                        lineView(line)
                            .id(line.id)
                            .transition(.asymmetric(insertion: .move(edge: .bottom).combined(with: .opacity), removal: .opacity))
                    }
                    Color.clear.frame(height: 1).id("bottom")
                }
                .frame(maxWidth: .infinity, alignment: .leading)
                .animation(.smooth(duration: 0.35), value: lines.map(\.id))
            }
            .scrollIndicators(.hidden)
            .defaultScrollAnchor(.bottom)
            .mask(
                LinearGradient(stops: [
                    .init(color: .clear, location: 0),
                    .init(color: .black.opacity(0.35), location: 0.18),
                    .init(color: .black, location: 0.45),
                    .init(color: .black, location: 1),
                ], startPoint: .top, endPoint: .bottom)
            )
            .onChange(of: lines.last?.id) { _, _ in
                withAnimation(.smooth(duration: 0.35)) { proxy.scrollTo("bottom", anchor: .bottom) }
            }
            .onChange(of: lines.last?.text) { _, _ in
                proxy.scrollTo("bottom", anchor: .bottom)
            }
        }
    }

    @ViewBuilder
    private func lineView(_ line: TranscriptLine) -> some View {
        switch line.style {
        case .event:
            Text(line.text)
                .pixelLabel(11)
                .frame(maxWidth: .infinity, alignment: .center)
        case .speech, .partial:
            VStack(alignment: .leading, spacing: 4) {
                Text(line.speaker.map { $0.uuid == myUUID ? "You" : $0.label } ?? "Someone")
                    .pixelLabel(11)
                Text(line.text)
                    .font(RingFont.ui(Metrics.isTouch ? 17 : 18))
                    .lineSpacing(4)
                    .foregroundStyle(line.style == .partial ? Palette.textMuted : Palette.foreground)
                    .fixedSize(horizontal: false, vertical: true)
                    .textSelection(.enabled)
            }
        }
    }
}
