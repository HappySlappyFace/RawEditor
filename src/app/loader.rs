use crate::app::message::Message;

/// Drive the session's background loads (`Session::load_jobs`).
///
/// Each job gets its own `run_with_id` key, so iced keeps them as distinct
/// subscriptions and runs them concurrently, each exactly once while it stays
/// listed. Previously only the queue's first entry was driven, which
/// serialized every decode no matter how many cores were free or how deep
/// the preload queue was.
pub fn subscription(session: &crate::session::Session) -> iced::Subscription<Message> {
    iced::Subscription::batch(session.load_jobs().map(|job| {
        let key = job.clone();
        iced::Subscription::run_with_id(
            key,
            iced::futures::stream::once(async move { Message::Session(job.run().await) }),
        )
    }))
}
