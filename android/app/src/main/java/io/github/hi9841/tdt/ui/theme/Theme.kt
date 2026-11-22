package io.github.hi9841.tdt.ui.theme

import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Typography
import androidx.compose.material3.darkColorScheme
import androidx.compose.material3.lightColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.em
import androidx.compose.ui.unit.sp

/** Brand colours from TDT desktop (desktop/src/ui/theme.rs) and the orb logo. The bubble uses these in both themes. */
object Brand {
    /** The blue at the centre of the orb logo (branding/tdt-app-icon.png). */
    val Orb = Color(0xFF5B9BFF)
    val Iris = Color(0xFF987CF2)
    val Foam = Color(0xFF8EC8FF)
    val Success = Color(0xFF7EDBB3)
    val Ink = Color(0xFF0A0B0F)
    val Paper = Color(0xFFEFF0F2)
}

// Dark: depth from surface lightness, not shadows. background < surface < surfaceVariant (raised cards) <
// surfaceContainerHigh (controls on cards).
private val Dark = darkColorScheme(
    primary = Brand.Orb,
    onPrimary = Brand.Ink,
    secondary = Brand.Foam,
    tertiary = Brand.Success,
    onTertiary = Brand.Ink,
    background = Brand.Ink,
    onBackground = Brand.Paper,
    surface = Brand.Ink,
    onSurface = Brand.Paper,
    surfaceVariant = Color(0xFF15161C),
    surfaceContainerHigh = Color(0xFF20212A),
    onSurfaceVariant = Color(0xFF9EA1AB),
    outline = Color(0xFF2C2E38),
    outlineVariant = Color(0xFF1F2029),
    error = Color(0xFFFF8FA3),
)

// Desktop has no light theme. These keep the orb accent and reach 4.5:1 text contrast on the light surfaces.
private val Light = lightColorScheme(
    primary = Color(0xFF1F5FD6),
    onPrimary = Color.White,
    secondary = Color(0xFF2F6FA8),
    tertiary = Color(0xFF1E7A55),
    onTertiary = Color.White,
    background = Color(0xFFF4F5F8),
    onBackground = Color(0xFF15161B),
    surface = Color(0xFFF4F5F8),
    onSurface = Color(0xFF15161B),
    surfaceVariant = Color(0xFFFFFFFF),
    surfaceContainerHigh = Color(0xFFEEEFF3),
    onSurfaceVariant = Color(0xFF5A5D66),
    outline = Color(0xFFD9DBE2),
    outlineVariant = Color(0xFFE8E9EE),
    error = Color(0xFFC62848),
)

/**
 * Six sizes only: 12, 14, 16, 20, 28 and 40. Headers sit tight (-2% letter spacing, 1.15 line height), as in the
 * desktop app's panel.
 */
private val Type = Typography(
    displaySmall = TextStyle(fontSize = 40.sp, lineHeight = 46.sp, fontWeight = FontWeight.SemiBold, letterSpacing = (-0.03).em),
    headlineSmall = TextStyle(fontSize = 28.sp, lineHeight = 32.sp, fontWeight = FontWeight.SemiBold, letterSpacing = (-0.02).em),
    titleLarge = TextStyle(fontSize = 20.sp, lineHeight = 24.sp, fontWeight = FontWeight.SemiBold, letterSpacing = (-0.02).em),
    titleMedium = TextStyle(fontSize = 16.sp, lineHeight = 22.sp, fontWeight = FontWeight.SemiBold, letterSpacing = (-0.01).em),
    titleSmall = TextStyle(fontSize = 14.sp, lineHeight = 20.sp, fontWeight = FontWeight.SemiBold),
    bodyLarge = TextStyle(fontSize = 16.sp, lineHeight = 24.sp),
    bodyMedium = TextStyle(fontSize = 14.sp, lineHeight = 20.sp),
    bodySmall = TextStyle(fontSize = 12.sp, lineHeight = 16.sp),
    labelLarge = TextStyle(fontSize = 14.sp, lineHeight = 20.sp, fontWeight = FontWeight.SemiBold),
    labelMedium = TextStyle(fontSize = 12.sp, lineHeight = 16.sp, fontWeight = FontWeight.SemiBold, letterSpacing = 0.02.em),
    labelSmall = TextStyle(fontSize = 12.sp, lineHeight = 16.sp, fontWeight = FontWeight.Medium),
)

@Composable
fun TdtTheme(content: @Composable () -> Unit) {
    MaterialTheme(colorScheme = if (isSystemInDarkTheme()) Dark else Light, typography = Type, content = content)
}
