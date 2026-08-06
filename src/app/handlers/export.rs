use crate::app::message::Message;
use crate::app::state::{ExportFormat, ExportSettings, Modal, RawEditor};
use crate::gpu;
use crate::raw;
use iced::Task;
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub fn handle_set_export_format(editor: &mut RawEditor, format: ExportFormat) -> Task<Message> {
    editor.export_settings.format = format;
    Task::none()
}

pub fn handle_set_export_quality(editor: &mut RawEditor, quality: u8) -> Task<Message> {
    editor.export_settings.quality = quality;
    Task::none()
}

pub fn handle_toggle_export_resize(editor: &mut RawEditor, resize: bool) -> Task<Message> {
    editor.export_settings.resize = resize;
    Task::none()
}

pub fn handle_set_export_width(editor: &mut RawEditor, width: u32) -> Task<Message> {
    editor.export_settings.max_width = width;
    Task::none()
}

pub fn handle_set_export_subfolder(editor: &mut RawEditor, subfolder: String) -> Task<Message> {
    editor.export_settings.subfolder = subfolder;
    Task::none()
}

pub fn handle_pick_export_base_path(editor: &mut RawEditor) -> Task<Message> {
    let current = editor.export_settings.base_path.clone();
    Task::perform(
        async move {
            rfd::AsyncFileDialog::new()
                .set_directory(current)
                .pick_folder()
                .await
                .map(|handle| handle.path().to_path_buf())
        },
        |opt| {
            if let Some(path) = opt {
                Message::SetExportBasePath(path)
            } else {
                Message::ModalNoOp
            }
        },
    )
}

pub fn handle_set_export_base_path(editor: &mut RawEditor, path: PathBuf) -> Task<Message> {
    if !path.as_os_str().is_empty() {
        editor.export_settings.base_path = path;
    }
    Task::none()
}

pub fn handle_open_export_modal(editor: &mut RawEditor) -> Task<Message> {
    editor.active_modal = Modal::Export;

    let default_dir = dirs::picture_dir().unwrap_or_else(|| PathBuf::from("."));
    if editor.export_settings.base_path == default_dir {
        if let Some(id) = editor.selected_image_id {
            if let Some(img) = editor.images.iter().find(|i| i.id == id) {
                if let Some(parent) = Path::new(&img.path).parent() {
                    editor.export_settings.base_path = parent.to_path_buf();
                }
            }
        }
    }

    Task::none()
}

/// Images the current selection would export, in display order.
///
/// `multi_selection` is a `HashSet`, so iterating it directly gave an
/// arbitrary order — and the queue is drained with `pop()`, so batches ran
/// back-to-front in a different order every time. Ordering through
/// `displayed_ids` makes a run reproducible and the progress list readable.
pub fn export_target_ids(editor: &RawEditor) -> Vec<i64> {
    if editor.multi_selection.is_empty() {
        return editor.selected_image_id.into_iter().collect();
    }
    let mut ids: Vec<i64> = editor
        .displayed_ids()
        .into_iter()
        .filter(|id| editor.multi_selection.contains(id))
        .collect();
    // Anything selected but not currently displayed still belongs in the
    // batch; append it rather than silently dropping it.
    let mut extra: Vec<i64> = editor
        .multi_selection
        .iter()
        .copied()
        .filter(|id| !ids.contains(id))
        .collect();
    extra.sort_unstable();
    ids.append(&mut extra);
    ids
}

pub fn handle_export_confirmed(editor: &mut RawEditor) -> Task<Message> {
    editor.active_modal = Modal::None;

    // Re-entrancy guard, matching handle_delete_from_disk_confirmed's
    // `is_deleting` check. Without it, confirming again mid-run overwrote
    // export_queue and started a second interleaved ProcessNextExport chain.
    if editor.is_exporting {
        editor.status = "An export is already running.".to_string();
        return Task::none();
    }

    let ids = export_target_ids(editor);
    if ids.is_empty() {
        editor.status = "Nothing to export.".to_string();
        return Task::none();
    }

    editor.export_jobs = ids
        .iter()
        .map(|id| crate::app::state::ExportJob {
            id: *id,
            filename: editor
                .images
                .iter()
                .find(|i| i.id == *id)
                .map(|i| i.filename.clone())
                .unwrap_or_else(|| format!("image {id}")),
            state: crate::app::state::ExportJobState::Pending,
        })
        .collect();
    // Reversed: the queue is drained with `pop()`, so this yields display order.
    editor.export_queue = ids.into_iter().rev().collect();

    editor.is_exporting = true;
    editor.status = format!("Starting export of {} images...", editor.export_jobs.len());
    Task::perform(async {}, |_| Message::ProcessNextExport)
}

