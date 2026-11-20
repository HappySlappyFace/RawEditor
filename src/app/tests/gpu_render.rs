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
        e.session.selected_image_id = Some(1);
        e.session.gpu_context = Some(ctx);
        e.session.image_resources = Some(Arc::new(res));
        e.session.viewport_size = (800.0, 600.0);
        e.session.scale_factor = 1.0;

        e.session.is_rendering = true; // a render task is in flight
        let edit = EditParams { exposure: 1.5, ..EditParams::default() };
        let _ = push_uniforms_and_render(&mut e, &edit);
        let pushed = || e.session.image_resources.as_ref().unwrap().pushed_params().exposure;
        assert_eq!(pushed(), 0.0, "the UI thread wrote the uniforms mid-render");
        assert!(e.session.pending_render);

        // The finished render's follow-up applies it before it renders.
        e.session.is_rendering = true;
        let _ = trigger_async_render(&mut e);
        assert_eq!(e.session.image_resources.as_ref().unwrap().pushed_params().exposure, 1.5);
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
        e.session.gpu_context = Some(ctx.clone());
        e.session.gpu_init.set(ctx.clone()).ok();
        let _ = handle_gpu_lost(&mut e);
        assert!(e.session.gpu_context.is_none());
        assert!(e.session.gpu_init.get().is_none(), "the dead context would be handed out again");
        assert!(e.session.status.contains("GPU"));
    });
}

/// Benchmark, not an assertion: the cost of one slider tick end to end, as
/// the UI sees it. Each tick sends `ExposureChanged` through `update` (the
/// UI-thread cost), runs the render task it returns on the GPU, and feeds the
/// `RenderFinished` back through `update`. Used to check that refactors of the
/// app layer cost nothing. Run in an optimised build — with debug assertions
/// on, because the test binary needs them (iced's null renderer, used by the
/// slider widget's tests, only exists under `debug_assertions`):
/// `CARGO_PROFILE_QUICK_DEBUG_ASSERTIONS=true cargo test --profile quick bench_slider_ticks -- --ignored --nocapture`.
#[test]
#[ignore]
fn bench_slider_ticks() {
    use crate::app::message::{AppTab, Message};
    use crate::app::update::update;
    use iced::futures::StreamExt;
    use std::time::{Duration, Instant};

    const WARMUP: usize = 10;
    const TICKS: usize = 200;
    // About the develop preview's size (2.7 MP texture behind a 1600×1000
    // viewport, so no full-resolution upgrade is asked for).
    let (w, h) = (2400u32, 1600u32);
    let data: Vec<u16> = (0..w * h).map(|i| 1000 + ((i * 7919) % 9000) as u16).collect();

    let rt = runtime();
    let ctx = rt.block_on(SharedContext::new()).map(Arc::new).expect("gpu");
    let res = ImageResources::new(
        &ctx, 1, &data, w, h, &EditParams::default(), WB,
        [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0], 0, [BLACK; 4], WHITE,
        None, None, 1, Arc::new(LensContext::default()),
    )
    .expect("image resources");
    res.update_uniforms(&ctx, &EditParams::default(), None, None);
    let mut e = RawEditor::new().0;
    e.session.selected_image_id = Some(1);
    e.session.current_tab = AppTab::Develop;
    e.session.gpu_context = Some(ctx);
    e.session.image_resources = Some(Arc::new(res));
    e.session.viewport_size = (1600.0, 1000.0);
    e.session.scale_factor = 1.0;

    // Run a task to completion, returning the messages it produced.
    let run = |task: iced::Task<Message>| -> Vec<Message> {
        let Some(stream) = iced_runtime::task::into_stream(task) else { return Vec::new() };
        rt.block_on(stream.collect::<Vec<_>>())
            .into_iter()
            .filter_map(|a| match a {
                iced_runtime::Action::Output(m) => Some(m),
                _ => None,
            })
            .collect()
    };

    let (mut t_update, mut t_render, mut t_finish) = (Duration::ZERO, Duration::ZERO, Duration::ZERO);
    let mut ticks = Vec::with_capacity(TICKS);
    for i in 0..WARMUP + TICKS {
        let v = (i % 40) as f32 * 0.05 - 1.0;
        let t0 = Instant::now();
        let task = update(&mut e, Message::ExposureChanged(v));
        let d_update = t0.elapsed();
        let t1 = Instant::now();
        let out = run(task);
        let d_render = t1.elapsed();
        let finished: Vec<_> =
            out.into_iter().filter(|m| matches!(m, Message::RenderFinished(_))).collect();
        assert_eq!(finished.len(), 1, "tick {i}: one render per tick");
        let t2 = Instant::now();
        for m in finished {
            let _ = update(&mut e, m);
        }
        let d_finish = t2.elapsed();
        if i >= WARMUP {
            t_update += d_update;
            t_render += d_render;
            t_finish += d_finish;
            ticks.push(d_update + d_render + d_finish);
        }
    }
    ticks.sort();
    let ms = |d: Duration| d.as_secs_f64() * 1000.0;
    let n = TICKS as u32;
    eprintln!(
        "bench_slider_ticks ({TICKS} ticks, {w}x{h}): update {:.3} ms, render {:.3} ms, \
         finish {:.3} ms, tick mean {:.3} ms, p50 {:.3} ms, p95 {:.3} ms",
        ms(t_update / n),
        ms(t_render / n),
        ms(t_finish / n),
        ms((t_update + t_render + t_finish) / n),
        ms(ticks[TICKS / 2]),
        ms(ticks[TICKS * 95 / 100]),
    );
}

