package com.teamofsilicons.ring.ui.theme

import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Typography
import androidx.compose.material3.darkColorScheme
import androidx.compose.material3.lightColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.Immutable
import androidx.compose.runtime.staticCompositionLocalOf
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.Font
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.em
import androidx.compose.ui.unit.sp
import com.teamofsilicons.ring.R
import com.teamofsilicons.ring.data.Appearance

/** The Ring tokens from udd/design.md (teamofsilicons.com base, dark from the Ring mark). */
@Immutable
data class RingColors(
    val background: Color,
    val surface: Color,
    val surfaceRaised: Color,
    val surfaceMuted: Color,
    val beige: Color,
    val foreground: Color,
    val ink: Color,
    val textSecondary: Color,
    val textMuted: Color,
    val border: Color,
    val borderStrong: Color,
    val accent: Color,
    val accentHover: Color,
    val onPrimary: Color,
    val accentText: Color,
    val accentSubtle: Color,
    val accentBorder: Color,
    val success: Color,
    val danger: Color,
    val warning: Color,
    val dark: Boolean,
)

val LightRingColors = RingColors(
    background = Color(0xFFFFFDF9),
    surface = Color(0xFFFFFFFF),
    surfaceRaised = Color(0xFFFFFFFF),
    surfaceMuted = Color(0xFFF6F2EA),
    beige = Color(0xFFEDE8E0),
    foreground = Color(0xFF1E1E1C),
    ink = Color(0xFF181818),
    textSecondary = Color(0xFF4A4842),
    textMuted = Color(0xFF6E6B63),
    border = Color(0xFFE7E3D8),
    borderStrong = Color(0xFFD9D2C4),
    accent = Color(0xFF1F5CB1),
    accentHover = Color(0xFF184A94),
    onPrimary = Color(0xFFFFFFFF),
    accentText = Color(0xFF1F5CB1),
    accentSubtle = Color(0x141F5CB1),
    accentBorder = Color(0x401F5CB1),
    success = Color(0xFF1F7A4D),
    danger = Color(0xFFC8322B),
    warning = Color(0xFFB66400),
    dark = false,
)

val DarkRingColors = RingColors(
    background = Color(0xFF181818),
    surface = Color(0xFF1E1E1C),
    surfaceRaised = Color(0xFF252421),
    surfaceMuted = Color(0xFF22211E),
    beige = Color(0xFF2E2C28),
    foreground = Color(0xFFEDE8E0),
    ink = Color(0xFFF6F2EA),
    textSecondary = Color(0xFFC9C3B6),
    textMuted = Color(0xFF9C978C),
    border = Color(0xFF2F2D29),
    borderStrong = Color(0xFF3D3A35),
    accent = Color(0xFF3872CA),
    accentHover = Color(0xFF4A80D2),
    onPrimary = Color(0xFFFFFFFF),
    accentText = Color(0xFF8DB2EC),
    accentSubtle = Color(0x295B8FD9),
    accentBorder = Color(0x595B8FD9),
    success = Color(0xFF4CC38A),
    danger = Color(0xFFF2706A),
    warning = Color(0xFFF0B54A),
    dark = true,
)

/** Fixed colours of the Ring mark, used by "moment" screens in both themes. */
object MarkColors {
    val ink = Color(0xFF181818)
    val cream = Color(0xFFEDE8E0)
    val creamMuted = Color(0xFF9C978C)
}

val BdoGrotesk = FontFamily(
    Font(R.font.bdo_grotesk_regular, FontWeight.Normal),
    Font(R.font.bdo_grotesk_medium, FontWeight.Medium),
    Font(R.font.bdo_grotesk_bold, FontWeight.Bold),
)
val HedvigSerif = FontFamily(Font(R.font.hedvig_letters_serif, FontWeight.Normal))
val GeistPixel = FontFamily(Font(R.font.geist_pixel_square, FontWeight.Normal))

