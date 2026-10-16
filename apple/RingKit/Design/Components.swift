import SwiftUI

// MARK: - Ring mark and wordmark

/// The Ring mark: concentric rings around a dot (assets/icon/icon.svg geometry, 200-unit box).
struct RingMark: View {
    var size: CGFloat
    /// Draw the ink tile behind the mark, as on the app icon.
    var tile = true
    var markColor: Color = Color(red: 0xED / 255, green: 0xE8 / 255, blue: 0xE0 / 255)
    var tileColor: Color = Color(red: 0x18 / 255, green: 0x18 / 255, blue: 0x18 / 255)

    var body: some View {
        Canvas { context, canvas in
            let scale = canvas.width / (tile ? 200 : 120)
            let origin = tile ? CGPoint.zero : CGPoint(x: -40 * scale, y: -40 * scale)
            func p(_ x: CGFloat, _ y: CGFloat) -> CGPoint { CGPoint(x: origin.x + x * scale, y: origin.y + y * scale) }
            if tile {
                context.fill(Path(roundedRect: CGRect(origin: .zero, size: canvas), cornerRadius: canvas.width * 0.225, style: .continuous),
                             with: .color(tileColor))
            }
            let center = p(99.5, 99.5)
            func circle(_ r: CGFloat) -> Path {
                Path(ellipseIn: CGRect(x: center.x - r * scale, y: center.y - r * scale, width: 2 * r * scale, height: 2 * r * scale))
            }
            context.fill(circle(11.5), with: .color(markColor))
            context.stroke(circle(32), with: .color(markColor), lineWidth: (tile ? 3 : 5) * scale)
            if tile {
                let ellipse = Path(ellipseIn: CGRect(x: p(55, 56).x, y: p(55, 56).y, width: 89 * scale, height: 87 * scale))
                context.stroke(ellipse, with: .color(markColor), lineWidth: 2 * scale)
                context.stroke(circle(55), with: .color(markColor), lineWidth: 1 * scale)
            } else {
                context.stroke(circle(55), with: .color(markColor), lineWidth: 4 * scale)
            }
        }
        .frame(width: size, height: size)
        .accessibilityHidden(true)
    }
}

struct Wordmark: View {
    var size: CGFloat = 13
    var color: Color = Palette.ink

    var body: some View {
        Text("RING")
            .font(RingFont.pixel(size))
            .tracking(size * 0.12)
            .foregroundStyle(color)
            .accessibilityLabel("Ring")
    }
}

// MARK: - Avatars and badges

struct AvatarView: View {
    var account: Account
    var size: CGFloat
    var onMoment = false

    var body: some View {
        ZStack {
            Circle().fill(onMoment ? Palette.momentSurface : Palette.beige)
            if let string = account.pfpURL, let url = URL(string: string) {
                AsyncImage(url: url, transaction: Transaction(animation: .easeOut(duration: 0.15))) { phase in
                    if let image = phase.image {
                        image.resizable().scaledToFill()
                    } else {
                        initial
                    }
                }
            } else {
                initial
            }
        }
        .frame(width: size, height: size)
        .clipShape(Circle())
        .overlay(Circle().strokeBorder(onMoment ? Palette.momentMuted.opacity(0.4) : Palette.border, lineWidth: 0.5))
        .accessibilityHidden(true)
    }

    private var initial: some View {
        Text(Self.initial(for: account))
            .font(RingFont.display(size * 0.42))
            .foregroundStyle(onMoment ? Palette.momentForeground : Palette.accentText)
    }

    static func initial(for account: Account) -> String {
        let source = account.hasDisplayName ? account.label : (account.id.split(separator: ":").last.map(String.init) ?? account.id)
        return source.first.map { String($0).uppercased() } ?? "?"
    }
}

struct Badge: View {
    enum Style { case plain, silicon, onMoment }
    var text: String
    var style: Style = .plain

    var body: some View {
        Text(text)
            .font(RingFont.pixel(11))
            .tracking(0.33)
            .textCase(.uppercase)
            .lineLimit(1)
            .padding(.horizontal, 8)
            .frame(height: 22)
            .foregroundStyle(foreground)
            .background(RoundedRectangle(cornerRadius: 5, style: .continuous).fill(fill))
            .overlay(RoundedRectangle(cornerRadius: 5, style: .continuous).strokeBorder(stroke, lineWidth: 0.5))
    }

    private var foreground: Color {
        switch style {
        case .plain: Palette.foreground
        case .silicon: Palette.accentText
        case .onMoment: Palette.momentForeground
        }
    }

    private var fill: Color {
        switch style {
        case .plain: Palette.beige
        case .silicon: Palette.accentSubtle
        case .onMoment: Palette.momentSurface
        }
    }

    private var stroke: Color {
        switch style {
        case .plain: Palette.borderStrong
        case .silicon: Palette.accentBorder
        case .onMoment: Palette.momentMuted.opacity(0.4)
        }
    }
}

