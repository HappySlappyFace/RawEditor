package dev.raweditor

import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.darkColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.font.Font
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp

/**
 * The desktop's design tokens (`ui::theme`, its DARK palette), so the phone
 * looks like the same app. Every colour, size and space in the UI comes from
 * here; colours drawn over the photo (scrims, the histogram's channels) are
 * the exception, as on the desktop, since a photo looks the same in any theme.
 */
object Colours {
    val Chrome = Color(0xFF0D0D0D)
    val Bg = Color(0xFF141414)
    val Surface = Color(0xFF1A1A1A)
    val SurfaceRaised = Color(0xFF212121)
    val Control = Color(0xFF262626)
    val ControlActive = Color(0xFF404040)
    val BorderSubtle = Color(0xFF333333)
    val Border = Color(0xFF404040)
    val TextStrong = Color.White
    val Text = Color(0xFFE0E0E0)
    val TextSecondary = Color(0xFFB3B3B3)
    val TextMuted = Color(0xFF808080)
    val TextDisabled = Color(0xFF666666)
    val Accent = Color(0xFFF97316)
    val OnAccent = Color.White
    val OnPhoto = Color.White
    val Error = Color(0xFFF26659)
}

/**
 * Slider rails that depict their effect: the desktop's `rail_gradient`
 * colours, keyed by the controls table's rail names (`controls::Rail`).
 * They show the effect, so they are not theme colours.
 */
object Rails {
    private val gradients = mapOf(
        "dark-light" to listOf(Color(0xFF141414), Color(0xFFD1D1D1)),
        "cool-warm" to listOf(Color(0xFF4578DB), Color(0xFFEDC74D)),
        "green-magenta" to listOf(Color(0xFF54B35C), Color(0xFFCC5CC2)),
        "grey-colour" to listOf(Color(0xFF6B6B6B), Color(0xFFE0523D)),
    )

    /** The rail's colours, left to right; null for a plain rail. */
    fun colours(name: String): List<Color>? = gradients[name]
}

/** Text sizes, as the desktop's `theme::size`. */
object TextSize {
    val Caption = 11.sp
    val Small = 12.sp
    val Body = 13.sp
    val Label = 14.sp
    val Heading = 16.sp
    val Title = 18.sp
    val Display = 24.sp
}

/** Spacing, as the desktop's `theme::space`, plus the phone's own sizes. */
object Space {
    val XXS = 2.dp
    val XS = 4.dp
    val S = 6.dp
    val M = 8.dp
    val L = 10.dp
    val XL = 12.dp
    val XXL = 16.dp
    /** The screen's side margin. */
    val Gutter = 16.dp
    /** A slider or tool row. */
    val Row = 48.dp
    /** The smallest comfortable touch target. */
    val Touch = 44.dp
    /** The bar over the top of the photo. */
    val TopBar = 56.dp
}

object Radius {
    val Control = 4.dp
    val Card = 6.dp
    val Dialog = 10.dp
    val Pill = 14.dp
}

/**
 * The desktop's icon font (`assets/fonts/icons.ttf`, JetBrains Mono Nerd
 * Font: Font Awesome glyphs, copied in at build time) and its monospace,
 * which the desktop uses for technical readouts.
 */
val IconFont = FontFamily(Font(R.font.icons))
val Mono = IconFont

/** Font Awesome code points, as the desktop's `ui::icons`. */
object Icons {
    const val Undo = ""
    const val Redo = ""
    const val Open = ""
    const val Export = ""
    const val More = ""
    const val Image = ""
    const val Warning = ""
    const val Light = ""
    const val Colour = ""
    const val Presence = ""
    const val Crop = ""
    const val Sliders = ""
}

/** Material's components (menus, spinners) in the tokens' colours. */
@Composable
fun RawEditorTheme(content: @Composable () -> Unit) {
    MaterialTheme(
        colorScheme = darkColorScheme(
            primary = Colours.Accent,
            onPrimary = Colours.OnAccent,
            background = Colours.Chrome,
            onBackground = Colours.Text,
            surface = Colours.SurfaceRaised,
            onSurface = Colours.Text,
            surfaceVariant = Colours.Control,
            onSurfaceVariant = Colours.TextSecondary,
            outline = Colours.Border,
            error = Colours.Error,
        ),
        content = content,
    )
}
