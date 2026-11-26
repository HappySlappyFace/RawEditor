//! A headless run of the editing session on one RAW, the way a front end
//! drives it: import it into a fresh catalogue, open it in Develop, move
//! Exposure through a series of renders, and export it at full resolution as
//! a JPEG. Prints the GPU it found, how long each kind of work took, and the
//! peak memory.
//!
//! It exists to answer "does RawEditor run on this platform, and how fast?"
//! before any UI is written for it — Android first, over `adb shell`. It runs
//! on the desktop too, which is how it is checked.
//!
//! Everything it writes (catalogue, caches, export) stays under its work
//! folder; nothing touches the user's own data or cache folders.
//!
//! ```text
//! raw-editor-probe <photo.raw> [--work DIR] [--ticks N] [--surface]
//! ```
//!
//! `--surface` draws the ticks the way a front end that owns its surface
//! does (`Session::surface_frame` → `render_to_target`), into a stand-in
//! 1080×2400 texture, with no readback of the display.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use raw_editor_session::session::{
    AppTab, EditorReadiness, ExportFormat, ExportJobState, SelectionChange, Session, SessionEvent,
};
use raw_editor_session::Effects;

struct Args {
    raw: PathBuf,
    work: PathBuf,
    ticks: usize,
    surface: bool,
}

fn parse_args() -> Result<Args, String> {
    let mut raw = None;
    let mut work = None;
    let mut ticks = 100;
    let mut surface = false;
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--work" => work = Some(PathBuf::from(it.next().ok_or("--work needs a folder")?)),
            "--ticks" => {
                ticks = it.next().ok_or("--ticks needs a number")?.parse().map_err(|e| format!("--ticks: {e}"))?
            }
            "--surface" => surface = true,
            "-h" | "--help" => return Err(String::new()),
            _ if raw.is_none() => raw = Some(PathBuf::from(a)),
            _ => return Err(format!("unexpected argument {a}")),
        }
    }
    let raw = raw.ok_or("which RAW file?")?;
    let work = work.unwrap_or_else(|| std::env::temp_dir().join("raw-editor-probe"));
    Ok(Args { raw, work, ticks, surface })
}

fn main() {
    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            if !e.is_empty() {
                eprintln!("{e}");
            }
            eprintln!("usage: raw-editor-probe <photo.raw> [--work DIR] [--ticks N] [--surface]");
            std::process::exit(2);
        }
    };
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "warn".into()),
        )
        .with_writer(std::io::stderr)
        .init();

    match run(&args) {
        Ok(()) => println!("\nPROBE OK"),
        Err(e) => {
            println!("\nPROBE FAILED: {e}");
            std::process::exit(1);
        }
    }
}

/// Keep every file the session writes inside `work`. On Linux the data and
/// cache folders follow XDG; on Android the platform reports neither, so the
/// session falls back to the current folder, which is `work` too.
fn confine_to(work: &Path) -> Result<(), String> {
    let _ = std::fs::remove_dir_all(work);
    for sub in ["data", "cache", "photos", "export"] {
        std::fs::create_dir_all(work.join(sub)).map_err(|e| format!("{}: {e}", work.display()))?;
    }
    // Before any thread exists: the runtime is created after this.
    std::env::set_var("XDG_DATA_HOME", work.join("data"));
    std::env::set_var("XDG_CACHE_HOME", work.join("cache"));
    std::env::set_current_dir(work).map_err(|e| e.to_string())?;
    Ok(())
}

