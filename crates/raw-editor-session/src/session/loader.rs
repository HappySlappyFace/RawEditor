//! Background loads that run while their queue entry is at the head: the
//! working previews, RAW preloads and saved proxies (`Session::load_jobs`).

use super::{Session, SessionEvent};

/// Preview (JPEG) decodes allowed in flight at once. These are cheap and
/// bounded in memory, so a few running together keeps arrow-key navigation
/// ahead of the user instead of one decode behind.
pub const MAX_CONCURRENT_PREVIEW_LOADS: usize = 4;

/// RAW decodes allowed in flight at once. Deliberately lower than the preview
/// limit: each one can hold ~100 MB of sensor data, and they are already
/// bounded by `raw_preload_budget_mb` downstream.
pub const MAX_CONCURRENT_RAW_LOADS: usize = 2;

/// One background load the session wants running.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum LoadJob {
    /// The working preview (camera JPEG), for the RAM look-ahead cache.
    Preview(i64, String),
    /// A RAW preload.
    Raw(i64, String),
    /// A saved edited proxy.
    Proxy(i64, String),
}

impl LoadJob {
    /// What it reports when done.
    pub async fn run(self) -> SessionEvent {
        match self {
            LoadJob::Preview(id, path) => process_load_request(id, path).await,
            LoadJob::Raw(id, path) => process_raw_load_request(id, path).await,
            LoadJob::Proxy(id, path) => SessionEvent::EditedProxyLoaded(id, super::read_jpeg(path).await),
        }
    }
}

impl Session {
    /// The loads that should be running now: the head of each queue, up to
    /// its limit. The front end runs each as long as it is listed (and only
    /// once), and delivers its result; a decode under way must never be
    /// dropped or reordered (`requeue` keeps the heads in place).
    pub fn load_jobs(&self) -> impl Iterator<Item = LoadJob> + '_ {
        let previews = self
            .queued_loads
            .iter()
            .take(MAX_CONCURRENT_PREVIEW_LOADS)
            .map(|(id, path)| LoadJob::Preview(*id, path.clone()));
        let raws = self
            .queued_raw_loads
            .iter()
            .take(MAX_CONCURRENT_RAW_LOADS)
            .map(|(id, path)| LoadJob::Raw(*id, path.clone()));
        // Edited proxies are ~30 KB JPEGs: cheap enough to share the preview limit.
        let proxies = self
            .queued_proxy_loads
            .iter()
            .take(MAX_CONCURRENT_PREVIEW_LOADS)
            .map(|(id, path)| LoadJob::Proxy(*id, path.clone()));
        previews.chain(raws).chain(proxies)
    }
}

async fn process_load_request(id: i64, path: String) -> SessionEvent {
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
                    for chunk in pixels.as_chunks::<3>().0 {
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
    
    SessionEvent::PreviewCached(id, final_result)
}

async fn process_raw_load_request(id: i64, path: String) -> SessionEvent {
    let result = crate::raw::loader::load_raw_data(path)
        .await
        .map(std::sync::Arc::new);
    SessionEvent::RawPreloaded(id, result)
}
