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

pub fn handle_export_confirmed(editor: &mut RawEditor) -> Task<Message> {
    editor.active_modal = Modal::None;

    if editor.multi_selection.is_empty() {
        if let Some(id) = editor.selected_image_id {
            editor.export_queue = vec![id];
        }
    } else {
        editor.export_queue = editor.multi_selection.iter().cloned().collect();
    }

    if editor.export_queue.is_empty() {
        editor.status = "Nothing to export.".to_string();
        return Task::none();
    }

    editor.is_exporting = true;
    editor.status = format!("Starting export of {} images...", editor.export_queue.len());
    Task::perform(async {}, |_| Message::ProcessNextExport)
}

pub fn handle_process_next_export(editor: &mut RawEditor) -> Task<Message> {
    if let Some(image_id) = editor.export_queue.pop() {
        editor.status = format!("Exporting image {}...", image_id);
        if let Some(img) = editor.images.iter().find(|i| i.id == image_id) {
            let path = img.path.clone();
            return Task::perform(raw::loader::load_raw_data(path), move |res| {
                Message::ExportRawLoaded(image_id, res)
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
    result: Result<raw::loader::RawDataResult, String>,
) -> Task<Message> {
    match result {
        Ok(raw_data) => {
            // Always build fresh full-resolution resources for export.
            // The develop pipeline uses a subsampled preview; export must use the
            // original sensor dimensions so we don't re-use those resources here.

            let params = editor.current_edit_params;
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
                    crate::raw::dcp::interpolate_at_temperature(dcp, kelvin, params.profile_curve);
                (interpolated.forward_matrix, Some(interpolated))
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
                move |res| Message::ExportPipelineReady(image_id, res),
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
            let crop = editor.current_edit_params.crop;
            let crop_w = crop[2].clamp(0.001, 1.0);
            let crop_h = crop[3].clamp(0.001, 1.0);
            // Use original (full-sensor) dimensions in DISPLAY orientation —
            // portrait shots must export portrait (crop is display-space too).
            let (full_w, full_h) = resources.oriented_original_dims();
            let target_width = ((full_w as f32 * crop_w) as u32).max(1);
            let target_height = ((full_h as f32 * crop_h) as u32).max(1);

            let settings = editor.export_settings.clone();
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
                    let (bytes, _, _) = crate::gpu::render_functions::render_to_bytes(
                        &context,
                        &resources,
                        target_width,
                        target_height,
                    )
                    .await;
                    save_export_async(filename, bytes, target_width, target_height, settings).await
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
    _id: i64,
    result: Result<PathBuf, String>,
) -> Task<Message> {
    match result {
        Ok(path) => {
            tracing::info!("Export saved successfully: {}", path.display());
            editor.status = format!("Saved: {}", path.display());
        }
        Err(e) => {
            tracing::error!("Export save failed: {}", e);
            editor.status = format!("Save failed: {}", e);
        }
    }
    Task::perform(async {}, |_| Message::ProcessNextExport)
}

// Helpers

async fn save_export_async(
    filename: String,
    bytes: Vec<u8>,
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
        };

        let output_path = output_dir.join(format!("{}.{}", filename, extension));
        let file = File::create(&output_path).map_err(|e| e.to_string())?;
        let writer = BufWriter::new(file);

        match settings.format {
            ExportFormat::Png => {
                let mut encoder = png::Encoder::new(writer, width, height);
                encoder.set_color(png::ColorType::Rgba);
                encoder.set_depth(png::BitDepth::Eight);

                let mut png_writer = encoder.write_header().map_err(|e| e.to_string())?;
                png_writer
                    .write_image_data(&bytes)
                    .map_err(|e| e.to_string())?;
            }
            ExportFormat::Jpeg => {
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
        }

        Ok(output_path)
    })
    .await
    .map_err(|e| e.to_string())?
}
