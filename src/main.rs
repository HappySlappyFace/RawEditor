use iced::{Element, Task, Theme};
use iced::widget::{column, container, text};
use iced::alignment::{Horizontal, Vertical};
use iced::{Alignment, Length};

// Declare the state module
mod state;

/// Main application state
struct RawEditor {
    /// The catalog database
    library: state::library::Library,
}

/// Application messages (events)
#[derive(Debug, Clone)]
enum Message {
    // Placeholder for future messages
}

impl RawEditor {
    /// Create a new instance of the application
    fn new() -> (Self, Task<Message>) {
        // Initialize the database
        // If this fails, we panic because the app cannot function without its database
        let library = state::library::Library::new()
            .expect("Failed to initialize database. Check permissions and disk space.");
        
        println!("🎨 RAW Editor initialized with {} images", 
                 library.image_count().unwrap_or(0));
        
        (
            RawEditor { library },
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
            text("RAW Editor v0.0.2")
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