/** Text roles from udd/design.md. */
object RingType {
    val body = TextStyle(fontFamily = BdoGrotesk, fontSize = 15.sp, lineHeight = 22.sp, letterSpacing = 0.sp)
    val bodySmall = TextStyle(fontFamily = BdoGrotesk, fontSize = 14.sp, lineHeight = 20.sp, letterSpacing = 0.sp)
    val bodyLarge = TextStyle(fontFamily = BdoGrotesk, fontSize = 16.sp, lineHeight = 24.sp, letterSpacing = 0.sp)
    val button = TextStyle(fontFamily = BdoGrotesk, fontWeight = FontWeight.Medium, fontSize = 15.sp, lineHeight = 20.sp)
    val title = TextStyle(fontFamily = BdoGrotesk, fontWeight = FontWeight.Medium, fontSize = 16.sp, lineHeight = 22.sp)
    val heading = TextStyle(fontFamily = BdoGrotesk, fontWeight = FontWeight.Medium, fontSize = 18.sp, lineHeight = 24.sp)
    val pageTitle = TextStyle(fontFamily = HedvigSerif, fontSize = 26.sp, lineHeight = 31.sp)
    val serifName = TextStyle(fontFamily = HedvigSerif, fontSize = 18.sp, lineHeight = 21.sp)
    val displayName = TextStyle(fontFamily = HedvigSerif, fontSize = 36.sp, lineHeight = 42.sp)
    val hero = TextStyle(fontFamily = HedvigSerif, fontSize = 26.sp, lineHeight = 31.sp)
    val pixel = TextStyle(fontFamily = GeistPixel, fontSize = 12.sp, lineHeight = 16.sp, letterSpacing = 0.03.em)
    val pixelSmall = TextStyle(fontFamily = GeistPixel, fontSize = 11.sp, lineHeight = 14.sp, letterSpacing = 0.03.em)
    val pixelLarge = TextStyle(fontFamily = GeistPixel, fontSize = 13.sp, lineHeight = 18.sp, letterSpacing = 0.04.em)
    val transcript = TextStyle(fontFamily = BdoGrotesk, fontSize = 17.sp, lineHeight = 25.sp)
    val wordmark = TextStyle(fontFamily = GeistPixel, fontSize = 22.sp, lineHeight = 26.sp, letterSpacing = 0.12.em)
}

val LocalRingColors = staticCompositionLocalOf { LightRingColors }

object Ring {
    val colors: RingColors
        @Composable get() = LocalRingColors.current
}

@Composable
fun RingTheme(appearance: Appearance = Appearance.System, forceDark: Boolean = false, content: @Composable () -> Unit) {
    val dark = forceDark || when (appearance) {
        Appearance.System -> isSystemInDarkTheme()
        Appearance.Light -> false
        Appearance.Dark -> true
    }
    val colors = if (dark) DarkRingColors else LightRingColors
    val scheme = if (dark) {
        darkColorScheme(
            primary = colors.accent, onPrimary = colors.onPrimary,
            secondary = colors.textSecondary, onSecondary = colors.background,
            background = colors.background, onBackground = colors.foreground,
            surface = colors.surface, onSurface = colors.foreground,
            surfaceVariant = colors.surfaceMuted, onSurfaceVariant = colors.textSecondary,
            surfaceContainer = colors.surface, surfaceContainerHigh = colors.surfaceRaised,
            surfaceContainerHighest = colors.surfaceRaised, surfaceContainerLow = colors.surfaceMuted,
            secondaryContainer = colors.beige, onSecondaryContainer = colors.foreground,
            outline = colors.borderStrong, outlineVariant = colors.border,
            error = colors.danger, onError = colors.onPrimary,
        )
    } else {
        lightColorScheme(
            primary = colors.accent, onPrimary = colors.onPrimary,
            secondary = colors.textSecondary, onSecondary = colors.background,
            background = colors.background, onBackground = colors.foreground,
            surface = colors.surface, onSurface = colors.foreground,
            surfaceVariant = colors.surfaceMuted, onSurfaceVariant = colors.textSecondary,
            surfaceContainer = colors.surface, surfaceContainerHigh = colors.surfaceRaised,
            surfaceContainerHighest = colors.surfaceRaised, surfaceContainerLow = colors.surfaceMuted,
            secondaryContainer = colors.beige, onSecondaryContainer = colors.foreground,
            outline = colors.borderStrong, outlineVariant = colors.border,
            error = colors.danger, onError = colors.onPrimary,
        )
    }
    val base = Typography()
    val typography = Typography(
        displayLarge = base.displayLarge.copy(fontFamily = HedvigSerif),
        displayMedium = base.displayMedium.copy(fontFamily = HedvigSerif),
        displaySmall = base.displaySmall.copy(fontFamily = HedvigSerif),
        headlineLarge = RingType.pageTitle,
        headlineMedium = RingType.pageTitle.copy(fontSize = 22.sp, lineHeight = 26.sp),
        headlineSmall = RingType.heading,
        titleLarge = RingType.heading,
        titleMedium = RingType.title,
        titleSmall = RingType.title.copy(fontSize = 14.sp),
        bodyLarge = RingType.bodyLarge,
        bodyMedium = RingType.body,
        bodySmall = RingType.bodySmall,
        labelLarge = RingType.button,
        labelMedium = RingType.pixel,
        labelSmall = RingType.pixelSmall,
    )
    CompositionLocalProvider(LocalRingColors provides colors) {
        MaterialTheme(colorScheme = scheme, typography = typography, content = content)
    }
}