/// Cancel after the in-flight image finishes. The running GPU render is not
/// interruptible, so this drains the pending work rather than killing a task
/// mid-readback.
pub fn handle_cancel_export(editor: &mut RawEditor) -> Task<Message> {
    if !editor.is_exporting {
        return Task::none();
    }
    let dropped = editor.export_queue.len();
    editor.export_queue.clear();
    for job in editor.export_jobs.iter_mut() {
        if job.state == crate::app::state::ExportJobState::Pending {
            job.state = crate::app::state::ExportJobState::Failed("cancelled".to_string());
        }
    }
    editor.status = format!("Export cancelled — {dropped} images skipped");
    Task::none()
}

/// Update one job's progress. Silently ignores unknown ids so a stale message
/// from a cancelled batch can't panic.
fn set_job_state(editor: &mut RawEditor, id: i64, state: crate::app::state::ExportJobState) {
    if let Some(job) = editor.export_jobs.iter_mut().find(|j| j.id == id) {
        job.state = state;
    }
}

pub fn handle_process_next_export(editor: &mut RawEditor) -> Task<Message> {
    if let Some(image_id) = editor.export_queue.pop() {
        let done = editor
            .export_jobs
            .iter()
            .filter(|j| !matches!(j.state, crate::app::state::ExportJobState::Pending))
            .count();
        editor.status = format!("Exporting {} of {}...", done + 1, editor.export_jobs.len());
        set_job_state(editor, image_id, crate::app::state::ExportJobState::Working);
        if let Some(img) = editor.images.iter().find(|i| i.id == image_id) {
            let path = img.path.clone();
            // Each image's own saved edits. For the image currently open in
            // Develop, prefer the in-memory params: they may hold a gesture
            // that has not been flushed to SQLite yet.
            let params = if Some(image_id) == editor.selected_image_id {
                editor.current_edit_params
            } else {
                editor
                    .library
                    .as_ref()
                    .and_then(|lib| lib.load_edit_params(image_id).ok())
                    .unwrap_or_default()
            };
            // Reuse an already-decoded RAW when the develop/cull preload has
            // one. Skips a multi-second decode per image for the common
            // "cull, then export the selects" flow, and costs nothing: the
            // cache is already byte-budgeted and the entry is shared, not
            // copied.
            if let Some(cached) = editor.raw_cache.get(&image_id).cloned() {
                return Task::done(Message::ExportRawLoaded(image_id, params, Ok(cached)));
            }
            return Task::perform(raw::loader::load_raw_data(path), move |res| {
                Message::ExportRawLoaded(image_id, params, res.map(std::sync::Arc::new))
            });
        }
        Task::perform(async {}, |_| Message::ProcessNextExport)
    } else {
        editor.is_exporting = false;
        editor.status = "Export Complete!".to_string();
        Task::none()
    }
}