/// SILICON (accent) or nothing for carbons in lists; CARBON appears only where kind matters.
struct KindBadge: View {
    var account: Account
    var showCarbon = false
    var onMoment = false

    var body: some View {
        if account.isSilicon {
            Badge(text: "Silicon", style: onMoment ? .onMoment : .silicon)
        } else if showCarbon {
            Badge(text: "Carbon", style: onMoment ? .onMoment : .plain)
        }
    }
}

// MARK: - Buttons

struct RingButtonStyle: ButtonStyle {
    enum Kind { case primary, secondary, danger, success, ghost }
    var kind: Kind = .primary
    var fullWidth = false
    var small = false

    @Environment(\.isEnabled) private var isEnabled
    @Environment(\.colorScheme) private var colorScheme

    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .font(RingFont.ui(small ? 14 : 15, kind == .ghost || kind == .secondary ? .regular : .medium))
            .lineLimit(1)
            .padding(.horizontal, small ? 10 : 14)
            .frame(minHeight: small ? 30 : Metrics.controlHeight)
            .frame(maxWidth: fullWidth ? .infinity : nil)
            .foregroundStyle(foreground)
            .background(
                RoundedRectangle(cornerRadius: Metrics.controlRadius, style: .continuous)
                    .fill(fill(pressed: configuration.isPressed))
                    .shadow(color: .black.opacity(kind == .ghost ? 0 : 0.08), radius: 3, y: 2)
            )
            .overlay(
                RoundedRectangle(cornerRadius: Metrics.controlRadius, style: .continuous)
                    .strokeBorder(stroke, lineWidth: kind == .ghost ? 0 : 0.5)
            )
            .contentShape(RoundedRectangle(cornerRadius: Metrics.controlRadius, style: .continuous))
            .scaleEffect(configuration.isPressed ? 0.98 : 1)
            .opacity(isEnabled ? 1 : 0.45)
            .animation(.easeOut(duration: 0.15), value: configuration.isPressed)
    }

    private var foreground: Color {
        switch kind {
        case .primary: Palette.onPrimary
        case .danger, .success: Palette.onStatus(colorScheme)
        case .secondary: Palette.foreground
        case .ghost: Palette.accentText
        }
    }

    private func fill(pressed: Bool) -> Color {
        switch kind {
        case .primary: pressed ? Palette.accentHover : Palette.accent
        case .danger: Palette.danger.opacity(pressed ? 0.85 : 1)
        case .success: Palette.success.opacity(pressed ? 0.85 : 1)
        case .secondary: pressed ? Palette.beige : Palette.secondaryFill
        case .ghost: pressed ? Palette.beige.opacity(0.6) : .clear
        }
    }

    private var stroke: Color {
        switch kind {
        case .primary: Palette.accentBorder
        case .danger, .success: .black.opacity(0.08)
        case .secondary: Palette.secondaryBorder
        case .ghost: .clear
        }
    }
}

extension View {
    /// The main action in a toolbar. From macOS 26 and iOS 26 the toolbar sets every item in a
    /// Liquid Glass capsule, so the button takes the system's tinted glass instead of drawing its
    /// own rounded rectangle inside that capsule, where its corners would be clipped. Set the
    /// font on the label: the glass style ignores one set on the button.
    @ViewBuilder
    func toolbarProminentStyle() -> some View {
        if #available(iOS 26, macOS 26, *) {
            buttonStyle(.glassProminent)
                .tint(Palette.accent)
        } else {
            buttonStyle(RingButtonStyle(kind: .primary, small: true))
        }
    }
}

extension ButtonStyle where Self == RingButtonStyle {
    static var ringPrimary: RingButtonStyle { RingButtonStyle(kind: .primary) }
    static var ringSecondary: RingButtonStyle { RingButtonStyle(kind: .secondary) }
    static var ringGhost: RingButtonStyle { RingButtonStyle(kind: .ghost) }
}

/// The round call controls: mute, speaker, add person, hang up, accept and decline.
struct RoundCallButton: View {
    enum Kind { case normal, toggled, danger, success }
    var symbol: String
    var label: String
    var kind: Kind = .normal
    var size: CGFloat = 64
    var onMoment = true
    var action: () -> Void
    @Environment(\.colorScheme) private var colorScheme

    var body: some View {
        Button(action: action) {
            VStack(spacing: 8) {
                Image(systemName: symbol)
                    .font(.system(size: size * 0.36, weight: .light))
                    .frame(width: size, height: size)
                    .foregroundStyle(iconColor)
                    .background(Circle().fill(fill))
                    .overlay(Circle().strokeBorder(stroke, lineWidth: 0.5))
                Text(label)
                    .font(RingFont.pixel(11))
                    .tracking(0.33)
                    .textCase(.uppercase)
                    .foregroundStyle(onMoment ? Palette.momentMuted : Palette.textMuted)
            }
        }
        .buttonStyle(PressScaleStyle())
        .accessibilityLabel(label)
    }

