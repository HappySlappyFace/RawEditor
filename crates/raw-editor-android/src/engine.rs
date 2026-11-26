//! The render thread: the session, the GPU context and the surface live
//! here and nowhere else. Messages from the UI are drained in batches —
//! twenty slider moves between two frames become one edit — and a frame is
//! drawn only when something changed, then waits for vsync in
//! `get_current_texture` (FIFO presentation). With nothing to draw the
//! thread sleeps in `recv`, so an idle screen costs nothing.
//!
//! It never waits on the session's work either: imports, decodes and the
//! full-resolution rebuild run on the tokio runtime, and their results come
//! back through the same channel as the UI's messages (`Command::Event`).

use std::collections::{HashSet, VecDeque};
use std::path::PathBuf;
use std::ptr::NonNull;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use raw_editor_session::gpu::render_functions::{render_to_target, DisplayTarget};
use raw_editor_session::gpu::shared::{pick_surface_format, SharedContext};
use raw_editor_session::session::{host, AppTab, EditorReadiness, LoadJob, SelectionChange, Session, SessionEvent};
use raw_editor_session::{Changed, Effects};

pub enum Command {
    Open(PathBuf),
    Surface(ndk::native_window::NativeWindow, u32, u32),
    /// The surface is going; answer once it is dropped.
    SurfaceGone(Sender<()>),
    Exposure(f32),
    /// A pinch or drag: scale the zoom about `at`, then move by `pan`
    /// (surface pixels).
    View { scale: f32, at: [f32; 2], pan: [f32; 2] },
    /// A double tap at a point: back to fit, or in to 3× about it.
    ToggleZoom([f32; 2]),
    /// A piece of the session's background work finished.
    Event(Box<SessionEvent>),
    /// Save and stop.
    Quit,
}