pub fn handle_export_raw_loaded(
    editor: &mut RawEditor,
    image_id: i64,
    params: crate::core::types::EditParams,
    result: Result<Arc<raw::loader::RawDataResult>, String>,
) -> Task<Message> {
    match result {
        Ok(raw_data) => {
            // Always build fresh full-resolution resources for export.
            // The develop pipeline uses a subsampled preview; export must use the
            // original sensor dimensions so we don't re-use those resources here.
            let ctx = editor.gpu_context.clone();

            // Must mirror the develop path (loading.rs): with a DCP the shader takes the
            // has_dcp branch and expects the DCP ForwardMatrix (camera → XYZ D50) — passing
            // the fallback camera → sRGB matrix there wrecks every colour (pink skin tones).
            // Anchored WB: honour saved Temp/Tint edits exactly like develop.
            let as_shot = crate::color::as_shot_kelvin_tint(&raw_data);
            let (kelvin, _tint, wb_override) = crate::color::solve_wb(
                &params,
                as_shot,
                raw_data.dcp_profile.as_deref(),
                raw_data.color_matrix,
            );
            let wb_final = wb_override.unwrap_or(raw_data.wb_multipliers);

            let (forward_matrix, interpolated_dcp) = if let Some(dcp) = &raw_data.dcp_profile {
                let interpolated =
                    crate::raw::dcp::interpolate_at_temperature(
                        dcp,
                        kelvin,
                        crate::core::tonemap::ToneSettings::from_params(&params),
                    );
                (interpolated.camera_to_prophoto, Some(interpolated))
            } else {
                let xyz_to_cam = raw_data.color_matrix;
                let cam_to_srgb = crate::color::calculate_cam_to_srgb(
                    xyz_to_cam,
                    raw_data.wb_multipliers,
                    raw_data.color_matrix_is_d65,
                );
                (cam_to_srgb, None)
            };

            Task::perform(
                async move {
                    let context = if let Some(c) = ctx {
                        c
                    } else {
                        match gpu::shared::SharedContext::new().await {
                            Ok(c) => Arc::new(c),
                            Err(e) => return Err(e),
                        }
                    };

                    gpu::shared::ImageResources::new(
                        &context,
                        image_id,
                        &raw_data.data,
                        raw_data.width,
                        raw_data.height,
                        &params,
                        wb_final,
                        forward_matrix,
                        raw_data.cfa_pattern,
                        raw_data.black_levels,
                        raw_data.white_level,
                        interpolated_dcp.as_ref(),
                        None, // full resolution for export
                        raw_data.orientation,
                    )
                    .map(|res| (context, Arc::new(res)))
                },
                move |res| Message::ExportPipelineReady(image_id, params, res),
            )
        }
        Err(e) => {
            editor.status = format!("Failed to load RAW: {}", e);
            Task::perform(async {}, |_| Message::ProcessNextExport)
        }
    }
}

pub fn handle_export_pipeline_ready(
    editor: &mut RawEditor,
    image_id: i64,
    params: crate::core::types::EditParams,
    result: Result<
        (
            Arc<gpu::shared::SharedContext>,
            Arc<gpu::shared::ImageResources>,
        ),
        String,
    >,
) -> Task<Message> {
    match result {
        Ok((context, resources)) => {
            // This image's own crop, not the one open in Develop — otherwise
            // the output is sized from the wrong image's crop rectangle.
            let crop = params.crop;
            let crop_w = crop[2].clamp(0.001, 1.0);
            let crop_h = crop[3].clamp(0.001, 1.0);
            // Use original (full-sensor) dimensions in DISPLAY orientation —
            // portrait shots must export portrait (crop is display-space too).
            let (full_w, full_h) = resources.oriented_original_dims();
            let cropped_w = ((full_w as f32 * crop_w) as u32).max(1);
            let cropped_h = ((full_h as f32 * crop_h) as u32).max(1);

            let settings = editor.export_settings.clone();
            // Honour "Resize Long Edge". This was previously computed but the
            // setting was never read, so the checkbox and the size field did
            // nothing at all.
            let (target_width, target_height) = resized_dims(cropped_w, cropped_h, &settings);
            let filename = if let Some(img) = editor.images.iter().find(|i| i.id == image_id) {
                Path::new(&img.path)
                    .file_stem()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_string()
            } else {
                format!("image_{}", image_id)
            };

            Task::perform(
                async move {
                    let pixels = if settings.format == ExportFormat::Tiff {
                        let (samples, _, _) = crate::gpu::render_functions::render_to_bytes_16bit(
                            &context,
                            &resources,
                            target_width,
                            target_height,
                        )
                        .await;
                        ExportPixels::Rgb16(samples)
                    } else {
                        let (bytes, _, _) = crate::gpu::render_functions::render_to_bytes(
                            &context,
                            &resources,
                            target_width,
                            target_height,
                            // Export always renders the whole cropped frame,
                            // never the on-screen window.
                            crate::gpu::params::FULL_VIEW_RECT,
                        )
                        .await;
                        ExportPixels::Rgba8(bytes)
                    };
                    save_export_async(filename, pixels, target_width, target_height, settings).await
                },
                move |res| Message::ExportSaveComplete(image_id, res),
            )
        }
        Err(e) => {
            editor.status = format!("Export failed: {}", e);
            Task::perform(async {}, |_| Message::ProcessNextExport)
        }
    }
}

