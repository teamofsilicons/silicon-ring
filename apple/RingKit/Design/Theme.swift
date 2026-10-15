import SwiftUI

/// The teamofsilicons.com tokens (udd/design.md) as named asset-catalog colours with light and dark variants.
enum Palette {
    static let background = Color("background")
    static let surface = Color("surface")
    static let surfaceRaised = Color("surfaceRaised")
    static let surfaceMuted = Color("surfaceMuted")
    static let beige = Color("beige")
    static let foreground = Color("foreground")
    static let ink = Color("ink")
    static let textSecondary = Color("textSecondary")
    static let textMuted = Color("textMuted")
    static let border = Color("border")
    static let borderStrong = Color("borderStrong")
    static let accent = Color("accent")
    static let accentHover = Color("accentHover")
    static let onPrimary = Color("onPrimary")
    static let accentText = Color("accentText")
    static let accentSubtle = Color("accentSubtle")
    static let accentBorder = Color("accentBorder")
    static let success = Color("success")
    static let danger = Color("danger")
    static let warning = Color("warning")
    static let secondaryFill = Color("secondaryFill")
    static let secondaryBorder = Color("secondaryBorder")

    /// The incoming and in-call "moment" screens: the Ring mark's ink and cream in both themes.
    static let momentBackground = Color("momentBackground")
    static let momentSurface = Color("momentSurface")
    static let momentForeground = Color("momentForeground")
    static let momentMuted = Color("momentMuted")

    /// Text and glyphs on success/danger fills.
    static func onStatus(_ scheme: ColorScheme) -> Color {
        scheme == .dark ? Color(red: 0x18 / 255, green: 0x18 / 255, blue: 0x18 / 255) : .white
    }
}

/// BDO Grotesk for UI, Hedvig Letters Serif for display, Geist Pixel Square for labels.
enum RingFont {
    enum Weight {
        case regular, medium, bold

        var postScriptName: String {
            switch self {
            case .regular: "BDOGrotesk-Regular"
            case .medium: "BDOGrotesk-Medium"
            case .bold: "BDOGrotesk-Bold"
            }
        }
    }

    static func ui(_ size: CGFloat, _ weight: Weight = .regular, relativeTo style: Font.TextStyle = .body) -> Font {
        .custom(weight.postScriptName, size: size, relativeTo: style)
    }

    static func display(_ size: CGFloat, relativeTo style: Font.TextStyle = .largeTitle) -> Font {
        .custom("HedvigLettersSerif24pt24pt-Regular", size: size, relativeTo: style)
    }

    static func pixel(_ size: CGFloat, relativeTo style: Font.TextStyle = .caption) -> Font {
        .custom("GeistPixel-Square", size: size, relativeTo: style)
    }
}

enum Metrics {
    #if os(iOS)
    static let controlHeight: CGFloat = 44
    static let isTouch = true
    #else
    static let controlHeight: CGFloat = 36
    static let isTouch = false
    #endif
    static let cardRadius: CGFloat = 12
    static let controlRadius: CGFloat = 8
}

extension View {
    /// Small uppercase pixel-font label with +3% tracking.
    func pixelLabel(_ size: CGFloat = 12, color: Color = Palette.textMuted) -> some View {
        font(RingFont.pixel(size))
            .tracking(size * 0.03)
            .textCase(.uppercase)
            .foregroundStyle(color)
    }

    /// An account id (`c:alice`, `si:tos`) in the pixel font; ids keep their case.
    func pixelID(_ size: CGFloat = 12, color: Color = Palette.textMuted) -> some View {
        font(RingFont.pixel(size))
            .tracking(size * 0.03)
            .foregroundStyle(color)
    }

    /// The card group: surface fill, 0.5 px border, soft shadow, continuous corners.
    func ringCard(radius: CGFloat = Metrics.cardRadius) -> some View {
        background(
            RoundedRectangle(cornerRadius: radius, style: .continuous)
                .fill(Palette.surface)
                .shadow(color: .black.opacity(0.06), radius: 2, y: 1)
        )
        .overlay(
            RoundedRectangle(cornerRadius: radius, style: .continuous)
                .strokeBorder(Palette.border, lineWidth: 0.5)
        )
    }

    /// Popovers, sheets and dialogs: raised surface, 0.5 px strong border, deeper shadow.
    func ringRaised(radius: CGFloat = 12) -> some View {
        background(
            RoundedRectangle(cornerRadius: radius, style: .continuous)
                .fill(Palette.surfaceRaised)
                .shadow(color: .black.opacity(0.10), radius: 16, y: 12)
                .shadow(color: .black.opacity(0.05), radius: 3, y: 2)
        )
        .overlay(
            RoundedRectangle(cornerRadius: radius, style: .continuous)
                .strokeBorder(Palette.borderStrong, lineWidth: 0.5)
        )
    }
}

/// A one-physical-pixel divider in the border colour.
struct Hairline: View {
    @Environment(\.displayScale) private var displayScale
    var color: Color = Palette.border

    var body: some View {
        Rectangle()
            .fill(color)
            .frame(height: 1 / max(displayScale, 1))
    }
}

/// Home screens draw their own serif title: phones hide the navigation bar; iPad and Mac keep
/// it (inline, untitled) for the sidebar toggle and the New call button.
private struct HomeScreenChrome: ViewModifier {
    #if os(iOS)
    @Environment(\.horizontalSizeClass) private var sizeClass
    #endif

    func body(content: Content) -> some View {
        #if os(iOS)
        if sizeClass == .compact {
            content.toolbar(.hidden, for: .navigationBar)
        } else {
            content
                .navigationBarTitleDisplayMode(.inline)
                .toolbar { ToolbarItem(placement: .principal) { Color.clear.frame(width: 1, height: 1) } }
        }
        #else
        content
        #endif
    }
}

extension View {
    func homeScreenChrome() -> some View { modifier(HomeScreenChrome()) }
}

extension Appearance {
    var colorScheme: ColorScheme? {
        switch self {
        case .system: nil
        case .light: .light
        case .dark: .dark
        }
    }
}
