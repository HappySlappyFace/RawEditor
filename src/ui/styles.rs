use iced::widget::{slider, container};
use iced::{Background, Border, Color, Theme};
use super::palette;

pub struct ProSlider;

impl ProSlider {
    pub fn style(_theme: &Theme, status: slider::Status) -> slider::Style {
        let rail_colors = (
            Background::Color(Color::from_rgb(0.25, 0.25, 0.25)),
            Background::Color(Color::from_rgb(0.25, 0.25, 0.25)),
        );

        let handle = match status {
            slider::Status::Active => slider::Handle {
                shape: slider::HandleShape::Circle { radius: 7.0 },
                background: Background::Color(Color::from_rgb(0.8, 0.8, 0.8)),
                border_color: Color::WHITE,
                border_width: 1.0,
            },
            slider::Status::Hovered => slider::Handle {
                shape: slider::HandleShape::Circle { radius: 7.0 },
                background: Background::Color(Color::from_rgb(0.9, 0.9, 0.9)),
                border_color: Color::WHITE,
                border_width: 1.0,
            },
            slider::Status::Dragged => slider::Handle {
                shape: slider::HandleShape::Circle { radius: 7.0 },
                background: Background::Color(Color::WHITE),
                border_color: Color::WHITE,
                border_width: 1.0,
            },
        };

        slider::Style {
            rail: slider::Rail {
                backgrounds: rail_colors,
                width: 4.0,
                border: Border {
                    radius: 2.0.into(),
                    width: 0.0,
                    color: Color::TRANSPARENT,
                },
            },
            handle,
        }
    }
}

pub struct WindowControlButton;

impl WindowControlButton {
    pub fn style(_theme: &Theme, status: iced::widget::button::Status) -> iced::widget::button::Style {
        match status {
            iced::widget::button::Status::Hovered => iced::widget::button::Style {
                background: Some(Background::Color(Color::from_rgb(0.2, 0.2, 0.2))),
                text_color: Color::WHITE,
                ..Default::default()
            },
            iced::widget::button::Status::Pressed => iced::widget::button::Style {
                background: Some(Background::Color(Color::from_rgb(0.3, 0.3, 0.3))),
                text_color: Color::WHITE,
                ..Default::default()
            },
            _ => iced::widget::button::Style {
                background: None,
                text_color: Color::from_rgb(0.7, 0.7, 0.7),
                ..Default::default()
            },
        }
    }
}

pub struct TabButton {
    pub is_active: bool,
}

impl TabButton {
    pub fn style(&self, _theme: &Theme, status: iced::widget::button::Status) -> iced::widget::button::Style {
        if self.is_active {
            iced::widget::button::Style {
                background: Some(Background::Color(Color::from_rgb(0.2, 0.2, 0.2))), // Grey
                text_color: Color::WHITE,
                ..Default::default()
            }
        } else {
            match status {
                iced::widget::button::Status::Hovered => iced::widget::button::Style {
                    background: Some(Background::Color(Color::from_rgb(0.2, 0.2, 0.2))),
                    text_color: Color::WHITE,
                    ..Default::default()
                },
                iced::widget::button::Status::Pressed => iced::widget::button::Style {
                    background: Some(Background::Color(Color::from_rgb(0.3, 0.3, 0.3))),
                    text_color: Color::WHITE,
                    ..Default::default()
                },
                _ => iced::widget::button::Style {
                    background: None,
                    text_color: Color::from_rgb(0.7, 0.7, 0.7),
                    ..Default::default()
                },
            }
        }
    }
}

pub struct NeutralButton;

impl NeutralButton {
    pub fn style(_theme: &Theme, status: iced::widget::button::Status) -> iced::widget::button::Style {
        match status {
            iced::widget::button::Status::Active => iced::widget::button::Style {
                background: Some(Background::Color(Color::from_rgb(0.15, 0.15, 0.15))),
                text_color: Color::from_rgb(0.7, 0.7, 0.7),
                border: Border {
                    radius: 4.0.into(),
                    width: 0.0,
                    color: Color::TRANSPARENT,
                },
                ..Default::default()
            },
            iced::widget::button::Status::Hovered => iced::widget::button::Style {
                background: Some(Background::Color(Color::from_rgb(0.25, 0.25, 0.25))),
                text_color: Color::from_rgb(0.9, 0.9, 0.9),
                border: Border {
                    radius: 4.0.into(),
                    width: 0.0,
                    color: Color::TRANSPARENT,
                },
                ..Default::default()
            },
            iced::widget::button::Status::Pressed => iced::widget::button::Style {
                background: Some(Background::Color(Color::from_rgb(0.1, 0.1, 0.1))),
                text_color: Color::from_rgb(0.5, 0.5, 0.5),
                border: Border {
                    radius: 4.0.into(),
                    width: 0.0,
                    color: Color::TRANSPARENT,
                },
                ..Default::default()
            },
            iced::widget::button::Status::Disabled => iced::widget::button::Style {
                background: None,
                text_color: Color::from_rgb(0.4, 0.4, 0.4),
                border: Border {
                    radius: 4.0.into(),
                    width: 0.0,
                    color: Color::TRANSPARENT,
                },
                ..Default::default()
            },

        }
    }
}