pub fn handle_export_save_complete(
    editor: &mut RawEditor,
    id: i64,
    result: Result<PathBuf, String>,
) -> Task<Message> {
    match result {
        Ok(path) => {
            tracing::info!("Export saved successfully: {}", path.display());
            editor.status = format!("Saved: {}", path.display());
            set_job_state(editor, id, crate::app::state::ExportJobState::Done(path));
        }
        Err(e) => {
            tracing::error!("Export save failed: {}", e);
            editor.status = format!("Save failed: {}", e);
            set_job_state(editor, id, crate::app::state::ExportJobState::Failed(e));
        }
    }
    Task::perform(async {}, |_| Message::ProcessNextExport)
}

// Helpers

/// Apply the "Resize Long Edge" setting, preserving aspect ratio.
///
/// The setting is a cap on the LONGER edge, so portrait and landscape frames
/// come out the same size on their long side — which is what "long edge"
/// means and what the checkbox has always said. (The width field said "Max
/// Width"; the two disagreed, and neither was implemented.)
///
/// Only ever shrinks: asking for a 4000px long edge on a 3000px crop returns
/// the crop untouched rather than upscaling it into softness.
fn resized_dims(width: u32, height: u32, settings: &ExportSettings) -> (u32, u32) {
    if !settings.resize || settings.max_width == 0 {
        return (width, height);
    }
    let long_edge = width.max(height);
    if long_edge <= settings.max_width {
        return (width, height);
    }
    let scale = settings.max_width as f64 / long_edge as f64;
    (
        ((width as f64 * scale).round() as u32).max(1),
        ((height as f64 * scale).round() as u32).max(1),
    )
}

/// Pick a destination path, adding ` (2)`, ` (3)`… rather than overwriting.
///
/// Two source folders commonly hold the same stems (DSC_0001 from two cards,
/// or a re-export into the same folder). `File::create` truncates, so the
/// previous behaviour silently destroyed the earlier export.
fn unique_output_path(dir: &Path, stem: &str, extension: &str) -> PathBuf {
    let candidate = dir.join(format!("{stem}.{extension}"));
    if !candidate.exists() {
        return candidate;
    }
    for n in 2..10_000u32 {
        let candidate = dir.join(format!("{stem} ({n}).{extension}"));
        if !candidate.exists() {
            return candidate;
        }
    }
    // Pathological: fall back to overwriting rather than looping forever.
    dir.join(format!("{stem}.{extension}"))
}

/// Carries either an 8-bit RGBA (JPEG/PNG) or 16-bit RGB (TIFF) sample
/// buffer through the async save step, so `save_export_async` can't be
/// called with a format/pixel-type mismatch.
enum ExportPixels {
    /// Shared rather than owned because `render_to_bytes` returns the buffer
    /// already wrapped; the encoders only ever read it as a slice.
    Rgba8(std::sync::Arc<[u8]>),
    Rgb16(Vec<u16>),
}

