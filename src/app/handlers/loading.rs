use iced::Task;
use std::sync::Arc;
use crate::app::state::{RawEditor, EditorReadiness};
use crate::app::message::{AppTab, Message};
use crate::raw;
use crate::gpu;

pub fn handle_raw_data_loaded(
    editor: &mut RawEditor,
    image_id: i64,
    result: Result<raw::loader::RawDataResult, String>,
) -> Task<Message> {
    match result {
        Ok(raw) => {
            let raw = Arc::new(raw);
            editor.insert_raw_cache(image_id, raw.clone());
            apply_raw_preload_cleanup(editor, image_id);

            if editor.current_tab == AppTab::Develop && editor.selected_image_id == Some(image_id) {
                prepare_image_resources_from_raw(editor, image_id, raw)
            } else {
                Task::none()
            }
        }
        Err(e) => { 
            if editor.selected_image_id == Some(image_id) {
                editor.status = format!("Failed to load RAW: {}", e);
                editor.editor_readiness = EditorReadiness::Failed(image_id, e);
            }
            Task::none()
        }
    }
}

pub fn handle_raw_preloaded(
    editor: &mut RawEditor,
    image_id: i64,
    result: Result<Arc<raw::loader::RawDataResult>, String>,
) -> Task<Message> {
    apply_raw_preload_cleanup(editor, image_id);

    match result {
        Ok(raw) => {
            editor.insert_raw_cache(image_id, raw.clone());
            if editor.current_tab == AppTab::Develop
                && editor.selected_image_id == Some(image_id)
                && matches!(editor.editor_readiness, EditorReadiness::Loading(id) if id == image_id)
            {
                return prepare_image_resources_from_raw(editor, image_id, raw);
            }
            Task::none()
        }
        Err(e) => {
            tracing::warn!("RAW preload failed for image {}: {}", image_id, e);
            Task::none()
        }
    }
}

pub fn prepare_image_resources_from_raw(
    editor: &mut RawEditor,
    image_id: i64,
    raw: Arc<raw::loader::RawDataResult>,
) -> Task<Message> {
    editor.current_metadata = Some(metadata_snapshot(&raw));
    
    // Phase 140: Store current DCP profile
    editor.current_dcp_profile = raw.dcp_profile.clone();
    
    let params = editor.current_edit_params;
    let context = editor.gpu_context.clone();

    Task::perform(
        async move {
            let ctx = if let Some(c) = context {
                c
            } else {
                match gpu::shared::SharedContext::new().await {
                    Ok(c) => Arc::new(c),
                    Err(e) => return Err(e),
                }
            };

            let (forward_matrix, interpolated_dcp) = if let Some(dcp) = &raw.dcp_profile {
                let interpolated = crate::raw::dcp::interpolate_at_temperature(dcp, params.temperature_to_kelvin());
                (interpolated.forward_matrix, Some(interpolated))
            } else {
                let xyz_to_cam = raw.color_matrix;
                let cam_to_srgb = crate::color::calculate_cam_to_srgb(xyz_to_cam, raw.wb_multipliers);
                (cam_to_srgb, None)
            };

            match gpu::shared::ImageResources::new(
                &ctx,
                image_id,
                &raw.data,
                raw.width,
                raw.height,
                &params,
                raw.wb_multipliers,
                forward_matrix,
                raw.cfa_pattern,
                raw.black_levels,
                raw.white_level,
                interpolated_dcp.as_ref(),
                Some(1280), // develop preview: subsample for fast load
            ) {
                Ok(resources) => Ok((ctx, std::sync::Arc::new(resources))),
                Err(e) => Err(e),
            }
        },
        move |res| Message::ImageResourcesReady(image_id, res),
    )
}

