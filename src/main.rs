use iced::{Element, Task, Theme};
use iced::widget::{column, container, text};
use iced::alignment::{Horizontal, Vertical};
use iced::{Alignment, Length};

/// Main application state
struct RawEditor {
    // For now, this is empty. We'll add state fields as we build out features.
}

/// Application messages (events)
#[derive(Debug, Clone)]
enum Message {
    // Placeholder for future messages
}

impl RawEditor {
    /// Create a new instance of the application
    fn new() -> (Self, Task<Message>) {
        (
            RawEditor {},
            Task::none(),
        )
    }

    /// Handle application messages and update state
    fn update(&mut self, _message: Message) -> Task<Message> {
        Task::none()
    }

    /// Build the user interface
    fn view(&self) -> Element<Message> {
        let content = column![
            text("RAW Editor v0.0.1")
                .size(48)
                .width(Length::Fill)
                .height(Length::Fill)
                .align_x(Horizontal::Center)
                .align_y(Vertical::Center),
        ]
        .width(Length::Fill)
        .height(Length::Fill)
        .align_x(Alignment::Center);

        container(content)
            .width(Length::Fill)
            .height(Length::Fill)
            .center_x(Length::Fill)
            .center_y(Length::Fill)
            .into()
    }

    /// Set the application theme
    fn theme(&self) -> Theme {
        Theme::Dark
    }
}

fn main() -> iced::Result {
    iced::application(
        "RAW Editor",
        RawEditor::update,
        RawEditor::view,
    )
    .theme(RawEditor::theme)
    .centered()
    .run_with(RawEditor::new)
}
