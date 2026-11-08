//! The design tokens: every colour, text size, spacing step and corner
//! radius the UI uses, by role.
//!
//! Colours are looked up at runtime through [`colours()`], never copied into
//! constants at call sites, so a second theme (light mode) is one more
//! [`Colours`] value plus a way to pick it: no view changes. Name colours by
//! what they are FOR (`text_muted`, `surface`), never by how they look
//! (`grey_50`), or a light theme can't reinterpret them.
//!
//! Colours drawn over photos (scrims, badges on thumbnails, canvas overlays)
//! are not themed: a photo is the same in either theme, so they stay literal
//! where they're drawn.

use iced::Color;

/// Every colour role in the UI.
#[derive(Debug, Clone, Copy)]
pub struct Colours {
    // ── Surfaces, darkest to lightest (dark theme) ──
    /// The window chrome: the title bar.
    pub chrome: Color,
    /// The app background behind panels and the image.
    pub bg: Color,
    /// Dialogs and other floating panels.
    pub surface: Color,
    /// Cards and grouped blocks on a surface.
    pub surface_raised: Color,
    /// Side panels (the Develop sidebar).
    pub panel: Color,
    /// Around the photo in Develop. A neutral grey in both themes, never
    /// white: the surround changes how bright the photo looks.
    pub canvas: Color,
    /// Buttons, inputs and dropdowns at rest. On a panel of the same shade
    /// a control is told apart by its outline.
    pub control: Color,
    /// … hovered.
    pub control_hover: Color,
    /// … pressed or open.
    pub control_active: Color,

    // ── Lines ──
    pub border_subtle: Color,
    pub border: Color,
    /// Dividers and a dialog's own outline.
    pub border_strong: Color,
    /// The outline of a checkbox or radio, which must read as a target.
    pub border_control: Color,

    // ── Text, most to least prominent ──
    /// Selected / active labels.
    pub text_strong: Color,
    pub text: Color,
    pub text_secondary: Color,
    /// Section labels, captions.
    pub text_muted: Color,
    /// One-line explanations under a control.
    pub text_hint: Color,
    pub text_disabled: Color,
    /// Text on an accent or danger fill: white in every theme.
    pub on_accent: Color,
    /// Text on a dark scrim over a photo (badges, the info overlay): white in
    /// every theme, because the photo doesn't change with the theme.
    pub on_photo: Color,
    /// A translucent hover tint for controls that must let what is beneath
    /// show through (the title-bar tabs over their sliding highlight).
    pub hover_wash: Color,

    // ── Accent (the electric orange) ──
    /// Full strength, for small high-signal marks (active tab underline,
    /// the curve's handles).
    pub accent: Color,
    /// Accent-coloured text and glyphs on dark surfaces.
    pub accent_text: Color,
    /// An accent button / active chip.
    pub accent_fill: Color,
    pub accent_border: Color,
    pub accent_fill_hover: Color,
    pub accent_border_hover: Color,
    /// Selected rows and pressed accent controls: a dim orange wash.
    pub accent_selection: Color,
    pub accent_selection_border: Color,

    // ── Status ──
    /// Status text: done, installed, a measured value.
    pub ok: Color,
    pub warn: Color,
    pub error: Color,
    /// Destructive buttons.
    pub danger: Color,
    pub danger_hover: Color,
    pub danger_pressed: Color,
}

pub const DARK: Colours = Colours {
    chrome: Color::from_rgb(0.05, 0.05, 0.05),
    bg: Color::from_rgb(0.08, 0.08, 0.08),
    surface: Color::from_rgb(0.10, 0.10, 0.10),
    surface_raised: Color::from_rgb(0.13, 0.13, 0.13),
    panel: Color::from_rgb(0.15, 0.15, 0.15),
    canvas: Color::from_rgb(0.13, 0.13, 0.13),
    control: Color::from_rgb(0.15, 0.15, 0.15),
    control_hover: Color::from_rgb(0.20, 0.20, 0.20),
    control_active: Color::from_rgb(0.25, 0.25, 0.25),

    border_subtle: Color::from_rgb(0.20, 0.20, 0.20),
    border: Color::from_rgb(0.25, 0.25, 0.25),
    border_strong: Color::from_rgb(0.30, 0.30, 0.30),
    border_control: Color::from_rgb(0.45, 0.45, 0.45),

    text_strong: Color::WHITE,
    text: Color::from_rgb(0.88, 0.88, 0.88),
    text_secondary: Color::from_rgb(0.70, 0.70, 0.70),
    text_muted: Color::from_rgb(0.50, 0.50, 0.50),
    text_hint: Color::from_rgb(0.45, 0.45, 0.45),
    text_disabled: Color::from_rgb(0.40, 0.40, 0.40),
    on_accent: Color::WHITE,
    on_photo: Color::WHITE,
    hover_wash: Color::from_rgba(1.0, 1.0, 1.0, 0.05),

    accent: Color::from_rgb(0.976, 0.451, 0.086),
    accent_text: Color::from_rgb(0.92, 0.63, 0.34),
    accent_fill: Color::from_rgb(0.52, 0.30, 0.12),
    accent_border: Color::from_rgb(0.72, 0.45, 0.22),
    accent_fill_hover: Color::from_rgb(0.50, 0.27, 0.10),
    accent_border_hover: Color::from_rgb(0.70, 0.41, 0.18),
    accent_selection: Color::from_rgb(0.29, 0.19, 0.12),
    accent_selection_border: Color::from_rgb(0.82, 0.51, 0.24),

    ok: Color::from_rgb(0.55, 0.75, 0.55),
    warn: Color::from_rgb(0.90, 0.55, 0.30),
    error: Color::from_rgb(0.95, 0.40, 0.35),
    danger: Color::from_rgb(0.75, 0.15, 0.15),
    danger_hover: Color::from_rgb(0.90, 0.20, 0.20),
    danger_pressed: Color::from_rgb(0.55, 0.10, 0.10),
};

