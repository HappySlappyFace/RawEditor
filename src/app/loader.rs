use crate::app::message::Message;

pub fn subscription(
    queued_loads: Vec<(i64, String)>,
    queued_raw_loads: Vec<(i64, String)>,
) -> iced::Subscription<Message> {
    let preview_subscription = if let Some((id, path)) = queued_loads.first() {
        iced::Subscription::run_with_id(
            ("preview", *id),
            iced::futures::stream::once(process_load_request(*id, path.clone())),
        )
    } else {
        iced::Subscription::none()
    };

    let raw_subscription = if let Some((id, path)) = queued_raw_loads.first() {
        iced::Subscription::run_with_id(
            ("raw", *id),
            iced::futures::stream::once(process_raw_load_request(*id, path.clone())),
        )
    } else {
        iced::Subscription::none()
    };

    iced::Subscription::batch(vec![preview_subscription, raw_subscription])
}

async fn process_load_request(id: i64, path: String) -> Message {
    let result = tokio::task::spawn_blocking(move || {
        #[cfg(feature = "fast-jpeg")]
        {
            let zune_result = (|| -> Result<(u32, u32, Vec<u8>), String> {
                use zune_core::{colorspace::ColorSpace, options::DecoderOptions};
                use zune_jpeg::JpegDecoder;

                let file_bytes = std::fs::read(&path).map_err(|e| e.to_string())?;
                let opts = DecoderOptions::default().jpeg_set_out_colorspace(ColorSpace::RGB);
                let mut decoder = JpegDecoder::new_with_options(&file_bytes, opts);
                let pixels = decoder.decode().map_err(|e| format!("{e:?}"))?;
                let (width, height) = decoder.dimensions().ok_or("no dimensions")?;
                let width = width as u32;
                let height = height as u32;
                let expected = (width as usize) * (height as usize) * 3;
                if pixels.len() == expected {
                    let mut rgba = Vec::with_capacity((width * height * 4) as usize);
                    for chunk in pixels.chunks_exact(3) {
                        rgba.extend_from_slice(chunk);
                        rgba.push(255);
                    }
                    Ok((width, height, rgba))
                } else {
                    Err(format!("unexpected pixel length: {}", pixels.len()))
                }
            })();

            match zune_result {
                Ok(res) => return Ok(res),
                Err(e) => tracing::debug!("zune-jpeg fallback: {}", e),
            }
        }

        let img = image::open(&path).map_err(|e| e.to_string())?;
        let rgba = img.to_rgba8();
        Ok((rgba.width(), rgba.height(), rgba.into_raw()))
    }).await;

    let final_result = match result {
        Ok(Ok((w, h, bytes))) => Ok((w, h, std::sync::Arc::from(bytes))),
        Ok(Err(e)) => Err(e),
        Err(e) => Err(e.to_string()),
    };
    
    Message::PreviewCached(id, final_result)
}

async fn process_raw_load_request(id: i64, path: String) -> Message {
    let result = crate::raw::loader::load_raw_data(path)
        .await
        .map(std::sync::Arc::new);
    Message::RawPreloaded(id, result)
}
