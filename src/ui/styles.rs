use iced::widget::slider;
use iced::{Background, Border, Color, Theme};
use super::theme;

pub struct ProSlider;

impl ProSlider {
    pub fn style(_theme: &Theme, status: slider::Status) -> slider::Style {
        let c = theme::colours();
        let fill = match status {
            slider::Status::Active => c.text,
            slider::Status::Hovered | slider::Status::Dragged => c.text_strong,
        };
        slider::Style {
            rail: slider::Rail {
                backgrounds: (Background::Color(c.control_active), Background::Color(c.control_active)),
                width: 4.0,
                border: Border { radius: 2.0.into(), width: 0.0, color: Color::TRANSPARENT },
            },
            handle: slider::Handle {
                shape: slider::HandleShape::Circle { radius: 7.0 },
                background: Background::Color(fill),
                border_color: c.text_strong,
                border_width: 1.0,
            },
        }
    }
}

pub struct WindowControlButton;

impl WindowControlButton {
    pub fn style(_theme: &Theme, status: iced::widget::button::Status) -> iced::widget::button::Style {
        match status {
            iced::widget::button::Status::Hovered => iced::widget::button::Style {
                background: Some(Background::Color(theme::colours().control_hover)),
                text_color: theme::colours().text_strong,
                ..Default::default()
            },
            iced::widget::button::Status::Pressed => iced::widget::button::Style {
                background: Some(Background::Color(theme::colours().control_active)),
                text_color: theme::colours().text_strong,
                ..Default::default()
            },
            _ => iced::widget::button::Style {
                background: None,
                text_color: theme::colours().text_secondary,
                ..Default::default()
            },
        }
    }
}

pub struct TabButton {
    pub is_active: bool,
}

impl TabButton {
    /// The active tab draws no background of its own: the sliding highlight
    /// (`ui::widgets::indicator`) beneath it is the active state. Hover is a
    /// translucent tint for the same reason — an opaque one would hide the
    /// highlight as it slides past.
    pub fn style(&self, _theme: &Theme, status: iced::widget::button::Status) -> iced::widget::button::Style {
        use iced::widget::button::{Status, Style};
        let wash = theme::colours().hover_wash;
        let tint = |a: f32| Some(Background::Color(Color { a: wash.a * a / 0.05, ..wash }));
        match (self.is_active, status) {
            (true, _) => Style { background: None, text_color: theme::colours().text_strong, ..Default::default() },
            (false, Status::Hovered) => Style {
                background: tint(0.05),
                text_color: theme::colours().text,
                ..Default::default()
            },
            (false, Status::Pressed) => Style {
                background: tint(0.08),
                text_color: theme::colours().text_strong,
                ..Default::default()
            },
            (false, _) => Style {
                background: None,
                text_color: theme::colours().text_muted,
                ..Default::default()
            },
        }
    }
}

pub struct NeutralButton;

impl NeutralButton {
    /// Charcoal with a 1 px outline: the outline is what separates it from a
    /// panel of the same shade (the Develop sidebar).
    pub fn style(_theme: &Theme, status: iced::widget::button::Status) -> iced::widget::button::Style {
        use iced::widget::button::{Status, Style};
        let c = theme::colours();
        let (background, text_color, edge) = match status {
            Status::Active => (Some(c.control), c.text_secondary, c.border),
            Status::Hovered => (Some(c.control_active), c.text, c.border_strong),
            Status::Pressed => (Some(c.surface), c.text_muted, c.border),
            Status::Disabled => (None, c.text_disabled, c.border_subtle),
        };
        Style {
            background: background.map(Background::Color),
            text_color,
            border: Border { radius: theme::radius::CONTROL.into(), width: 1.0, color: edge },
            ..Default::default()
        }
    }
}

pub struct AccentButton;

impl AccentButton {
    pub fn style(_theme: &Theme, status: iced::widget::button::Status) -> iced::widget::button::Style {
        match status {
            iced::widget::button::Status::Active => iced::widget::button::Style {
                background: Some(Background::Color(theme::colours().accent_fill)),
                text_color: theme::colours().on_accent,
                border: Border {
                    radius: theme::radius::CONTROL.into(),
                    width: 1.0,
                    color: theme::colours().accent_border,
                },
                ..Default::default()
            },
            iced::widget::button::Status::Hovered => iced::widget::button::Style {
                background: Some(Background::Color(theme::colours().accent_fill_hover)),
                text_color: theme::colours().on_accent,
                border: Border {
                    radius: theme::radius::CONTROL.into(),
                    width: 1.0,
                    color: theme::colours().accent_border_hover,
                },
                ..Default::default()
            },
            iced::widget::button::Status::Pressed => iced::widget::button::Style {
                background: Some(Background::Color(theme::colours().accent_selection)),
                text_color: theme::colours().on_accent,
                border: Border {
                    radius: theme::radius::CONTROL.into(),
                    width: 1.0,
                    color: theme::colours().accent_selection_border,
                },
                ..Default::default()
            },
            iced::widget::button::Status::Disabled => iced::widget::button::Style {
                background: None,
                text_color: theme::colours().text_disabled,
                border: Border {
                    radius: theme::radius::CONTROL.into(),
                    width: 0.0,
                    color: Color::TRANSPARENT,
                },
                ..Default::default()
            },
        }
    }
}

