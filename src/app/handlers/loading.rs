//! Loading is the session's (`session::loading`): its events go straight
//! to `Session::handle`. What is left here are the tests that need the app.

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use crate::app::state::RawEditor;
    use crate::raw;
    use crate::app::state::EditorReadiness;
    use crate::gpu;
    use crate::app::message::AppTab;

    /// With the preload budget at 0 nothing enters `raw_cache`, yet the open
    /// image's RAW must stay reachable (eyedroppers, loupe, AI tools). Runs
    /// the real `RawDataLoaded` handling; `apply_raw_preload_cleanup` runs in it.
    #[test]
    fn the_open_images_raw_is_pinned_with_a_zero_preload_budget() {
        let mut e = RawEditor::new().0;
        e.session.raw_preload_budget_mb = 0;
        e.session.selected_image_id = Some(1);
        e.session.editor_readiness = EditorReadiness::Loading(1);
        let raw = crate::ml::segment::test_support::disc_raw(60, 40);
        let _ = e.session.raw_data_loaded(1, Ok(raw));
        assert!(e.session.raw_cache.is_empty(), "budget 0 caches nothing");
        assert!(e.session.raw_for(1).is_some(), "the open image's RAW was dropped");
        assert!(e.session.raw_for(2).is_none());
        assert!(e.session.sample_raw_rgb(1, 0.5, 0.5).is_ok());
        // Deleting the image releases the pin.
        e.session.remove_from_raw_cache(1);
        assert!(e.session.raw_for(1).is_none());
    }

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
        e.session.current_tab = AppTab::Develop;
        e.session.images = vec![crate::database::models::Image {
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
        e.session.selected_image_id = Some(1);
        e.session.gpu_context = Some(ctx.clone());
        e.session.viewport_size = (1500.0, 1000.0);
        e.session.scale_factor = 1.0;
        e.session.editor_readiness = EditorReadiness::Loading(1);

        // At fit on a standard display the preview is sharp: no upgrade.
        e.session.zoom = 1.0;
        let _ = e.session.image_resources_ready(1, Ok((ctx.clone(), preview.clone())));
        assert!(!e.session.full_res_upgrading, "upgraded at fit");

        // The same image arriving in a view zoomed in on the previous one —
        // with its RAW not cached: the render requests the upgrade, from disk.
        e.session.is_rendering = false;
        e.session.zoom = 3.0;
        e.session.editor_readiness = EditorReadiness::Loading(1);
        assert!(e.session.raw_cache.get(&1).is_none());
        let _ = e.session.image_resources_ready(1, Ok((ctx.clone(), preview.clone())));
        assert!(e.session.full_res_upgrading, "a zoomed-in view stayed on the subsampled preview");

        // One upgrade at a time: a second request waits for the first.
        assert!(e.session.full_res_upgrading);
        let _ = e.session.trigger_full_res_upgrade();
        assert!(e.session.full_res_upgrading);

        // A failed upgrade keeps the preview to edit on, and is not retried
        // on every render.
        let _ = e.session.full_res_ready(1, Err("out of memory".into()));
        assert!(!e.session.full_res_upgrading);
        assert!(matches!(e.session.editor_readiness, EditorReadiness::Ready(1)), "editor failed");
        assert!(e.session.image_resources.is_some());
        e.session.is_rendering = false;
        let _ = e.session.render();
        assert!(!e.session.full_res_upgrading, "retried a failed upgrade");
        // A successful upgrade keeps the preview beside it: renders that full
        // resolution would not sharpen — and a noise-reduction drag, whose
        // stage costs per source pixel — come from the preview instead.
        let full = rt.block_on(async {
            let raw = raw::loader::load_raw_data(e.session.images[0].path.clone()).await.expect("decode");
            let fm = crate::color::calculate_cam_to_srgb(raw.color_matrix, raw.wb_multipliers, raw.color_matrix_is_d65);
            let res = gpu::shared::ImageResources::new(
                &ctx, 1, &raw.data, raw.width, raw.height, &Default::default(), raw.wb_multipliers, fm,
                raw.cfa_pattern, raw.black_levels, raw.white_level, None, None, raw.orientation,
                raw.lens_context.clone(),
            )
            .expect("resources");
            Arc::new(res)
        });
        e.session.full_res_upgrading = true;
        e.session.is_rendering = false;
        let _ = e.session.full_res_ready(1, Ok((ctx.clone(), full.clone())));
        assert!(e.session.image_resources.as_ref().is_some_and(|r| Arc::ptr_eq(r, &full)));
        assert!(e.session.preview_resources.as_ref().is_some_and(|p| Arc::ptr_eq(p, &preview)), "preview dropped");

        // Zoomed in, an NR drag holds the main view's noise reduction (the
        // detail loupe shows it live); when the drag pauses the main view
        // catches up from the preview, whose stage costs per SOURCE pixel,
        // and the release goes back to full resolution.
        e.session.is_rendering = false;
        let _ = crate::app::handlers::develop::handle_luma_noise_changed(&mut e, 0.5);
        assert!(e.session.held_noise.is_some(), "a drag tick re-ran noise reduction on the main view");
        assert!(!e.session.is_rendering, "a held drag tick rendered the main view");
        let token = e.session.noise_settle_token;
        let _ = e.session.noise_settled(token);
        assert!(e.session.held_noise.is_none());
        assert!(e.session.display_from_drag_preview, "the paused drag rendered at full resolution");
        e.session.is_rendering = false;
        let _ = crate::app::handlers::develop::handle_commit_edit(&mut e);
        assert!(!e.session.stage_drag);
        assert!(!e.session.display_from_drag_preview, "the release stayed on the preview");

        // Any load that is not a full-resolution upgrade drops it.
        e.session.is_rendering = true;
        let _ = e.session.image_resources_ready(1, Ok((ctx.clone(), preview.clone())));
        assert!(e.session.preview_resources.is_none());
    }
}
