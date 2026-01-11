use crate::app::message::Message;

pub fn subscription(queued_loads: Vec<(i64, String)>) -> iced::Subscription<Message> {
    if let Some((id, path)) = queued_loads.first() {
        iced::Subscription::run_with_id(
            *id,
            iced::futures::stream::once(process_load_request(*id, path.clone()))
        )
    } else {
        iced::Subscription::none()
    }
}

async fn process_load_request(id: i64, path: String) -> Message {
    let result = tokio::task::spawn_blocking(move || {
        // Phase 82: High-Performance Decoding
        // Try to decode with zune-jpeg first for speed
        let zune_result = (|| -> Result<(u32, u32, Vec<u8>), String> {
            let file_bytes = std::fs::read(&path).map_err(|e| e.to_string())?;
            let mut decoder = zune_jpeg::JpegDecoder::new(&file_bytes);
            
            // Decode to RGB (usually)
            let pixels = decoder.decode().map_err(|e| e.to_string())?;
            let info = decoder.info().ok_or("No zune-jpeg info")?;
            
            let width = info.width as u32;
            let height = info.height as u32;
            
            // zune-jpeg typically returns RGB8. We need RGBA8 for Iced.
            // Ideally we check info.options.color_space but for now assume RGB if length matches
            if pixels.len() == (width * height * 3) as usize {
                let mut rgba = Vec::with_capacity((width * height * 4) as usize);
                for chunk in pixels.chunks_exact(3) {
                    rgba.extend_from_slice(chunk);
                    rgba.push(255); // Alpha
                }
                Ok((width, height, rgba))
            } else {
                // Grayscale or CMYK or already RGBA? Fallback to image crate to be safe
                Err(format!("Unsupported zune-jpeg output length: {}", pixels.len()))
            }
        })();

        match zune_result {
            Ok(res) => Ok(res),
            Err(_) => {
                // Fallback to image crate if zune fails
                let img = image::open(&path).map_err(|e| e.to_string())?;
                let rgba = img.to_rgba8();
                Ok((rgba.width(), rgba.height(), rgba.into_raw()))
            }
        }
    }).await;

    let final_result = match result {
        Ok(res) => res,
        Err(e) => Err(e.to_string()),
    };
    
    Message::PreviewCached(id, final_result)
}