fn run(args: &Args) -> Result<(), String> {
    let raw = std::fs::canonicalize(&args.raw).map_err(|e| format!("{}: {e}", args.raw.display()))?;
    confine_to(&args.work)?;
    let work = std::fs::canonicalize(&args.work).map_err(|e| e.to_string())?;
    println!("RawEditor probe {}", env!("CARGO_PKG_VERSION"));
    println!("platform   {} {}", std::env::consts::OS, std::env::consts::ARCH);
    println!("work       {}", work.display());

    let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().map_err(|e| e.to_string())?;
    let mut probe = Probe { session: Session::new(&Default::default()), rt, times: BTreeMap::new() };

    // 1. The GPU.
    rt_section("GPU");
    list_adapters();
    let t = Instant::now();
    let ctx = probe.rt.block_on(raw_editor_session::gpu::shared::SharedContext::new())?;
    println!("context    {:.0} ms", ms(t.elapsed()));
    probe.session.gpu_context = Some(std::sync::Arc::new(ctx));

    // 2. A catalogue with the one photo in it. The photo is copied into its
    //    own folder, because importing a folder imports all of it.
    rt_section("Import");
    let name = raw.file_name().ok_or("the RAW path has no file name")?;
    let photo = work.join("photos").join(name);
    std::fs::copy(&raw, &photo).map_err(|e| format!("copying the RAW: {e}"))?;
    let library = raw_editor_session::database::library::Library::open_at(&work.join("catalogue.db"))
        .map_err(|e| format!("opening the catalogue: {e}"))?;
    probe.session.library = Some(library);
    let fx = probe.session.import_folder(work.join("photos"));
    probe.drive(fx)?;
    println!("status     {}", probe.session.status);
    let id = probe.session.images.first().map(|i| i.id).ok_or("the import found no photo")?;

    // 3. Open it in Develop, as on a 1080×2400 phone held upright.
    rt_section("Develop");
    probe.session.viewport_size = (1080.0, 1800.0);
    probe.session.scale_factor = 1.0;
    let fx = probe.session.set_tab(AppTab::Develop);
    probe.drive(fx)?;
    let t = Instant::now();
    let fx = probe.session.select_image(id, SelectionChange::Only);
    probe.drive(fx)?;
    match &probe.session.editor_readiness {
        EditorReadiness::Ready(ready) if *ready == id => {}
        other => return Err(format!("Develop did not finish loading: {other:?} ({})", probe.session.status)),
    }
    println!("open       {:.0} ms to the first render", ms(t.elapsed()));
    if let Some(res) = &probe.session.image_resources {
        let (w, h) = res.oriented_original_dims(0);
        println!("photo      {w}×{h}, develop texture {}×{}", res.width, res.height);
    }
    let target = probe.session.render_target();
    println!(
        "render     {}×{} target, magnification {:.2}",
        target.width, target.height, target.magnification
    );

    // 4. Slider ticks: one render each, as when Exposure is dragged.
    rt_section(if args.surface { "Slider ticks (drawn onto a surface)" } else { "Slider ticks" });
    probe.session.profiler.history.clear();
    let mut ticks = Vec::with_capacity(args.ticks);
    let surface = args.surface.then(|| StandInSurface::new(&probe.session, 1080, 2400)).flatten();
    if args.surface && surface.is_none() {
        return Err("no GPU context for the stand-in surface".into());
    }
    for i in 0..args.ticks {
        probe.session.current_edit_params.exposure = (i % 40) as f32 * 0.05 - 1.0;
        let t = Instant::now();
        match &surface {
            Some(surface) => surface.draw(&mut probe)?,
            None => {
                let fx = probe.session.update_pipeline();
                probe.drive(fx)?;
            }
        }
        ticks.push(t.elapsed());
    }
    print_breakdown(&probe.session.profiler);
    probe.session.current_edit_params.exposure = 0.5;
    let fx = probe.session.update_pipeline();
    probe.drive(fx)?;
    probe.session.flush_edits();
    if let Some((i, slowest)) = ticks.iter().enumerate().max_by_key(|(_, d)| **d) {
        println!("slowest    tick {i} of {}, {:.1} ms", ticks.len(), ms(*slowest));
    }
    if !ticks.is_empty() {
        ticks.sort();
        let n = ticks.len();
        let mean = ticks.iter().sum::<Duration>() / n as u32;
        println!(
            "{n} ticks   mean {:.1} ms, p50 {:.1} ms, p95 {:.1} ms, max {:.1} ms",
            ms(mean),
            ms(ticks[n / 2]),
            ms(ticks[(n * 95 / 100).min(n - 1)]),
            ms(ticks[n - 1]),
        );
    }

    // 5. Export at full resolution.
    rt_section("Export");
    let s = &mut probe.session.export_settings;
    s.format = ExportFormat::Jpeg;
    s.base_path = work.join("export");
    s.subfolder = String::new();
    s.resize = false;
    let t = Instant::now();
    let fx = probe.session.start_export().ok_or_else(|| probe.session.status.clone())?;
    probe.drive(fx)?;
    println!("export     {:.0} ms", ms(t.elapsed()));
    for job in &probe.session.export_jobs {
        match &job.state {
            ExportJobState::Done(path) => {
                let size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
                println!("saved      {} ({:.1} MB)", path.display(), size as f64 / 1e6);
            }
            other => return Err(format!("export of {}: {other:?}", job.filename)),
        }
    }

    rt_section("Where the time went");
    for (label, (n, total)) in &probe.times {
        println!("{label:<28} {n:>4} × {:>8.1} ms = {:>8.0} ms", ms(*total) / *n as f64, ms(*total));
    }
    if let Some(peak) = peak_memory_mb() {
        println!("peak memory  {peak} MB");
    }
    Ok(())
}