/// A whole export batch, driven through `update` the way the app runs it:
/// every task's messages are fed back until nothing is left. Covers the
/// chain (next image → RAW → resources → render → save → next) and both
/// pixel paths (8-bit JPEG, 16-bit TIFF). The RAW comes from the RAW cache,
/// so no file needs to exist.
#[test]
#[ignore]
fn an_export_batch_runs_to_completion() {
    use crate::app::message::Message;
    use crate::app::update::update;
    use crate::session::{ExportFormat, ExportJobState};
    use iced::futures::StreamExt;

    let dir = std::env::temp_dir().join(format!("raw-editor-export-batch-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let rt = runtime();

    for format in [ExportFormat::Jpeg, ExportFormat::Tiff] {
        let lib = crate::database::library::Library::open_at(&dir.join(format!("{format}.db"))).unwrap();
        let a = lib.import_image("/x/disc-a.nef", "disc-a.nef").unwrap();
        let b = lib.import_image("/x/disc-b.nef", "disc-b.nef").unwrap();
        let mut e = RawEditor::new().0;
        e.session.images = lib.get_all_images().unwrap();
        e.session.library = Some(lib);
        e.session.multi_selection = [a, b].into_iter().collect();
        for id in [a, b] {
            e.session.insert_raw_cache(id, Arc::new(crate::ml::segment::test_support::disc_raw(600, 400)));
        }
        let s = &mut e.session.export_settings;
        s.format = format;
        s.base_path = dir.clone();
        s.subfolder = String::new();
        s.resize = true;
        s.set_max_width_text("300".into());

        let mut pending = vec![update(&mut e, Message::ExportConfirmed)];
        let mut steps = 0;
        while let Some(task) = pending.pop() {
            steps += 1;
            assert!(steps < 100, "the export chain never ended");
            let Some(stream) = iced_runtime::task::into_stream(task) else { continue };
            for action in rt.block_on(stream.collect::<Vec<_>>()) {
                if let iced_runtime::Action::Output(m) = action {
                    pending.push(update(&mut e, m));
                }
            }
        }

        assert!(!e.session.is_exporting, "{format}: batch still running");
        assert_eq!(e.session.status, "Export Complete!", "{format}");
        assert!(e.session.gpu_context.is_some(), "{format}: the context was not kept for reuse");
        for job in &e.session.export_jobs {
            let ExportJobState::Done(path) = &job.state else { panic!("{format}: {job:?}") };
            let bytes = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
            assert!(bytes > 1000, "{format}: {} is {bytes} bytes", path.display());
        }
        assert_eq!(e.session.export_jobs.len(), 2);
    }
    let _ = std::fs::remove_dir_all(&dir);
}
