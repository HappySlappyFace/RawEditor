use iced::futures::sink::SinkExt;

use crate::app::message::Message;

pub fn subscription(queued_loads: Vec<(i64, String)>) -> iced::Subscription<Message> {
    if let Some((id, path)) = queued_loads.first() {
        let id = *id;
        let path = path.clone();
        
        // Phase 81: Throttled Image Loader
        // We use Subscription::run to process one image at a time.
        iced::Subscription::run_with_id(
            id,
            iced::futures::stream::once(async move {
                let result = tokio::task::spawn_blocking(move || {
                    let img = image::open(&path).map_err(|e| e.to_string())?;
                    let rgba = img.to_rgba8();
                    Ok((rgba.width(), rgba.height(), rgba.into_raw()))
                }).await;

                let final_result = match result {
                    Ok(res) => res,
                    Err(e) => Err(e.to_string()),
                };
                
                Message::PreviewCached(id, final_result)
            })
        )
    } else {
        iced::Subscription::none()
    }
}