/// Destructive-action button (e.g. "Delete from Disk"). Red, distinct from
/// AccentButton's neutral-accent styling.
pub struct DangerButton;

impl DangerButton {
    pub fn style(_theme: &Theme, status: iced::widget::button::Status) -> iced::widget::button::Style {
        match status {
            iced::widget::button::Status::Active => iced::widget::button::Style {
                background: Some(Background::Color(theme::colours().danger)),
                text_color: theme::colours().on_accent,
                border: Border {
                    radius: theme::radius::CONTROL.into(),
                    width: 0.0,
                    color: Color::TRANSPARENT,
                },
                ..Default::default()
            },
            iced::widget::button::Status::Hovered => iced::widget::button::Style {
                // Matches the title-bar close-button hover color (layout.rs).
                background: Some(Background::Color(theme::colours().danger_hover)),
                text_color: theme::colours().on_accent,
                border: Border {
                    radius: theme::radius::CONTROL.into(),
                    width: 0.0,
                    color: Color::TRANSPARENT,
                },
                ..Default::default()
            },
            iced::widget::button::Status::Pressed => iced::widget::button::Style {
                background: Some(Background::Color(theme::colours().danger_pressed)),
                text_color: theme::colours().text,
                border: Border {
                    radius: theme::radius::CONTROL.into(),
                    width: 0.0,
                    color: Color::TRANSPARENT,
                },
                ..Default::default()
            },
            iced::widget::button::Status::Disabled => iced::widget::button::Style {
                background: None,
                text_color: theme::colours().text_disabled,
                border: Border {
                    radius: theme::radius::CONTROL.into(),
                    width: 0.0,
                    color: Color::TRANSPARENT,
                },
                ..Default::default()
            },
        }
    }
}

pub fn radio_style(_theme: &Theme, _status: iced::widget::radio::Status) -> iced::widget::radio::Style {
    iced::widget::radio::Style {
        background: Background::Color(theme::colours().control_hover),
        dot_color: theme::colours().text_strong,
        border_width: 1.0,
        border_color: theme::colours().border_control,
        text_color: Some(theme::colours().text_strong),
    }
}

// ...

// ...

pub fn checkbox_style(_theme: &Theme, _status: iced::widget::checkbox::Status) -> iced::widget::checkbox::Style {
    iced::widget::checkbox::Style {
        background: Background::Color(theme::colours().control_hover),
        // checkmark_color removed
        border: Border {
            radius: theme::radius::CONTROL.into(),
            width: 1.0,
            color: theme::colours().border_control,
        },
        text_color: Some(theme::colours().text_strong),
        icon_color: theme::colours().text_strong,
    }
}

pub fn text_input_style(_theme: &Theme, _status: iced::widget::text_input::Status) -> iced::widget::text_input::Style {
    iced::widget::text_input::Style {
        background: Background::Color(theme::colours().control),
        border: Border {
            radius: theme::radius::CONTROL.into(),
            width: 1.0,
            color: theme::colours().border_strong,
        },
        icon: theme::colours().text_muted,
        placeholder: theme::colours().text_disabled,
        value: theme::colours().text_strong,
        selection: theme::colours().accent_selection_border,
    }
}

/// The one dialog surface, drawn by `ui::widgets::modal::Sheet` so it can
/// fade. A deep, soft shadow lifts it off the dimmed app.
pub fn modal_surface() -> crate::ui::widgets::modal::Surface {
    crate::ui::widgets::modal::Surface {
        background: theme::colours().surface,
        border: Border {
            radius: theme::radius::DIALOG.into(),
            width: 1.0,
            color: theme::colours().border,
        },
        shadow: iced::Shadow {
            color: Color::from_rgba(0.0, 0.0, 0.0, 0.6),
            offset: iced::Vector::new(0.0, 18.0),
            blur_radius: 44.0,
        },
    }
}