/// Light: the same roles, inverted around a light grey. Surfaces get lighter
/// as they rise (bg < panel < surface), as they do in the dark theme. The
/// accent is darkened where it is text, to stay legible on white.
pub const LIGHT: Colours = Colours {
    chrome: Color::from_rgb(0.86, 0.86, 0.86),
    bg: Color::from_rgb(0.90, 0.90, 0.90),
    surface: Color::from_rgb(0.97, 0.97, 0.97),
    surface_raised: Color::from_rgb(1.0, 1.0, 1.0),
    panel: Color::from_rgb(0.94, 0.94, 0.94),
    canvas: Color::from_rgb(0.60, 0.60, 0.60),
    control: Color::from_rgb(0.94, 0.94, 0.94),
    control_hover: Color::from_rgb(0.88, 0.88, 0.88),
    control_active: Color::from_rgb(0.82, 0.82, 0.82),

    border_subtle: Color::from_rgb(0.84, 0.84, 0.84),
    border: Color::from_rgb(0.76, 0.76, 0.76),
    border_strong: Color::from_rgb(0.68, 0.68, 0.68),
    border_control: Color::from_rgb(0.50, 0.50, 0.50),

    text_strong: Color::BLACK,
    text: Color::from_rgb(0.10, 0.10, 0.10),
    text_secondary: Color::from_rgb(0.30, 0.30, 0.30),
    text_muted: Color::from_rgb(0.44, 0.44, 0.44),
    text_hint: Color::from_rgb(0.48, 0.48, 0.48),
    text_disabled: Color::from_rgb(0.62, 0.62, 0.62),
    on_accent: Color::WHITE,
    on_photo: Color::WHITE,
    hover_wash: Color::from_rgba(0.0, 0.0, 0.0, 0.05),

    accent: Color::from_rgb(0.976, 0.451, 0.086),
    accent_text: Color::from_rgb(0.76, 0.32, 0.02),
    accent_fill: Color::from_rgb(0.90, 0.42, 0.10),
    accent_border: Color::from_rgb(0.78, 0.34, 0.05),
    accent_fill_hover: Color::from_rgb(0.95, 0.48, 0.15),
    accent_border_hover: Color::from_rgb(0.82, 0.38, 0.08),
    accent_selection: Color::from_rgb(1.0, 0.89, 0.78),
    accent_selection_border: Color::from_rgb(0.93, 0.52, 0.18),

    ok: Color::from_rgb(0.14, 0.52, 0.20),
    warn: Color::from_rgb(0.78, 0.40, 0.04),
    error: Color::from_rgb(0.78, 0.16, 0.13),
    danger: Color::from_rgb(0.80, 0.15, 0.15),
    danger_hover: Color::from_rgb(0.90, 0.20, 0.20),
    danger_pressed: Color::from_rgb(0.60, 0.10, 0.10),
};

/// Which palette the UI draws with (Preferences › General › Appearance).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Appearance {
    #[default]
    Dark,
    Light,
}

impl Appearance {
    pub const ALL: [Appearance; 2] = [Appearance::Dark, Appearance::Light];

    /// The name saved in settings. Never rename one.
    pub fn name(self) -> &'static str {
        match self {
            Appearance::Dark => "dark",
            Appearance::Light => "light",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|a| a.name() == name)
    }
}

impl std::fmt::Display for Appearance {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Appearance::Dark => "Dark",
            Appearance::Light => "Light",
        })
    }
}

/// Process-global, like `motion::set_reduced`: every style function reads it
/// through `colours()`, and the next view rebuild picks it up.
static LIGHT_ACTIVE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn set_appearance(a: Appearance) {
    LIGHT_ACTIVE.store(a == Appearance::Light, std::sync::atomic::Ordering::Relaxed);
}

