use iced::widget::slider;
use iced::{Background, Border, Color, Theme};

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