/// Dropdown (`pick_list`) in the sidebar's vocabulary: the neutral button's
/// charcoal, an orange caret, the accent border while hovered or open.
pub fn pick_list_style(
    _theme: &Theme,
    status: iced::widget::pick_list::Status,
) -> iced::widget::pick_list::Style {
    use iced::widget::pick_list::Status;
    let lit = matches!(status, Status::Hovered | Status::Opened);
    iced::widget::pick_list::Style {
        text_color: theme::colours().text,
        placeholder_color: theme::colours().text_muted,
        handle_color: theme::colours().accent_text,
        background: Background::Color(if lit {
            theme::colours().control_hover
        } else {
            theme::colours().control
        }),
        border: Border {
            radius: theme::radius::CONTROL.into(),
            width: 1.0,
            color: if lit { theme::colours().accent_selection_border } else { theme::colours().border },
        },
    }
}

/// The open dropdown's list, matching [`pick_list_style`].
pub fn pick_list_menu_style(_theme: &Theme) -> iced::overlay::menu::Style {
    iced::overlay::menu::Style {
        background: Background::Color(theme::colours().surface_raised),
        border: Border { radius: theme::radius::CONTROL.into(), width: 1.0, color: theme::colours().border_strong },
        text_color: theme::colours().text,
        selected_text_color: theme::colours().text_strong,
        selected_background: Background::Color(theme::colours().accent_selection),
    }
}

/// A button with no chrome until hovered: section headers, and small icon
/// actions that sit inside a row of text (a section's reset).
pub fn ghost_button(_theme: &Theme, status: iced::widget::button::Status) -> iced::widget::button::Style {
    use iced::widget::button::{Status, Style};
    let c = theme::colours();
    let (background, text_color) = match status {
        Status::Hovered => (Some(Background::Color(c.control_hover)), c.text_strong),
        Status::Pressed => (Some(Background::Color(c.control_active)), c.text_strong),
        Status::Active => (None, c.text),
        Status::Disabled => (None, c.text_disabled),
    };
    Style {
        background,
        text_color,
        border: Border { radius: theme::radius::CONTROL.into(), width: 0.0, color: Color::TRANSPARENT },
        ..Default::default()
    }
}

/// The small label shown by `tooltip`.
pub fn tooltip_style(_theme: &Theme) -> iced::widget::container::Style {
    let c = theme::colours();
    iced::widget::container::Style {
        background: Some(Background::Color(c.surface)),
        text_color: Some(c.text),
        border: Border { radius: theme::radius::CONTROL.into(), width: 1.0, color: c.border_strong },
        ..Default::default()
    }
}

/// A filter chip (Library's rating filter): a pill, accent-filled when on.
pub fn chip(active: bool) -> impl Fn(&Theme, iced::widget::button::Status) -> iced::widget::button::Style {
    move |_theme, status| {
        use iced::widget::button::{Status, Style};
        let c = theme::colours();
        let (background, text_color, edge) = match (active, status) {
            (true, _) => (c.accent_fill, c.on_accent, c.accent_border),
            (false, Status::Hovered | Status::Pressed) => (c.control_hover, c.text, c.border_strong),
            (false, _) => (c.control, c.text_secondary, c.border),
        };
        Style {
            background: Some(Background::Color(background)),
            text_color,
            border: Border { radius: theme::radius::PILL.into(), width: 1.0, color: edge },
            ..Default::default()
        }
    }
}

/// A row in a selectable list (Library's folders): quiet until hovered,
/// accent-filled when it is the current one.
pub fn list_item(active: bool) -> impl Fn(&Theme, iced::widget::button::Status) -> iced::widget::button::Style {
    move |_theme, status| {
        use iced::widget::button::{Status, Style};
        let c = theme::colours();
        let (background, text_color, edge) = match (active, status) {
            (true, _) => (c.accent_fill_hover, c.on_accent, c.accent_border_hover),
            (false, Status::Hovered | Status::Pressed) => (c.control_hover, c.text, c.border_strong),
            (false, _) => (c.control, c.text_secondary, c.border_subtle),
        };
        Style {
            background: Some(Background::Color(background)),
            text_color,
            border: Border { radius: theme::radius::CONTROL.into(), width: 1.0, color: edge },
            ..Default::default()
        }
    }
}

/// Divider lines (`horizontal_rule(1).style(styles::rule)`).
pub fn rule(_theme: &Theme) -> iced::widget::rule::Style {
    iced::widget::rule::Style {
        color: theme::colours().border_strong,
        width: 1,
        radius: 0.0.into(),
        fill_mode: iced::widget::rule::FillMode::Full,
    }
}
