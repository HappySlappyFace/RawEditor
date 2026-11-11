use iced::Task;
use std::sync::Arc;
use crate::app::state::{RawEditor, EditorReadiness};
use crate::app::message::Message;
use crate::raw;
use crate::gpu;

pub fn handle_raw_data_loaded(
    editor: &mut RawEditor,
    image_id: i64,
    result: Result<raw::loader::RawDataResult, String>,
) -> Task<Message> {
    editor.direct_raw_loads.remove(&image_id);
    match result {
        Ok(raw) => {
            let raw = Arc::new(raw);
            editor.insert_raw_cache(image_id, raw.clone());
            apply_raw_preload_cleanup(editor, image_id);

            if awaiting_raw(editor, image_id) {
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
            if awaiting_raw(editor, image_id) {
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

/// True when Develop is waiting on exactly this RAW to finish loading.
///
/// Deliberately NOT gated on the current tab. `Loading(id)` is a promise that
/// a load is in flight; if the user stepped out to Library while it decoded
/// and the result were dropped, coming back would find "still loading", start
/// nothing, and wait forever.
fn awaiting_raw(editor: &RawEditor, image_id: i64) -> bool {
    editor.selected_image_id == Some(image_id)
        && matches!(editor.editor_readiness, EditorReadiness::Loading(id) if id == image_id)
}

pub fn prepare_image_resources_from_raw(
    editor: &mut RawEditor,
    image_id: i64,
    raw: Arc<raw::loader::RawDataResult>,
) -> Task<Message> {
    editor.current_metadata = Some(metadata_snapshot(&raw));
    editor.current_metadata_id = Some(image_id);

    // Phase 140: Store current DCP profile
    editor.current_dcp_profile = raw.dcp_profile.clone();
    // The memo is keyed only on (kelvin, profile_curve), so a new profile at
    // the same slider values would otherwise return the previous image's LUT.
    editor.dcp_memo.replace(None);

    // As-shot WB anchor: solve the camera multipliers to (Kelvin, tint) so the
    // Temp/Tint sliders can display and pivot around the real camera values.
    let (as_kelvin, as_tint) = crate::color::as_shot_kelvin_tint(&raw);
    editor.as_shot_wb = Some((as_kelvin, as_tint));
    tracing::info!("As-shot WB: {:.0}K, tint {:+.1}", as_kelvin, as_tint);

    let params = editor.current_edit_params;
    let profile_defaults = editor.camera_profile_defaults.clone();
    let context = editor.gpu_context.clone();
    let gpu_init = editor.gpu_init.clone();

    Task::perform(
        async move {
            let ctx = shared_context(&gpu_init, context).await?;
            // CPU-heavy (profile scan, mosaic blend, noise/haze analysis,
            // texture uploads): on the blocking pool, not a tokio worker, so
            // renders' `block_in_place` always finds a free worker.
            tokio::task::spawn_blocking(move || {

            // The image's camera profile: its own choice, else its camera's
            // default, else the best installed match. Resolved here, off the
            // UI thread (it may scan and parse profile files).
            let profile_use = crate::core::camera_profiles::ProfileUse::resolve(
                &raw,
                params.camera_profile,
                &profile_defaults,
            );

            // Anchored kelvin for the DCP interpolation. ImageResources::new
            // receives the AS-SHOT multipliers (the "None → as-shot" baseline
            // for update_uniforms); handle_image_resources_ready immediately
            // applies any saved Temp/Tint edits via the WB override.
            let as_shot = profile_use.as_shot;
            let kelvin = params.kelvin_from_anchor(as_shot.0);

            let (forward_matrix, interpolated_dcp) = if let Some(dcp) = &profile_use.profile {
                let interpolated = crate::raw::dcp::interpolate_at_temperature(dcp, kelvin, crate::core::tonemap::ToneSettings::from_params(&params));
                (interpolated.camera_to_prophoto, Some(interpolated))
            } else {
                let xyz_to_cam = raw.color_matrix;
                let cam_to_srgb = crate::color::calculate_cam_to_srgb(xyz_to_cam, raw.wb_multipliers, raw.color_matrix_is_d65);
                (cam_to_srgb, None)
            };

            // AI denoise rides in on the mosaic itself (`ml::denoise`).
            let (mosaic, ai_applied) = crate::ml::denoise::effective_mosaic(&raw, image_id, params.ai_denoise);
            match gpu::shared::ImageResources::new(
                &ctx,
                image_id,
                &mosaic,
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
                raw.orientation,
                raw.lens_context.clone(),
            ) {
                Ok(mut resources) => {
                    resources.profile_use = Some(profile_use);
                    resources.ai_denoise = (params.ai_denoise, ai_applied);
                    Ok((ctx, std::sync::Arc::new(resources)))
                }
                Err(e) => Err(e),
            }
            })
            .await
            .map_err(|e| format!("resource build panicked: {e}"))?
        },
        move |res| Message::ImageResourcesReady(image_id, res),
    )
}

/// The one `SharedContext`. Every path that needs a context goes through
/// here: tasks started before the first finishes (select 1 → 2 → 1 during the
/// first load, or an export beside a load) all get the same device, so
/// resources built by one can be installed beside another's context.
pub async fn shared_context(
    cell: &tokio::sync::OnceCell<Arc<gpu::shared::SharedContext>>,
    existing: Option<Arc<gpu::shared::SharedContext>>,
) -> Result<Arc<gpu::shared::SharedContext>, String> {
    cell.get_or_try_init(|| async {
        match existing {
            Some(c) => Ok(c),
            None => gpu::shared::SharedContext::new().await.map(Arc::new),
        }
    })
    .await
    .cloned()
}

pub fn handle_image_resources_ready(editor: &mut RawEditor, image_id: i64, result: Result<(Arc<gpu::shared::SharedContext>, Arc<gpu::shared::ImageResources>), String>) -> Task<Message> {
    // Ignore stale results from a previous image selection
    if editor.selected_image_id != Some(image_id) {
        return Task::none();
    }

    match result {
        Ok((context, resources)) => {
            if editor.gpu_context.is_none() {
                editor.gpu_context = Some(context.clone());
            }
            // The profile these resources were built with is the image's
            // profile from here on — the WB solve below already reads it.
            if let Some(used) = &resources.profile_use {
                if editor.current_dcp_profile.as_ref().map(Arc::as_ptr)
                    != used.profile.as_ref().map(Arc::as_ptr)
                {
                    editor.dcp_memo.replace(None);
                }
                editor.current_dcp_profile = used.profile.clone();
                editor.as_shot_wb = Some(used.as_shot);
                if let Some(meta) = editor.current_metadata.as_mut() {
                    meta.dcp_profile = used.profile.clone();
                }
            }

            editor.sync_tool_flags();
            if let Some(ctx) = &editor.gpu_context {
                let params = editor.view_params();
                let (interpolated, wb_override) =
                    crate::app::handlers::develop::resolve_wb_and_dcp(editor, &params);
                resources.update_uniforms(ctx, &params, interpolated.as_deref(), wb_override);
            }

            // A full-resolution upgrade keeps the preview it replaces: renders
            // that full resolution would not sharpen use it instead
            // (`RawEditor::preview_resources`). Anything else drops it.
            // AI masks: the new resources start with no bitmaps.
            if editor.ai_bitmaps.image_id == Some(image_id) {
                resources.set_ai_bitmaps(editor.ai_bitmaps.bitmaps.clone());
            }
            let upgrade = resources.width == resources.original_width;
            editor.preview_resources = editor.image_resources.take().filter(|old| {
                upgrade && old.image_id == image_id && old.width < old.original_width
            });
            editor.image_resources = Some(resources);
            editor.editor_readiness = EditorReadiness::Ready(image_id);
            editor.working_preview = None;
            editor.canvas_cache.clear();
            editor.histogram_cache.clear();

            // Throttled, not forced: a render can still be in flight here
            // (the previous image, or the subsampled pass before a full-res
            // upgrade). Starting a second one alongside it raced the two, and
            // whichever landed last won the screen.
            let render = crate::app::handlers::navigation::throttled_render(editor);
            Task::batch([render, crate::app::handlers::ai_denoise::after_resources(editor)])
        }
        Err(e) => {
            editor.status = format!("GPU Init Failed: {}", e);
            editor.editor_readiness = EditorReadiness::Failed(image_id, e);
            Task::none()
        }
    }
}

/// The full-resolution rebuild finished: release the one-at-a-time guard,
/// then install it — or, if the user has moved on, see whether the image
/// they are on now wants one (its own render may have asked while this was
/// building and been turned away by the guard).
pub fn handle_full_res_ready(
    editor: &mut RawEditor,
    image_id: i64,
    result: Result<(Arc<gpu::shared::SharedContext>, Arc<gpu::shared::ImageResources>), String>,
) -> Task<Message> {
    editor.full_res_upgrading = false;
    if editor.selected_image_id != Some(image_id) {
        return upgrade_if_soft(editor);
    }
    match result {
        Ok(_) => handle_image_resources_ready(editor, image_id, result),
        Err(e) => {
            // The preview resources still work: keep editing on them.
            tracing::warn!("Full-resolution rebuild of image {image_id} failed, keeping the preview: {e}");
            editor.full_res_failed = Some(image_id);
            Task::none()
        }
    }
}

/// Start the full-resolution rebuild if the current view shows the
/// subsampled develop texture magnified (`develop::render_target`).
fn upgrade_if_soft(editor: &mut RawEditor) -> Task<Message> {
    if editor.current_tab != crate::app::message::AppTab::Develop
        || !matches!(editor.editor_readiness, EditorReadiness::Ready(_))
        || crate::app::handlers::develop::render_target(editor).magnification
            <= crate::core::viewport::FULL_RES_MAGNIFICATION
    {
        return Task::none();
    }
    trigger_full_res_upgrade(editor)
}

/// Rebuild the current image's ImageResources at full resolution, replacing
/// the subsampled develop preview. Requested by any render whose view shows
/// the preview magnified (`develop::trigger_async_render`). The RAW comes from
/// `raw_cache`, or from disk if the cache no longer holds it — skipping the
/// upgrade there left a zoomed-in view soft with no way to sharpen it.
pub fn trigger_full_res_upgrade(editor: &mut RawEditor) -> Task<Message> {
    let Some(image_id) = editor.selected_image_id else { return Task::none(); };

    // Only upgrade if currently subsampled
    if let Some(res) = &editor.image_resources {
        if res.width == res.original_width { return Task::none(); } // already full-res
        if res.image_id != image_id { return Task::none(); } // still loading this one
    } else {
        return Task::none();
    }

    if editor.full_res_upgrading || editor.full_res_failed == Some(image_id) {
        return Task::none();
    }

    let cached = editor.raw_cache.get(&image_id).cloned();
    let path = match &cached {
        Some(_) => None,
        None => match editor.images.iter().find(|i| i.id == image_id) {
            Some(img) => Some(img.path.clone()),
            None => return Task::none(),
        },
    };

    editor.full_res_upgrading = true;
    let params = editor.current_edit_params;
    let profile_use = editor.image_resources.as_ref().and_then(|r| r.profile_use.clone());
    let profile_defaults = editor.camera_profile_defaults.clone();
    let context = editor.gpu_context.clone();
    let gpu_init = editor.gpu_init.clone();

    tracing::info!(
        "Upgrading to full-res resources for image {} ({})",
        image_id,
        if cached.is_some() { "cached RAW" } else { "re-reading RAW" }
    );

    Task::perform(
        async move {
            let raw = match (cached, path) {
                (Some(raw), _) => raw,
                (None, Some(path)) => Arc::new(raw::loader::load_raw_data(path).await?),
                (None, None) => return Err("no RAW to upgrade from".to_string()),
            };
            let ctx = shared_context(&gpu_init, context).await?;
            // CPU-heavy (profile scan, mosaic blend, noise/haze analysis,
            // texture uploads): on the blocking pool, not a tokio worker, so
            // renders' `block_in_place` always finds a free worker.
            tokio::task::spawn_blocking(move || {

            // The profile the image already uses (resolved for its preview),
            // or — if the preview recorded none — resolved now.
            let profile_use = profile_use.unwrap_or_else(|| {
                crate::core::camera_profiles::ProfileUse::resolve(&raw, params.camera_profile, &profile_defaults)
            });

            // Anchored kelvin; as-shot multipliers to ::new (same reasoning as
            // the preview path above).
            let as_shot = profile_use.as_shot;
            let kelvin = params.kelvin_from_anchor(as_shot.0);

            let (forward_matrix, interpolated_dcp) = if let Some(dcp) = &profile_use.profile {
                let interpolated = crate::raw::dcp::interpolate_at_temperature(dcp, kelvin, crate::core::tonemap::ToneSettings::from_params(&params));
                (interpolated.camera_to_prophoto, Some(interpolated))
            } else {
                let xyz_to_cam = raw.color_matrix;
                let cam_to_srgb = crate::color::calculate_cam_to_srgb(xyz_to_cam, raw.wb_multipliers, raw.color_matrix_is_d65);
                (cam_to_srgb, None)
            };

            // AI denoise rides in on the mosaic itself (`ml::denoise`).
            let (mosaic, ai_applied) = crate::ml::denoise::effective_mosaic(&raw, image_id, params.ai_denoise);
            match gpu::shared::ImageResources::new(
                &ctx,
                image_id,
                &mosaic,
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
                raw.orientation,
                raw.lens_context.clone(),
            ) {
                Ok(mut resources) => {
                    resources.profile_use = Some(profile_use);
                    resources.ai_denoise = (params.ai_denoise, ai_applied);
                    Ok((ctx, Arc::new(resources)))
                }
                Err(e) => Err(e),
            }
            })
            .await
            .map_err(|e| format!("resource build panicked: {e}"))?
        },
        move |res| Message::FullResReady(image_id, res),
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
        color_matrix_is_d65: raw.color_matrix_is_d65,
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
        lens_context: raw.lens_context.clone(),
        orientation: raw.orientation,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::message::AppTab;

    /// Zoom is kept across images and tabs, so a photo can load straight
    /// into a zoomed-in view. Only the zoom handler used to request full
    /// resolution, so such a view stayed on the subsampled preview — soft —
    /// until the user zoomed again; and a RAW gone from the cache skipped
    /// the upgrade outright. The render now asks.
    ///
    /// Needs a GPU (lavapipe is enough) and a RAW:
    /// `RAWEDITOR_TEST_RAW=x.NEF cargo test --release loads_into_a_zoomed -- --ignored`.
    #[test]
    #[ignore]
    fn loads_into_a_zoomed_in_view_at_full_resolution() {
        let Ok(path) = std::env::var("RAWEDITOR_TEST_RAW") else {
            eprintln!("RAWEDITOR_TEST_RAW not set, skipping");
            return;
        };
        let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).build().expect("runtime");
        let (ctx, preview) = rt.block_on(async {
            let ctx = Arc::new(gpu::shared::SharedContext::new().await.expect("gpu"));
            let raw = raw::loader::load_raw_data(path.clone()).await.expect("decode");
            let fm = crate::color::calculate_cam_to_srgb(raw.color_matrix, raw.wb_multipliers, raw.color_matrix_is_d65);
            let res = gpu::shared::ImageResources::new(
                &ctx, 1, &raw.data, raw.width, raw.height, &Default::default(), raw.wb_multipliers, fm,
                raw.cfa_pattern, raw.black_levels, raw.white_level, None, Some(1280), raw.orientation,
                raw.lens_context.clone(),
            )
            .expect("resources");
            (ctx, Arc::new(res))
        });
        assert!(preview.width < preview.original_width, "the develop preview is subsampled");

        let mut e = RawEditor::new().0;
        e.current_tab = AppTab::Develop;
        e.images = vec![crate::database::models::Image {
            id: 1,
            filename: "x.NEF".into(),
            path,
            cache_path_thumb: None,
            cache_path_instant: None,
            cache_path_working: None,
            cache_path_edited: None,
            file_status: "exists".into(),
            rating: 0,
            flag: 0,
        }];
        e.selected_image_id = Some(1);
        e.gpu_context = Some(ctx.clone());
        e.viewport_size = (1500.0, 1000.0);
        e.scale_factor = 1.0;
        e.editor_readiness = EditorReadiness::Loading(1);

        // At fit on a standard display the preview is sharp: no upgrade.
        e.zoom = 1.0;
        let _ = handle_image_resources_ready(&mut e, 1, Ok((ctx.clone(), preview.clone())));
        assert!(!e.full_res_upgrading, "upgraded at fit");

        // The same image arriving in a view zoomed in on the previous one —
        // with its RAW not cached: the render requests the upgrade, from disk.
        e.is_rendering = false;
        e.zoom = 3.0;
        e.editor_readiness = EditorReadiness::Loading(1);
        assert!(e.raw_cache.get(&1).is_none());
        let _ = handle_image_resources_ready(&mut e, 1, Ok((ctx.clone(), preview.clone())));
        assert!(e.full_res_upgrading, "a zoomed-in view stayed on the subsampled preview");

        // One upgrade at a time: a second request waits for the first.
        assert!(e.full_res_upgrading);
        let _ = trigger_full_res_upgrade(&mut e);
        assert!(e.full_res_upgrading);

        // A failed upgrade keeps the preview to edit on, and is not retried
        // on every render.
        let _ = handle_full_res_ready(&mut e, 1, Err("out of memory".into()));
        assert!(!e.full_res_upgrading);
        assert!(matches!(e.editor_readiness, EditorReadiness::Ready(1)), "editor failed");
        assert!(e.image_resources.is_some());
        e.is_rendering = false;
        let _ = crate::app::handlers::develop::trigger_async_render(&mut e);
        assert!(!e.full_res_upgrading, "retried a failed upgrade");
        // A successful upgrade keeps the preview beside it: renders that full
        // resolution would not sharpen — and a noise-reduction drag, whose
        // stage costs per source pixel — come from the preview instead.
        let full = rt.block_on(async {
            let raw = raw::loader::load_raw_data(e.images[0].path.clone()).await.expect("decode");
            let fm = crate::color::calculate_cam_to_srgb(raw.color_matrix, raw.wb_multipliers, raw.color_matrix_is_d65);
            let res = gpu::shared::ImageResources::new(
                &ctx, 1, &raw.data, raw.width, raw.height, &Default::default(), raw.wb_multipliers, fm,
                raw.cfa_pattern, raw.black_levels, raw.white_level, None, None, raw.orientation,
                raw.lens_context.clone(),
            )
            .expect("resources");
            Arc::new(res)
        });
        e.full_res_upgrading = true;
        e.is_rendering = false;
        let _ = handle_full_res_ready(&mut e, 1, Ok((ctx.clone(), full.clone())));
        assert!(e.image_resources.as_ref().is_some_and(|r| Arc::ptr_eq(r, &full)));
        assert!(e.preview_resources.as_ref().is_some_and(|p| Arc::ptr_eq(p, &preview)), "preview dropped");

        // Zoomed in, an NR drag holds the main view's noise reduction (the
        // detail loupe shows it live); when the drag pauses the main view
        // catches up from the preview, whose stage costs per SOURCE pixel,
        // and the release goes back to full resolution.
        e.is_rendering = false;
        let _ = crate::app::handlers::develop::handle_luma_noise_changed(&mut e, 0.5);
        assert!(e.held_noise.is_some(), "a drag tick re-ran noise reduction on the main view");
        assert!(!e.is_rendering, "a held drag tick rendered the main view");
        let token = e.noise_settle_token;
        let _ = crate::app::handlers::loupe::handle_noise_settled(&mut e, token);
        assert!(e.held_noise.is_none());
        assert!(e.display_from_drag_preview, "the paused drag rendered at full resolution");
        e.is_rendering = false;
        let _ = crate::app::handlers::develop::handle_commit_edit(&mut e);
        assert!(!e.stage_drag);
        assert!(!e.display_from_drag_preview, "the release stayed on the preview");

        // Any load that is not a full-resolution upgrade drops it.
        e.is_rendering = true;
        let _ = handle_image_resources_ready(&mut e, 1, Ok((ctx.clone(), preview.clone())));
        assert!(e.preview_resources.is_none());
    }
}
