use crate::app::message::Message;

/// Preview (JPEG) decodes allowed in flight at once. These are cheap and
/// bounded in memory, so a few running together keeps arrow-key navigation
/// ahead of the user instead of one decode behind.
pub const MAX_CONCURRENT_PREVIEW_LOADS: usize = 4;

/// RAW decodes allowed in flight at once. Deliberately lower than the preview
/// limit: each one can hold ~100 MB of sensor data, and they are already
/// bounded by `raw_preload_budget_mb` downstream.
pub const MAX_CONCURRENT_RAW_LOADS: usize = 2;

/// Drive the head of each load queue.
///
/// Takes the pre-truncated heads rather than the whole queues: `subscription()`
/// is rebuilt after every message, and cloning two full `Vec`s each time to read
/// only their first element was pure waste.
///
/// Each entry gets its own `run_with_id` key, so iced keeps them as distinct
/// subscriptions and runs them concurrently. Previously only `.first()` was
/// driven, which serialized every decode no matter how many cores were free or
/// how deep the preload queue was.
pub fn subscription(
    queued_loads: &[(i64, String)],
    queued_raw_loads: &[(i64, String)],
) -> iced::Subscription<Message> {
    let previews = queued_loads
        .iter()
        .take(MAX_CONCURRENT_PREVIEW_LOADS)
        .map(|(id, path)| {
            iced::Subscription::run_with_id(
                ("preview", *id),
                iced::futures::stream::once(process_load_request(*id, path.clone())),
            )
        });

    let raws = queued_raw_loads
        .iter()
        .take(MAX_CONCURRENT_RAW_LOADS)
        .map(|(id, path)| {
            iced::Subscription::run_with_id(
                ("raw", *id),
                iced::futures::stream::once(process_raw_load_request(*id, path.clone())),
            )
        });

    iced::Subscription::batch(previews.chain(raws))
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