/// A texture standing in for a phone's surface, for `--surface`.
struct StandInSurface {
    view: wgpu::TextureView,
    size: (u32, u32),
}

impl StandInSurface {
    fn new(session: &Session, width: u32, height: u32) -> Option<Self> {
        let ctx = session.gpu_context.as_ref()?;
        let texture = ctx.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("stand-in surface"),
            size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        Some(Self { view: texture.create_view(&Default::default()), size: (width, height) })
    }

    /// One frame, as a surface front end draws it on vsync.
    fn draw(&self, probe: &mut Probe) -> Result<(), String> {
        probe.session.mark_edits_dirty();
        let t = Instant::now();
        let frame = probe.session.surface_frame(self.size.0, self.size.1).ok_or("no image to draw")?;
        let target = raw_editor_session::gpu::render_functions::DisplayTarget {
            view: &self.view,
            format: wgpu::TextureFormat::Rgba8Unorm,
            size: self.size,
            placement: frame.placement,
            background: wgpu::Color::BLACK,
        };
        let out = probe.rt.block_on(raw_editor_session::gpu::render_functions::render_to_target(
            &frame.context,
            &frame.display,
            &frame.overview,
            target,
            frame.overview_dims,
            Default::default(),
            || {},
        ))?;
        let fx = probe.session.surface_frame_finished(&frame, out);
        let entry = probe.times.entry("surface frame".into()).or_default();
        entry.0 += 1;
        entry.1 += t.elapsed();
        probe.drive(fx)
    }
}

struct Probe {
    session: Session,
    rt: tokio::runtime::Runtime,
    /// Count and total time per kind of work.
    times: BTreeMap<String, (u32, Duration)>,
}

impl Probe {
    /// Run `effects` and everything they lead to (`session::host::run`),
    /// adding up the time per kind of work.
    fn drive(&mut self, effects: Effects<SessionEvent>) -> Result<(), String> {
        let times = &mut self.times;
        raw_editor_session::session::host::run(&mut self.session, &self.rt, effects, |label, took| {
            let entry = times.entry(label.to_string()).or_default();
            entry.0 += 1;
            entry.1 += took;
        })?;
        // Nothing draws here; drop what a front end would redraw.
        self.session.changed.take();
        self.session.camera_preview = None;
        Ok(())
    }
}

/// Every adapter wgpu can see, with the limits mobile GPUs most often fall
/// short on.
fn list_adapters() {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor { backends: wgpu::Backends::all(), ..Default::default() });
    let adapters = instance.enumerate_adapters(wgpu::Backends::all());
    if adapters.is_empty() {
        println!("adapters   none");
    }
    for a in adapters {
        let i = a.get_info();
        let l = a.limits();
        println!("adapter    {} ({:?}, {:?}) driver {} {}", i.name, i.backend, i.device_type, i.driver, i.driver_info);
        println!(
            "  limits   texture 2D {}, buffer {} MB, storage buffers/stage {}, bind groups {}",
            l.max_texture_dimension_2d,
            l.max_buffer_size >> 20,
            l.max_storage_buffers_per_shader_stage,
            l.max_bind_groups,
        );
    }
}

/// Where the ticks' renders spent their time, from the engine's profiler
/// (`--features profiling`; without it there is nothing to show).
fn print_breakdown(profiler: &raw_editor_session::core::profiler::Profiler) {
    use raw_editor_session::core::profiler::{Count, Span, ENABLED};
    if !ENABLED {
        println!("(build with --features profiling for the per-render breakdown)");
        return;
    }
    println!("per render ({} frames)        mean      max", profiler.history.len());
    for span in Span::ALL {
        let (_, mean, max) = profiler.span_stats(span);
        if max > 0.0 {
            println!("  {:<24} {mean:>7.2} ms {max:>7.2} ms", span.label());
        }
    }
    for count in Count::ALL {
        let n = profiler.count_total(count);
        if n > 0 {
            println!("  {:<24} {n:>7}", count.label());
        }
    }
}

fn rt_section(name: &str) {
    println!("\n── {name}");
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

/// Peak resident memory, from `/proc` (Linux and Android).
fn peak_memory_mb() -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    let line = status.lines().find(|l| l.starts_with("VmHWM:"))?;
    let kb: u64 = line.split_whitespace().nth(1)?.parse().ok()?;
    Some(kb / 1024)
}