/// The handle Kotlin holds.
pub struct Engine {
    tx: Option<Sender<Command>>,
    stats: Arc<Mutex<Stats>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Engine {
    pub fn start(files: PathBuf) -> Self {
        // The session's data and cache folders: Android reports neither, so
        // `core::paths` falls back to $HOME and the working folder.
        std::env::set_var("HOME", &files);
        let _ = std::env::set_current_dir(&files);
        let (tx, rx) = mpsc::channel();
        let stats = Arc::new(Mutex::new(Stats::default()));
        let thread = {
            let stats = stats.clone();
            let results = tx.clone();
            std::thread::Builder::new()
                .name("raweditor-render".into())
                .spawn(move || {
                    if let Err(e) = RenderThread::new(files, stats.clone(), results).and_then(|t| t.run(rx)) {
                        tracing::error!("render thread stopped: {e}");
                        stats.lock().unwrap_or_else(|e| e.into_inner()).status = format!("Error: {e}");
                    }
                })
                .expect("spawn the render thread")
        };
        Self { tx: Some(tx), stats, thread: Some(thread) }
    }

    pub fn send(&self, command: Command) {
        if let Some(tx) = &self.tx {
            let _ = tx.send(command);
        }
    }

    /// Block until the render thread has dropped the surface.
    pub fn release_surface(&self) {
        let (ack, done) = mpsc::channel();
        self.send(Command::SurfaceGone(ack));
        let _ = done.recv_timeout(Duration::from_secs(2));
    }

    pub fn summary(&self) -> String {
        self.stats.lock().unwrap_or_else(|e| e.into_inner()).summary()
    }

    /// How many photos have opened, and the last one's saved exposure: the
    /// UI sets its slider from it when the count moves.
    pub fn opened(&self) -> (u32, f32) {
        let stats = self.stats.lock().unwrap_or_else(|e| e.into_inner());
        (stats.opened, stats.exposure)
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        // The thread holds a sender of its own (for its results), so the
        // channel never closes by itself: ask it to stop. It saves on the way out.
        self.send(Command::Quit);
        self.tx.take();
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// What the readout shows.
#[derive(Default)]
pub struct Stats {
    status: String,
    opened: u32,
    exposure: f32,
    /// Recent frames: (work on the render thread, interval since the previous present).
    frames: VecDeque<(f32, Option<f32>)>,
    last_present: Option<Instant>,
    /// This second's frames, logged when it ends: a long session can then
    /// be read back from logcat next to temperatures and battery current.
    second: Option<(Instant, Vec<f32>)>,
}

impl Stats {
    fn frame(&mut self, work_ms: f32) {
        let now = Instant::now();
        let interval = self.last_present.map(|t| now.duration_since(t).as_secs_f32() * 1000.0);
        self.last_present = Some(now);
        if self.frames.len() == 120 {
            self.frames.pop_front();
        }
        self.frames.push_back((work_ms, interval));

        let (start, work) = self.second.get_or_insert_with(|| (now, Vec::new()));
        work.push(work_ms);
        if now.duration_since(*start) >= Duration::from_secs(1) {
            let mut work = std::mem::take(work);
            work.sort_by(f32::total_cmp);
            let pct = |p: usize| work[(work.len() * p / 100).min(work.len() - 1)];
            tracing::info!("frames {} in 1 s, work p50 {:.1} ms, p95 {:.1} ms", work.len(), pct(50), pct(95));
            self.second = None;
        }
    }

    fn summary(&self) -> String {
        let mut work: Vec<f32> = self.frames.iter().map(|f| f.0).collect();
        if work.is_empty() {
            return self.status.clone();
        }
        work.sort_by(f32::total_cmp);
        let pct = |p: usize| work[(work.len() * p / 100).min(work.len() - 1)];
        // Frame rate over consecutive frames only: a pause between drags is
        // not a slow frame.
        let intervals: Vec<f32> = self.frames.iter().filter_map(|f| f.1).filter(|&i| i < 100.0).collect();
        let fps = if intervals.is_empty() { 0.0 } else { 1000.0 * intervals.len() as f32 / intervals.iter().sum::<f32>() };
        format!(
            "{}  ·  frame p50 {:.1} ms, p95 {:.1} ms  ·  {:.0} fps while dragging",
            self.status,
            pct(50),
            pct(95),
            fps
        )
    }
}

/// Only the newest surface in a batch counts. Android replaces a
/// SurfaceView's surface whenever it likes, and while the thread was busy
/// (opening a photo) several changes can queue up; attaching to one already
/// replaced fails on its abandoned buffer queue. Goodbyes are still answered,
/// and in order, so `surfaceDestroyed` never waits longer than it must.
fn latest_surface_only(batch: Vec<Command>) -> Vec<Command> {
    let last = batch.iter().rposition(|c| matches!(c, Command::Surface(..) | Command::SurfaceGone(_)));
    batch
        .into_iter()
        .enumerate()
        .filter(|(i, c)| !matches!(c, Command::Surface(..)) || Some(*i) == last)
        .map(|(_, c)| c)
        .collect()
}

struct Live {
    _window: ndk::native_window::NativeWindow,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    view_format: wgpu::TextureFormat,
}

/// A photo on its way to the screen.
enum Opening {
    /// Its folder is being imported.
    Importing { path: PathBuf, since: Instant },
    /// Selected; its RAW is loading.
    Loading { id: i64, name: String, since: Instant },
}

struct RenderThread {
    rt: tokio::runtime::Runtime,
    session: Session,
    context: Arc<SharedContext>,
    files: PathBuf,
    live: Option<Live>,
    stats: Arc<Mutex<Stats>>,
    /// Where background work sends its results.
    results: Sender<Command>,
    /// The session's background loads already running.
    started: HashSet<LoadJob>,
    opening: Option<Opening>,
}

impl RenderThread {
    fn new(files: PathBuf, stats: Arc<Mutex<Stats>>, results: Sender<Command>) -> Result<Self, String> {
        let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().map_err(|e| e.to_string())?;
        let context = Arc::new(rt.block_on(SharedContext::new())?);
        let info = context.adapter.get_info();
        tracing::info!("GPU: {} ({:?})", info.name, info.backend);
        let mut session = Session::new(&Default::default());
        session.gpu_context = Some(context.clone());
        // This thread draws on vsync: the session must not render on its own.
        session.surface_drawn = true;
        Ok(Self { rt, session, context, files, live: None, stats, results, started: HashSet::new(), opening: None })
    }

    fn status(&self, text: impl Into<String>) {
        self.stats.lock().unwrap_or_else(|e| e.into_inner()).status = text.into();
    }

    fn run(mut self, rx: Receiver<Command>) -> Result<(), String> {
        let mut dirty = false;
        loop {
            // Sleep until there is something to do, then take everything
            // waiting: the latest slider value is the only one that counts.
            let mut batch = Vec::new();
            if !(dirty && self.can_draw()) {
                match rx.recv() {
                    Ok(c) => batch.push(c),
                    Err(_) => break,
                }
            }
            batch.extend(rx.try_iter());
            let mut quit = false;
            for command in latest_surface_only(batch) {
                match command {
                    Command::Quit => quit = true,
                    command => dirty |= self.apply(command)?,
                }
            }
            self.start_loads();
            self.check_opened();
            // Nothing else here draws from what `changed` signals.
            dirty |= self.session.changed.take().contains(Changed::FRAME);
            self.session.camera_preview = None;
            if quit {
                break;
            }
            if dirty && self.can_draw() {
                self.draw()?;
                dirty = false;
            }
        }
        // Leaving: save the edit and the edited proxy.
        self.session.flush_edits();
        let fx = self.session.set_tab(AppTab::Library);
        host::run(&mut self.session, &self.rt, fx, |_, _| {})?;
        Ok(())
    }

    fn can_draw(&self) -> bool {
        self.live.is_some() && matches!(self.session.editor_readiness, EditorReadiness::Ready(_))
    }

    /// Apply one message; true when the screen needs a new frame.
    fn apply(&mut self, command: Command) -> Result<bool, String> {
        match command {
            Command::Open(path) => {
                self.open(path);
                Ok(false)
            }
            Command::Surface(window, w, h) => match self.attach(window, w, h) {
                Ok(()) => Ok(true),
                // Not fatal: Android sends a new surface when it can.
                Err(e) => {
                    tracing::error!("could not draw on the surface: {e}");
                    self.live = None;
                    self.status(format!("Error: {e}"));
                    Ok(false)
                }
            },
            Command::SurfaceGone(ack) => {
                self.live = None;
                let _ = ack.send(());
                Ok(false)
            }
            Command::Exposure(ev) => {
                self.session.current_edit_params.exposure = ev;
                self.session.mark_edits_dirty();
                Ok(true)
            }
            Command::View { scale, at, pan } => {
                self.session.transform_view(scale, at, pan);
                Ok(false)
            }
            Command::ToggleZoom(at) => {
                let scale = if self.session.zoom > 1.01 { 1.0 / self.session.zoom } else { 3.0 };
                self.session.transform_view(scale, at, [0.0, 0.0]);
                Ok(false)
            }
            Command::Event(event) => {
                let import_done = matches!(*event, SessionEvent::ImportComplete(_));
                let fx = host::handle(&mut self.session, *event);
                self.spawn(fx);
                if import_done {
                    self.imported();
                }
                Ok(false)
            }
            Command::Quit => Ok(false),
        }
    }

    /// Run the session's work in the background; each result comes back as
    /// a `Command::Event`.
    fn spawn(&mut self, effects: Effects<SessionEvent>) {
        for effect in effects {
            let results = self.results.clone();
            self.rt.spawn(async move {
                let event = effect.future.await;
                let _ = results.send(Command::Event(Box::new(event)));
            });
        }
    }

    /// Start the background loads the session lists (the photo's RAW,
    /// neighbours' previews), each once.
    fn start_loads(&mut self) {
        let jobs: Vec<LoadJob> = self.session.load_jobs().collect();
        self.started.retain(|j| jobs.contains(j));
        for job in jobs {
            if self.started.insert(job.clone()) {
                let results = self.results.clone();
                self.rt.spawn(async move {
                    let event = job.run().await;
                    let _ = results.send(Command::Event(Box::new(event)));
                });
            }
        }
    }

    /// Open a RAW: import its folder if the catalogue doesn't have it yet,
    /// then select it. Nothing here waits; `imported` and `check_opened`
    /// take it on as the work comes back.
    fn open(&mut self, path: PathBuf) {
        let since = Instant::now();
        self.status(format!("Opening {}…", path.file_name().unwrap_or_default().to_string_lossy()));
        if self.session.library.is_none() {
            match raw_editor_session::database::library::Library::open_at(&self.files.join("catalogue.db")) {
                Ok(library) => {
                    self.session.images = library.get_all_images().unwrap_or_default();
                    self.session.library = Some(library);
                }
                Err(e) => {
                    self.status(format!("Error: opening the catalogue: {e}"));
                    return;
                }
            }
        }
        if let Some(id) = self.image_id(&path) {
            self.select(id, &path, since);
            return;
        }
        let Some(folder) = path.parent().map(PathBuf::from) else {
            self.status("Error: the RAW has no folder");
            return;
        };
        let fx = self.session.import_folder(folder);
        self.spawn(fx);
        self.opening = Some(Opening::Importing { path, since });
    }

    fn image_id(&self, path: &std::path::Path) -> Option<i64> {
        let path = path.to_string_lossy();
        self.session.images.iter().find(|i| i.path == path).map(|i| i.id)
    }

    /// An import finished: select the photo it was for.
    fn imported(&mut self) {
        let Some(Opening::Importing { path, since }) = self.opening.take() else { return };
        match self.image_id(&path) {
            Some(id) => self.select(id, &path, since),
            None => self.status(format!("Error: {} was not imported: {}", path.display(), self.session.status)),
        }
    }

    fn select(&mut self, id: i64, path: &std::path::Path, since: Instant) {
        let fx = self.session.set_tab(AppTab::Develop);
        self.spawn(fx);
        let fx = self.session.select_image(id, SelectionChange::Only);
        self.spawn(fx);
        let name = path.file_name().unwrap_or_default().to_string_lossy().to_string();
        self.opening = Some(Opening::Loading { id, name, since });
    }

    /// The selected photo is ready (or failed): say so, and hand its saved
    /// exposure to the UI's slider.
    fn check_opened(&mut self) {
        let Some(Opening::Loading { id, name, since }) = &self.opening else { return };
        let ms = since.elapsed().as_millis();
        match &self.session.editor_readiness {
            EditorReadiness::Ready(r) if r == id => {
                tracing::info!("opened {name} in {ms} ms");
                let mut stats = self.stats.lock().unwrap_or_else(|e| e.into_inner());
                stats.status = format!("{name}, opened in {ms} ms");
                stats.exposure = self.session.current_edit_params.exposure;
                stats.opened += 1;
            }
            EditorReadiness::Failed(f, e) if f == id => self.status(format!("Error: {name} did not load: {e}")),
            _ => return,
        }
        self.opening = None;
    }

    fn attach(&mut self, window: ndk::native_window::NativeWindow, width: u32, height: u32) -> Result<(), String> {
        // The same window at a new size only needs reconfiguring.
        if let Some(live) = &mut self.live {
            if live._window == window {
                live.config.width = width;
                live.config.height = height;
                live.surface.configure(&self.context.device, &live.config);
                self.session.viewport_size = (width as f32, height as f32);
                return Ok(());
            }
        }
        self.live = None;
        let ptr = NonNull::new(window.ptr().as_ptr().cast()).ok_or("null native window")?;
        let target = wgpu::SurfaceTargetUnsafe::RawHandle {
            raw_display_handle: raw_window_handle::RawDisplayHandle::Android(
                raw_window_handle::AndroidDisplayHandle::new(),
            ),
            raw_window_handle: raw_window_handle::RawWindowHandle::AndroidNdk(
                raw_window_handle::AndroidNdkWindowHandle::new(ptr),
            ),
        };
        // SAFETY: `window` holds a reference to the ANativeWindow and is kept
        // in `Live` alongside the surface, which is dropped first.
        let surface = unsafe { self.context.create_surface(target)? };
        let caps = surface.get_capabilities(&self.context.adapter);
        let (format, view_format) =
            pick_surface_format(&caps.formats).ok_or_else(|| format!("no usable surface format in {:?}", caps.formats))?;
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width,
            height,
            // Vsync: `get_current_texture` waits for a free buffer, which
            // paces the loop to the display.
            present_mode: wgpu::PresentMode::Fifo,
            desired_maximum_frame_latency: 2,
            alpha_mode: caps.alpha_modes.first().copied().unwrap_or(wgpu::CompositeAlphaMode::Auto),
            view_formats: if view_format != format { vec![view_format] } else { vec![] },
        };
        surface.configure(&self.context.device, &config);
        tracing::info!("surface {width}×{height}, {format:?} drawn as {view_format:?}");
        self.session.viewport_size = (width as f32, height as f32);
        self.session.scale_factor = 1.0;
        self.live = Some(Live { _window: window, surface, config, view_format });
        Ok(())
    }

    fn draw(&mut self) -> Result<(), String> {
        let Some(live) = &self.live else { return Ok(()) };
        let texture = match live.surface.get_current_texture() {
            Ok(t) => t,
            Err(wgpu::SurfaceError::Outdated | wgpu::SurfaceError::Lost) => {
                live.surface.configure(&self.context.device, &live.config);
                return Ok(());
            }
            Err(e) => {
                tracing::warn!("no surface texture this frame: {e}");
                return Ok(());
            }
        };
        // Timed from here: the wait for a free buffer (vsync) is not work.
        let t = Instant::now();
        let (w, h) = (live.config.width, live.config.height);
        let Some(frame) = self.session.surface_frame(w, h) else { return Ok(()) };
        let view = texture.texture.create_view(&wgpu::TextureViewDescriptor {
            format: Some(live.view_format),
            ..Default::default()
        });
        let target = DisplayTarget {
            view: &view,
            format: live.view_format,
            size: (w, h),
            placement: frame.placement,
            // theme::DARK's canvas, display-encoded.
            background: wgpu::Color { r: 0.04, g: 0.04, b: 0.043, a: 1.0 },
        };
        let out = self.rt.block_on(render_to_target(
            &frame.context,
            &frame.display,
            &frame.overview,
            target,
            frame.overview_dims,
            Default::default(),
            move || texture.present(),
        ))?;
        let fx = self.session.surface_frame_finished(&frame, out);
        self.stats.lock().unwrap_or_else(|e| e.into_inner()).frame(t.elapsed().as_secs_f32() * 1000.0);
        self.spawn(fx);
        Ok(())
    }
}