async fn save_export_async(
    filename: String,
    pixels: ExportPixels,
    width: u32,
    height: u32,
    settings: ExportSettings,
) -> Result<PathBuf, String> {
    tokio::task::spawn_blocking(move || {
        use std::fs::File;
        use std::io::BufWriter;

        let output_dir = settings.base_path.join(&settings.subfolder);
        std::fs::create_dir_all(&output_dir).map_err(|e| e.to_string())?;

        let extension = match settings.format {
            ExportFormat::Jpeg => "jpg",
            ExportFormat::Png => "png",
            ExportFormat::Tiff => "tiff",
        };

        let output_path = unique_output_path(&output_dir, &filename, extension);
        let file = File::create(&output_path).map_err(|e| e.to_string())?;
        let writer = BufWriter::new(file);

        match (settings.format, pixels) {
            (ExportFormat::Png, ExportPixels::Rgba8(bytes)) => {
                let mut encoder = png::Encoder::new(writer, width, height);
                encoder.set_color(png::ColorType::Rgba);
                encoder.set_depth(png::BitDepth::Eight);

                let mut png_writer = encoder.write_header().map_err(|e| e.to_string())?;
                png_writer
                    .write_image_data(&bytes)
                    .map_err(|e| e.to_string())?;
            }
            (ExportFormat::Jpeg, ExportPixels::Rgba8(bytes)) => {
                if !bytes.len().is_multiple_of(4) {
                    return Err(format!(
                        "Export buffer length {} is not a multiple of 4",
                        bytes.len()
                    ));
                }
                let rgb_bytes: Vec<u8> = bytes
                    .chunks_exact(4)
                    .flat_map(|rgba| [rgba[0], rgba[1], rgba[2]])
                    .collect();

                let encoder = jpeg_encoder::Encoder::new(writer, settings.quality);
                encoder
                    .encode(
                        &rgb_bytes,
                        width as u16,
                        height as u16,
                        jpeg_encoder::ColorType::Rgb,
                    )
                    .map_err(|e| e.to_string())?;
            }
            (ExportFormat::Tiff, ExportPixels::Rgb16(samples)) => {
                tiff::encoder::TiffEncoder::new(writer)
                    .map_err(|e| e.to_string())?
                    .write_image::<tiff::encoder::colortype::RGB16>(width, height, &samples)
                    .map_err(|e| e.to_string())?;
            }
            (format, _) => {
                return Err(format!("Export pixel buffer does not match format {format}"));
            }
        }

        Ok(output_path)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::state::ExportSettings;

    fn settings(resize: bool, max_width: u32) -> ExportSettings {
        ExportSettings { resize, max_width, ..ExportSettings::default() }
    }

    #[test]
    fn resize_off_leaves_dimensions_alone() {
        assert_eq!(resized_dims(6000, 4000, &settings(false, 1000)), (6000, 4000));
    }

    /// The setting caps the LONG edge, so a portrait and a landscape frame come
    /// out matched on their long side. Capping width instead would make
    /// portraits much larger, which is what the mismatched label implied.
    #[test]
    fn resize_caps_the_long_edge_in_either_orientation() {
        assert_eq!(resized_dims(6000, 4000, &settings(true, 3000)), (3000, 2000));
        assert_eq!(resized_dims(4000, 6000, &settings(true, 3000)), (2000, 3000));
    }

    #[test]
    fn resize_preserves_aspect_ratio() {
        let (w, h) = resized_dims(6000, 4000, &settings(true, 1999));
        let before = 6000.0 / 4000.0;
        let after = w as f64 / h as f64;
        assert!((before - after).abs() < 0.01, "{w}x{h}");
    }

    /// Never upscale: asking for a long edge bigger than the source would only
    /// add softness and file size.
    #[test]
    fn resize_never_enlarges() {
        assert_eq!(resized_dims(1200, 800, &settings(true, 4000)), (1200, 800));
        assert_eq!(resized_dims(1200, 800, &settings(true, 1200)), (1200, 800));
    }

    #[test]
    fn resize_ignores_a_zero_limit_rather_than_collapsing() {
        assert_eq!(resized_dims(6000, 4000, &settings(true, 0)), (6000, 4000));
    }

    #[test]
    fn resize_never_returns_a_zero_dimension() {
        let (w, h) = resized_dims(6000, 10, &settings(true, 1));
        assert!(w >= 1 && h >= 1, "{w}x{h}");
    }

    /// `File::create` truncates, so without uniquifying, two source folders
    /// holding the same stem meant the second export destroyed the first.
    #[test]
    fn output_path_avoids_clobbering_an_existing_file() {
        let dir = std::env::temp_dir().join(format!("raw-editor-export-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");

        let first = unique_output_path(&dir, "DSC_0001", "jpg");
        assert_eq!(first.file_name().unwrap(), "DSC_0001.jpg");
        std::fs::write(&first, b"x").expect("write");

        let second = unique_output_path(&dir, "DSC_0001", "jpg");
        assert_eq!(second.file_name().unwrap(), "DSC_0001 (2).jpg");
        std::fs::write(&second, b"x").expect("write");

        let third = unique_output_path(&dir, "DSC_0001", "jpg");
        assert_eq!(third.file_name().unwrap(), "DSC_0001 (3).jpg");

        // A different extension is a different file and must not be renamed.
        let png = unique_output_path(&dir, "DSC_0001", "png");
        assert_eq!(png.file_name().unwrap(), "DSC_0001.png");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