pub fn appearance() -> Appearance {
    if LIGHT_ACTIVE.load(std::sync::atomic::Ordering::Relaxed) {
        Appearance::Light
    } else {
        Appearance::Dark
    }
}

/// The active theme's colours.
pub fn colours() -> &'static Colours {
    match appearance() {
        Appearance::Dark => &DARK,
        Appearance::Light => &LIGHT,
    }
}

/// iced's own theme, built from the tokens, so every widget without a style
/// of its own (scrollbars, progress bars, a forgotten checkbox) still draws
/// in the app's colours instead of iced's stock blue.
pub fn iced_theme() -> iced::Theme {
    let c = colours();
    iced::Theme::custom(
        "RawEditor".to_string(),
        iced::theme::Palette {
            background: c.bg,
            text: c.text,
            primary: c.accent,
            success: c.ok,
            danger: c.danger,
        },
    )
}

/// Fonts. Regular text is the app's default font (JetBrains Mono Nerd Font,
/// which also carries the icons); bold text must name its family too, or
/// iced asks the OS for a default sans-serif and every platform shows a
/// different face.
pub mod font {
    use iced::font::{Family, Font, Weight};

    /// Headings and emphasis: JetBrains Mono Bold, bundled (OFL-1.1).
    pub const BOLD: Font = Font { family: Family::Name("JetBrains Mono"), weight: Weight::Bold, ..Font::DEFAULT };
    pub const BOLD_BYTES: &[u8] = include_bytes!("../../assets/fonts/JetBrainsMono-Bold.ttf");
}

/// Text sizes. The values are the ones the UI already used most.
pub mod size {
    /// Hints, badges, dense metadata.
    pub const CAPTION: u16 = 11;
    /// Most sidebar labels and controls.
    pub const SMALL: u16 = 12;
    /// Body text in dialogs and Preferences.
    pub const BODY: u16 = 13;
    /// Section headings in a sidebar.
    pub const LABEL: u16 = 14;
    /// A page's heading, and icon buttons.
    pub const HEADING: u16 = 16;
    /// Dialog titles.
    pub const TITLE: u16 = 18;
    /// Empty-state messages and their icons.
    pub const DISPLAY: u16 = 24;
}

/// Spacing steps for `spacing`/`padding`.
pub mod space {
    pub const XXS: u16 = 2;
    pub const XS: u16 = 4;
    pub const S: u16 = 6;
    pub const M: u16 = 8;
    pub const L: u16 = 10;
    pub const XL: u16 = 12;
    pub const XXL: u16 = 16;
}

/// Corner radii. Images are always square-cornered (0), by design.
pub mod radius {
    /// Buttons, inputs, dropdowns.
    pub const CONTROL: f32 = 4.0;
    /// Cards, toolbars, panels.
    pub const CARD: f32 = 6.0;
    /// Dialogs and floating badges.
    pub const DIALOG: f32 = 10.0;
    /// Fully round ends (chips, pills).
    pub const PILL: f32 = 14.0;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_iced_theme_uses_the_accent_not_iceds_blue() {
        let t = iced_theme();
        assert_eq!(t.palette().primary, DARK.accent);
        assert_eq!(t.palette().background, DARK.bg);
    }

    /// WCAG relative luminance and contrast ratio.
    fn contrast(a: Color, b: Color) -> f32 {
        let lin = |c: f32| if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) };
        let l = |c: Color| 0.2126 * lin(c.r) + 0.7152 * lin(c.g) + 0.0722 * lin(c.b);
        let (x, y) = (l(a), l(b));
        (x.max(y) + 0.05) / (x.min(y) + 0.05)
    }

    /// Both palettes keep their text readable on every surface it sits on:
    /// 4.5:1 for body text (WCAG AA), 3:1 for captions, hints and labels on
    /// coloured buttons. A light palette is easy to get subtly wrong.
    #[test]
    fn both_palettes_keep_text_readable() {
        for (name, c) in [("dark", DARK), ("light", LIGHT)] {
            for (surface_name, surface) in
                [("bg", c.bg), ("panel", c.panel), ("surface", c.surface), ("control", c.control)]
            {
                for (role, colour, min) in [
                    ("text", c.text, 4.5),
                    ("text_secondary", c.text_secondary, 4.5),
                    ("text_muted", c.text_muted, 3.0),
                    ("text_hint", c.text_hint, 3.0),
                    ("accent_text", c.accent_text, 3.0),
                ] {
                    let r = contrast(colour, surface);
                    assert!(r >= min, "{name}: {role} on {surface_name} is {r:.2}:1, needs {min}:1");
                }
            }
            for (fill_name, fill) in [("accent_fill", c.accent_fill), ("danger", c.danger)] {
                let r = contrast(c.on_accent, fill);
                assert!(r >= 3.0, "{name}: on_accent on {fill_name} is {r:.2}:1");
            }
        }
    }
}