    private var fill: Color {
        switch kind {
        case .normal: onMoment ? Palette.momentSurface : Palette.beige
        case .toggled: onMoment ? Palette.momentForeground : Palette.ink
        case .danger: Palette.danger
        case .success: Palette.success
        }
    }

    private var iconColor: Color {
        switch kind {
        case .normal: onMoment ? Palette.momentForeground : Palette.foreground
        case .toggled: onMoment ? Palette.momentBackground : Palette.background
        case .danger, .success: Palette.onStatus(colorScheme)
        }
    }

    private var stroke: Color {
        kind == .normal ? (onMoment ? Palette.momentMuted.opacity(0.35) : Palette.border) : .clear
    }
}

struct PressScaleStyle: ButtonStyle {
    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .scaleEffect(configuration.isPressed ? 0.96 : 1)
            .animation(.easeOut(duration: 0.12), value: configuration.isPressed)
    }
}

// MARK: - Motion: the Ring mark's rings

/// Faint rings expanding from an avatar while a call rings or connects.
struct ExpandingRings: View {
    var color: Color
    var count = 3
    var period: Double = 3

    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    var body: some View {
        if reduceMotion {
            ZStack {
                ForEach(1...count, id: \.self) { index in
                    Circle()
                        .stroke(color.opacity(0.18 / Double(index)), lineWidth: 1)
                        .scaleEffect(1 + 0.22 * CGFloat(index))
                }
            }
        } else {
            TimelineView(.animation) { timeline in
                let time = timeline.date.timeIntervalSinceReferenceDate
                ZStack {
                    ForEach(0..<count, id: \.self) { index in
                        let progress = (time / period + Double(index) / Double(count)).truncatingRemainder(dividingBy: 1)
                        Circle()
                            .stroke(color.opacity(0.35 * (1 - progress)), lineWidth: 1)
                            .scaleEffect(1 + 0.9 * progress)
                    }
                }
            }
        }
    }
}

/// A soft ring pulse around whoever is speaking.
struct SpeakingPulse: View {
    var active: Bool
    var color: Color

    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    var body: some View {
        TimelineView(.animation(paused: !active || reduceMotion)) { timeline in
            let phase = (timeline.date.timeIntervalSinceReferenceDate / 1.2).truncatingRemainder(dividingBy: 1)
            ZStack {
                Circle()
                    .stroke(color.opacity(active ? 0.55 : 0), lineWidth: 2)
                    .scaleEffect(1.08)
                Circle()
                    .stroke(color.opacity(active && !reduceMotion ? 0.3 * (1 - phase) : 0), lineWidth: 1.5)
                    .scaleEffect(1.08 + 0.25 * phase)
            }
        }
        .animation(.easeOut(duration: 0.15), value: active)
        .allowsHitTesting(false)
    }
}

// MARK: - Layout pieces

struct SectionLabel: View {
    var text: String

    var body: some View {
        Text(text)
            .pixelLabel(11)
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(.horizontal, 4)
            .padding(.bottom, 6)
            .accessibilityAddTraits(.isHeader)
    }
}

struct PageTitle: View {
    var text: String

    var body: some View {
        Text(text)
            .font(RingFont.display(34))
            .foregroundStyle(Palette.ink)
            .lineSpacing(0)
            .accessibilityAddTraits(.isHeader)
    }
}

struct EmptyStateView: View {
    var message: String
    var detail: String?

    @Environment(\.colorScheme) private var colorScheme

    var body: some View {
        VStack(spacing: 14) {
            RingMark(size: 72, tile: false, markColor: Palette.textMuted.opacity(colorScheme == .dark ? 0.35 : 0.25))
            Text(message)
                .font(RingFont.display(22))
                .foregroundStyle(Palette.textSecondary)
            if let detail {
                Text(detail)
                    .font(RingFont.ui(14))
                    .foregroundStyle(Palette.textMuted)
                    .multilineTextAlignment(.center)
            }
        }
        .frame(maxWidth: .infinity)
        .padding(.vertical, 48)
        .padding(.horizontal, 24)
    }
}

struct NoticeBanner: View {
    var notice: Notice

    var body: some View {
        HStack(spacing: 10) {
            Image(systemName: notice.style == .error ? "exclamationmark.circle" : "info.circle")
                .font(.system(size: 15, weight: .light))
                .foregroundStyle(notice.style == .error ? Palette.danger : Palette.accentText)
            Text(notice.message)
                .font(RingFont.ui(14))
                .foregroundStyle(Palette.foreground)
                .fixedSize(horizontal: false, vertical: true)
        }
        .padding(.horizontal, 14)
        .padding(.vertical, 10)
        .ringRaised(radius: 10)
        .padding(.horizontal, 16)
    }
}
