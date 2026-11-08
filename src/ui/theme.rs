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
    /// Barely there: placeholders for empty slots.
    pub text_faint: Color,

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
    text_faint: Color::from_rgb(0.30, 0.30, 0.30),

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

/// The active theme's colours. Only the dark theme exists today; a light one
/// slots in here (a process-global choice, set from Preferences, like
/// `motion::set_reduced`).
pub fn colours() -> &'static Colours {
    &DARK
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
}
