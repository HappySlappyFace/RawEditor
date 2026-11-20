//! GPU tests that need the editor itself (render scheduling, device loss).
//! They lived in the engine's `gpu::eval_tests` until the engine became its own
//! crate; what they check is the app's handling, so they live here now. Need a
//! GPU (lavapipe works), so `#[ignore]`d like the rest of the GPU tests.

use std::sync::Arc;

use crate::app::handlers::develop::{handle_gpu_lost, push_uniforms_and_render, trigger_async_render};
use crate::app::state::RawEditor;
use crate::core::lens::LensContext;
use crate::core::types::EditParams;
use crate::gpu::shared::{ImageResources, SharedContext};

const BLACK: u32 = 512;
const WHITE: u32 = 16383;
const WB: [f32; 4] = [2.0, 1.0, 1.5, 1.0];

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread().worker_threads(2).build().expect("runtime")
}

/// The UI thread must not write the uniform block while a render is in
/// flight (it would land `view_rect = FULL` or newer params in the render's
/// main pass). A push made mid-render is parked and applied by the next
/// render, which is its only writer.
#[test]
#[ignore]
fn a_push_during_a_render_waits_for_the_next_render() {
    // The content is irrelevant here: a flat mid-grey mosaic.
    let (w, h) = (200u32, 200u32);
    let data = vec![4000u16; (w * h) as usize];
    runtime().block_on(async {
        let ctx = Arc::new(SharedContext::new().await.expect("gpu"));
        let res = ImageResources::new(
            &ctx, 1, &data, w, h, &EditParams::default(), WB,
            [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0], 0, [BLACK; 4], WHITE,
            None, None, 1, Arc::new(LensContext::default()),
        )
        .expect("image resources");
        res.update_uniforms(&ctx, &EditParams::default(), None, None);
        let mut e = RawEditor::new().0;
        e.selected_image_id = Some(1);
        e.gpu_context = Some(ctx);
        e.image_resources = Some(Arc::new(res));
        e.viewport_size = (800.0, 600.0);
        e.scale_factor = 1.0;

        e.is_rendering = true; // a render task is in flight
        let edit = EditParams { exposure: 1.5, ..EditParams::default() };
        let _ = push_uniforms_and_render(&mut e, &edit);
        let pushed = || e.image_resources.as_ref().unwrap().pushed_params().exposure;
        assert_eq!(pushed(), 0.0, "the UI thread wrote the uniforms mid-render");
        assert!(e.pending_render);

        // The finished render's follow-up applies it before it renders.
        e.is_rendering = true;
        let _ = trigger_async_render(&mut e);
        assert_eq!(e.image_resources.as_ref().unwrap().pushed_params().exposure, 1.5);
    });
}

/// A lost device is detected, and the editor drops the dead context (and the
/// `gpu_init` cell that would hand it back) so the next load builds a new one.
#[test]
#[ignore]
fn a_lost_device_is_detected_and_the_context_replaced() {
    runtime().block_on(async {
        let ctx = Arc::new(SharedContext::new().await.expect("gpu"));
        assert!(!ctx.is_lost());
        ctx.device.destroy();
        ctx.device.poll(wgpu::Maintain::Wait);
        assert!(ctx.is_lost(), "destroying the device was not noticed");

        let mut e = RawEditor::new().0;
        e.gpu_context = Some(ctx.clone());
        e.gpu_init.set(ctx.clone()).ok();
        let _ = handle_gpu_lost(&mut e);
        assert!(e.gpu_context.is_none());
        assert!(e.gpu_init.get().is_none(), "the dead context would be handed out again");
        assert!(e.status.contains("GPU"));
    });
}