pub fn handle_image_resources_ready(editor: &mut RawEditor, image_id: i64, result: Result<(Arc<gpu::shared::SharedContext>, Arc<gpu::shared::ImageResources>), String>) -> Task<Message> {
    editor.full_res_upgrading = false;

    // Ignore stale results from a previous image selection
    if editor.selected_image_id != Some(image_id) {
        return Task::none();
    }

    match result {
        Ok((context, resources)) => {
            if editor.gpu_context.is_none() {
                editor.gpu_context = Some(context.clone());
            }

            if let Some(ctx) = &editor.gpu_context {
                let interpolated = editor.current_dcp_profile.as_ref().map(|dcp| {
                    crate::raw::dcp::interpolate_at_temperature(dcp, editor.current_edit_params.temperature_to_kelvin())
                });
                resources.update_uniforms(ctx, &editor.current_edit_params, interpolated.as_ref());
            }

            editor.image_resources = Some(resources);
            editor.editor_readiness = EditorReadiness::Ready(image_id);
            editor.working_preview = None;
            editor.canvas_cache.clear();
            editor.histogram_cache.clear();

            editor.is_rendering = true;
            crate::app::handlers::develop::trigger_async_render(editor)
        }
        Err(e) => {
            editor.status = format!("GPU Init Failed: {}", e);
            editor.editor_readiness = EditorReadiness::Failed(image_id, e);
            Task::none()
        }
    }
}

/// Rebuild full-resolution ImageResources from cached RAW data.
/// Called when the user zooms in past the point where the subsampled
/// preview becomes limiting. Uses the raw_cache (no disk I/O needed).
pub fn trigger_full_res_upgrade(editor: &mut RawEditor) -> Task<Message> {
    let Some(image_id) = editor.selected_image_id else { return Task::none(); };

    // Only upgrade if currently subsampled
    if let Some(res) = &editor.image_resources {
        if res.width == res.original_width { return Task::none(); } // already full-res
    } else {
        return Task::none();
    }

    if editor.full_res_upgrading { return Task::none(); }

    let Some(raw) = editor.raw_cache.get(&image_id).cloned() else {
        return Task::none(); // not in cache, skip (avoid extra disk I/O while editing)
    };

    editor.full_res_upgrading = true;
    let params = editor.current_edit_params;
    let context = editor.gpu_context.clone();

    tracing::info!("Upgrading to full-res resources for image {} (zoom-in)", image_id);

    Task::perform(
        async move {
            let ctx = if let Some(c) = context {
                c
            } else {
                match gpu::shared::SharedContext::new().await {
                    Ok(c) => Arc::new(c),
                    Err(e) => return Err(e),
                }
            };

            let (forward_matrix, interpolated_dcp) = if let Some(dcp) = &raw.dcp_profile {
                let interpolated = crate::raw::dcp::interpolate_at_temperature(dcp, params.temperature_to_kelvin());
                (interpolated.forward_matrix, Some(interpolated))
            } else {
                let xyz_to_cam = raw.color_matrix;
                let cam_to_srgb = crate::color::calculate_cam_to_srgb(xyz_to_cam, raw.wb_multipliers);
                (cam_to_srgb, None)
            };

            match gpu::shared::ImageResources::new(
                &ctx,
                image_id,
                &raw.data,
                raw.width,
                raw.height,
                &params,
                raw.wb_multipliers,
                forward_matrix,
                raw.cfa_pattern,
                raw.black_levels,
                raw.white_level,
                interpolated_dcp.as_ref(),
                None, // full resolution
            ) {
                Ok(resources) => Ok((ctx, Arc::new(resources))),
                Err(e) => Err(e),
            }
        },
        move |res| Message::ImageResourcesReady(image_id, res),
    )
}

fn apply_raw_preload_cleanup(editor: &mut RawEditor, image_id: i64) {
    editor.pending_raw_loads.remove(&image_id);
    editor.queued_raw_loads.retain(|(id, _)| *id != image_id);
}

fn metadata_snapshot(raw: &raw::loader::RawDataResult) -> raw::loader::RawDataResult {
    raw::loader::RawDataResult {
        data: Vec::new(),
        dcp_profile: raw.dcp_profile.clone(),
        width: raw.width,
        height: raw.height,
        wb_multipliers: raw.wb_multipliers,
        color_matrix: raw.color_matrix,
        cfa_pattern: raw.cfa_pattern,
        black_levels: raw.black_levels,
        white_level: raw.white_level,
        crops: raw.crops,
        cfa_name: raw.cfa_name.clone(),
        measured_black_levels: raw.measured_black_levels,
        make: raw.make.clone(),
        model: raw.model.clone(),
        iso: raw.iso.clone(),
        shutter_speed: raw.shutter_speed.clone(),
        aperture: raw.aperture.clone(),
        lens: raw.lens.clone(),
    }
}