pub struct AccentButton;

impl AccentButton {
    pub fn style(_theme: &Theme, status: iced::widget::button::Status) -> iced::widget::button::Style {
        match status {
            iced::widget::button::Status::Active => iced::widget::button::Style {
                background: Some(Background::Color(palette::accent_chip_fill())),
                text_color: Color::WHITE,
                border: Border {
                    radius: 4.0.into(),
                    width: 1.0,
                    color: palette::accent_chip_border(),
                },
                ..Default::default()
            },
            iced::widget::button::Status::Hovered => iced::widget::button::Style {
                background: Some(Background::Color(palette::accent_sidebar_fill())),
                text_color: Color::WHITE,
                border: Border {
                    radius: 4.0.into(),
                    width: 1.0,
                    color: palette::accent_sidebar_border(),
                },
                ..Default::default()
            },
            iced::widget::button::Status::Pressed => iced::widget::button::Style {
                background: Some(Background::Color(palette::accent_selection_fill())),
                text_color: Color::WHITE,
                border: Border {
                    radius: 4.0.into(),
                    width: 1.0,
                    color: palette::accent_selection_border(),
                },
                ..Default::default()
            },
            iced::widget::button::Status::Disabled => iced::widget::button::Style {
                background: None,
                text_color: Color::from_rgb(0.4, 0.4, 0.4),
                border: Border {
                    radius: 4.0.into(),
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
                background: Some(Background::Color(Color::from_rgb(0.75, 0.15, 0.15))),
                text_color: Color::WHITE,
                border: Border {
                    radius: 4.0.into(),
                    width: 0.0,
                    color: Color::TRANSPARENT,
                },
                ..Default::default()
            },
            iced::widget::button::Status::Hovered => iced::widget::button::Style {
                // Matches the title-bar close-button hover color (layout.rs).
                background: Some(Background::Color(Color::from_rgb(0.9, 0.2, 0.2))),
                text_color: Color::WHITE,
                border: Border {
                    radius: 4.0.into(),
                    width: 0.0,
                    color: Color::TRANSPARENT,
                },
                ..Default::default()
            },
            iced::widget::button::Status::Pressed => iced::widget::button::Style {
                background: Some(Background::Color(Color::from_rgb(0.55, 0.1, 0.1))),
                text_color: Color::from_rgb(0.9, 0.9, 0.9),
                border: Border {
                    radius: 4.0.into(),
                    width: 0.0,
                    color: Color::TRANSPARENT,
                },
                ..Default::default()
            },
            iced::widget::button::Status::Disabled => iced::widget::button::Style {
                background: None,
                text_color: Color::from_rgb(0.4, 0.4, 0.4),
                border: Border {
                    radius: 4.0.into(),
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
        background: Background::Color(Color::from_rgb(0.2, 0.2, 0.2)),
        dot_color: Color::WHITE,
        border_width: 1.0,
        border_color: Color::from_rgb(0.5, 0.5, 0.5),
        text_color: Some(Color::WHITE),
    }
}

// ...

// ...

pub fn checkbox_style(_theme: &Theme, _status: iced::widget::checkbox::Status) -> iced::widget::checkbox::Style {
    iced::widget::checkbox::Style {
        background: Background::Color(Color::from_rgb(0.2, 0.2, 0.2)),
        // checkmark_color removed
        border: Border {
            radius: 3.0.into(),
            width: 1.0,
            color: Color::from_rgb(0.5, 0.5, 0.5),
        },
        text_color: Some(Color::WHITE),
        icon_color: Color::WHITE,
    }
}

pub fn text_input_style(_theme: &Theme, _status: iced::widget::text_input::Status) -> iced::widget::text_input::Style {
    iced::widget::text_input::Style {
        background: Background::Color(Color::from_rgb(0.15, 0.15, 0.15)),
        border: Border {
            radius: 4.0.into(),
            width: 1.0,
            color: Color::from_rgb(0.3, 0.3, 0.3),
        },
        icon: Color::from_rgb(0.5, 0.5, 0.5),
        placeholder: Color::from_rgb(0.4, 0.4, 0.4),
        value: Color::WHITE,
        selection: palette::accent_selection_border(),
    }
}

pub fn modal_container_style(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(Color::from_rgb(0.1, 0.1, 0.1))),
        border: Border {
            radius: 10.0.into(),
            width: 1.0,
            color: Color::from_rgb(0.3, 0.3, 0.3),
        },
        text_color: Some(Color::WHITE),
        ..Default::default()
    }
}

pub fn button_style(theme: &Theme, status: iced::widget::button::Status) -> iced::widget::button::Style {
    NeutralButton::style(theme, status)
}

pub fn button_confirm() -> impl Fn(&Theme, iced::widget::button::Status) -> iced::widget::button::Style {
    AccentButton::style
}

pub fn button_cancel() -> impl Fn(&Theme, iced::widget::button::Status) -> iced::widget::button::Style {
    NeutralButton::style
}
