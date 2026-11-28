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

/// How often to look for an overview on its way back when nothing else
/// wakes the thread.
const OVERVIEW_POLL: Duration = Duration::from_millis(2);

/// JPEG quality for exports: high, as a gallery copy should be (the
/// desktop's default, 80, is for sharing).
const EXPORT_QUALITY: u8 = 92;

use raw_editor_session::gpu::render_functions::{render_to_target, DisplayTarget};
use raw_editor_session::gpu::shared::{pick_surface_format, SharedContext, HDR_FORMAT};
use raw_editor_session::core::output_space::OutputSpace;
use raw_editor_session::session::{host, AppTab, CropHandle, EditorReadiness, ExportFormat, ExportJobState, LoadJob, SelectionChange, Session, SessionEvent};
use raw_editor_session::{Changed, Effects};

pub enum Command {
    Open(PathBuf),
    Surface(ndk::native_window::NativeWindow, u32, u32),
    /// The surface is going; answer once it is dropped.
    SurfaceGone(Sender<()>),
    /// Control `.0` (`controls::CONTROLS`) moved to `.1`.
    Edit(usize, f32),
    /// A gesture ended: an undo step, saved.
    Commit,
    /// Step back through the open photo's history.
    Undo,
    /// Step forward again.
    Redo,
    /// Every edit back to its default, as one undo step.
    ResetAll,
    /// The crop tool on or off: the whole frame shown, the part cut away
    /// dimmed, and [`Engine::crop_frame`] published for the handles. The
    /// frame sits inside `insets` (left, top, right, bottom, surface
    /// pixels), clear of the system's edge gestures.
    Cropping { on: bool, insets: [f32; 4] },
    /// A crop handle dragged from one point to another (surface pixels).
    CropDrag { handle: CropHandle, from: [f32; 2], to: [f32; 2] },
    /// The crop drag ended: an undo step, saved.
    CropEnd,
    /// A centred crop of this width/height ratio; 0 for the whole frame.
    CropAspect(f32),
    /// A quarter turn of the whole frame.
    Turn { clockwise: bool },
    /// A pinch or drag: scale the zoom about `at`, then move by `pan`
    /// (surface pixels).
    View { scale: f32, at: [f32; 2], pan: [f32; 2] },
    /// A double tap at a point: back to fit, or in to 3× about it.
    ToggleZoom([f32; 2]),
    /// Save the open photo, as edited, as a full-size JPEG
    /// ([`Engine::take_export`] hears how it went).
    Export,
    /// Draw in HDR when the surface can (`shared::HDR_FORMAT`), or not.
    Hdr(bool),
    /// How far above SDR white the display can go right now (Android's
    /// HDR/SDR ratio); HDR frames are drawn for it.
    Headroom(f32),
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

    /// How many photos have opened, and the last one's values for every
    /// control: the UI sets its sliders from them when the count moves.
    pub fn opened(&self) -> (u32, Vec<f32>) {
        let stats = self.stats.lock().unwrap_or_else(|e| e.into_inner());
        (stats.opened, stats.values.clone())
    }

    /// Moves whenever the edit changed under the sliders (a photo opened,
    /// undo, redo, reset): the UI then takes [`Engine::opened`]'s values.
    pub fn values_version(&self) -> u32 {
        self.stats.lock().unwrap_or_else(|e| e.into_inner()).values_version
    }

    /// While cropping: the whole image's rectangle on the surface (x, y,
    /// width, height, in pixels), then the crop within it (left, top, width,
    /// height, fractions of the image); empty otherwise.
    pub fn crop_frame(&self) -> Vec<f32> {
        self.stats.lock().unwrap_or_else(|e| e.into_inner()).crop_frame.map_or_else(Vec::new, |f| f.to_vec())
    }

    /// Whether Undo and Redo have a step to take.
    pub fn history(&self) -> (bool, bool) {
        self.stats.lock().unwrap_or_else(|e| e.into_inner()).history
    }

    /// Moves whenever the histogram does.
    pub fn histogram_version(&self) -> u32 {
        self.stats.lock().unwrap_or_else(|e| e.into_inner()).histogram_version
    }

    /// The histogram: red, green, blue, 256 counts each, then the count to
    /// scale them to (empty before the first frame).
    pub fn histogram(&self) -> Vec<u32> {
        self.stats.lock().unwrap_or_else(|e| e.into_inner()).histogram.clone()
    }

    /// True while the surface is drawn in HDR.
    pub fn hdr_active(&self) -> bool {
        self.stats.lock().unwrap_or_else(|e| e.into_inner()).hdr_active
    }

    /// The next finished export, once: the JPEG's path, or why there is
    /// none. Kotlin copies the file into the gallery, then deletes it.
    pub fn take_export(&self) -> Option<Result<PathBuf, String>> {
        self.stats.lock().unwrap_or_else(|e| e.into_inner()).exports.pop_front()
    }

    /// How control `index` reads at `value` for the open photo.
    pub fn show(&self, index: usize, value: f32) -> String {
        let as_shot = self.stats.lock().unwrap_or_else(|e| e.into_inner()).as_shot;
        crate::controls::CONTROLS.get(index).map(|c| (c.show)(value, as_shot)).unwrap_or_default()
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
    values: Vec<f32>,
    /// Moves whenever `values` does.
    values_version: u32,
    /// Undo, Redo: is there a step to take.
    history: (bool, bool),
    /// See [`Engine::crop_frame`].
    crop_frame: Option<[f32; 8]>,
    /// The open photo's as-shot white balance (kelvin, tint).
    as_shot: (f32, f32),
    /// Red, green, blue: 256 counts each, then the count to scale to
    /// (`core::histogram::display_peak`).
    histogram: Vec<u32>,
    /// Moves whenever `histogram` does.
    histogram_version: u32,
    /// Finished exports not yet collected.
    exports: VecDeque<Result<PathBuf, String>>,
    /// The surface is drawn in HDR.
    hdr_active: bool,
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

/// theme::DARK's canvas: display-encoded for an SDR surface, linear for HDR.
fn background(format: wgpu::TextureFormat) -> wgpu::Color {
    if format == HDR_FORMAT {
        // 0.04 sRGB-encoded, decoded.
        let l = 0.04 / 12.92;
        wgpu::Color { r: l, g: l, b: (0.043f64 / 12.92), a: 1.0 }
    } else {
        wgpu::Color { r: 0.04, g: 0.04, b: 0.043, a: 1.0 }
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
    /// What the surface offers, for switching between SDR and HDR.
    formats: Vec<wgpu::TextureFormat>,
}

/// The surface format to draw in: HDR when wanted and offered, else an SDR
/// one the colour pass can draw (`(surface format, view format)`).
fn choose_format(offered: &[wgpu::TextureFormat], hdr: bool) -> Option<(wgpu::TextureFormat, wgpu::TextureFormat)> {
    if hdr && offered.contains(&HDR_FORMAT) {
        return Some((HDR_FORMAT, HDR_FORMAT));
    }
    pick_surface_format(offered)
}

/// A photo on its way to the screen.
enum Opening {
    /// Its folder is being imported.
    Importing { path: PathBuf, since: Instant },
    /// Selected; its RAW is loading.
    Loading { id: i64, name: String, since: Instant },
}

/// What an inset crop view was fitted for: the frame's shape, the surface,
/// the insets.
type InsetFor = ((u32, u32), (f32, f32), [f32; 4]);

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
    /// An export is running (the session's `is_exporting`, as last seen).
    exporting: bool,
    /// HDR is wanted (the UI's switch).
    hdr: bool,
    /// The display's current HDR/SDR ratio.
    headroom: f32,
    /// Undo/Redo availability as last published.
    history: (bool, bool),
    /// The crop tool is wanted (the UI's Crop tab).
    cropping: bool,
    /// Where its frame must stay inside (see `Command::Cropping`).
    crop_insets: [f32; 4],
    /// What the inset view was last fitted for: the frame's shape, the
    /// surface, the insets. A turn or a new surface fits it again.
    inset_for: Option<InsetFor>,
    /// The crop frame as last published.
    crop_frame: Option<[f32; 8]>,
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
        // No detail loupe here: it would render on every edit for nothing.
        let _none = session.set_loupe_panel_open(false);
        Ok(Self { rt, session, context, files, live: None, stats, results, started: HashSet::new(), opening: None, exporting: false, hdr: false, headroom: 1.0, history: (false, false), cropping: false, crop_insets: [0.0; 4], inset_for: None, crop_frame: None })
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
                // While an overview is on its way back, look again shortly
                // even if nothing else happens: the last frame of a drag
                // still owes the histogram.
                let next = if self.session.surface_overview_pending() {
                    rx.recv_timeout(OVERVIEW_POLL)
                } else {
                    rx.recv().map_err(|_| mpsc::RecvTimeoutError::Disconnected)
                };
                match next {
                    Ok(c) => batch.push(c),
                    Err(mpsc::RecvTimeoutError::Timeout) => {}
                    Err(mpsc::RecvTimeoutError::Disconnected) => break,
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
            let fx = self.session.surface_overview_poll();
            self.spawn(fx);
            self.start_loads();
            self.check_opened();
            self.check_export();
            self.check_history();
            self.sync_cropping();
            self.check_crop_frame();
            let changed = self.session.changed.take();
            dirty |= changed.contains(Changed::FRAME);
            if changed.contains(Changed::HISTOGRAM) {
                let data = self.session.histogram_data.borrow();
                let mut histogram: Vec<u32> = data.iter().flatten().copied().collect();
                histogram.push(raw_editor_session::core::histogram::display_peak(&data));
                drop(data);
                let mut stats = self.stats.lock().unwrap_or_else(|e| e.into_inner());
                stats.histogram = histogram;
                stats.histogram_version = stats.histogram_version.wrapping_add(1);
            }
            self.session.camera_preview = None;
            if quit {
                break;
            }
            if dirty && self.can_draw() {
                self.draw()?;
                dirty = false;
            }
        }
        // Leaving: save the edit and the edited proxy (from the last frame's
        // overview, if it is still on its way).
        let fx = self.session.surface_overview_wait();
        host::run(&mut self.session, &self.rt, fx, |_, _| {})?;
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
            Command::Edit(index, value) => {
                if let Some(control) = crate::controls::CONTROLS.get(index) {
                    let value = value.clamp(control.min, control.max);
                    let fx = self.session.edit(|p| (control.set)(p, value));
                    self.spawn(fx);
                }
                Ok(false)
            }
            Command::Commit => {
                let fx = self.session.commit_edit();
                self.spawn(fx);
                Ok(false)
            }
            Command::Undo => {
                let fx = self.session.undo();
                self.history_moved(fx)
            }
            Command::Redo => {
                let fx = self.session.redo();
                self.history_moved(fx)
            }
            Command::ResetAll => {
                let fx = self.session.reset_edits();
                self.history_moved(fx)
            }
            Command::Cropping { on, insets } => {
                self.cropping = on;
                self.crop_insets = insets;
                self.sync_cropping();
                Ok(true)
            }
            Command::CropDrag { handle, from, to } => {
                if self.session.is_cropping {
                    self.session.drag_crop(handle, from, to);
                }
                Ok(true)
            }
            Command::CropEnd => {
                self.session.end_handle_drag();
                Ok(true)
            }
            Command::CropAspect(ratio) => {
                let crop = match &self.session.image_resources {
                    Some(res) if ratio > 0.0 => {
                        let (w, h) = res.oriented_dims(self.session.current_edit_params.quarter_turns);
                        Session::calculate_center_crop(ratio, w, h)
                    }
                    Some(_) => [0.0, 0.0, 1.0, 1.0],
                    None => return Ok(false),
                };
                let fx = self.session.edit_and_commit(|p| p.crop = crop);
                self.spawn(fx);
                Ok(true)
            }
            Command::Turn { clockwise } => {
                let fx = self.session.turn_image(clockwise);
                self.spawn(fx);
                // A turn returns to fit, even when the frame keeps its shape.
                self.inset_for = None;
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
            Command::Export => {
                self.export();
                Ok(false)
            }
            Command::Hdr(on) => {
                self.hdr = on;
                Ok(self.reconfigure())
            }
            Command::Headroom(h) => {
                let h = if h.is_finite() { h.max(1.0) } else { 1.0 };
                let changed = (h - self.headroom).abs() > 1e-3;
                if (h - self.headroom).abs() > 0.05 {
                    tracing::info!("HDR headroom {h:.2}");
                }
                self.headroom = h;
                Ok(changed && self.view_format() == Some(HDR_FORMAT))
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

    /// Export the open photo through the session's own export (as the
    /// desktop's, one image): full size, sRGB (Ultra HDR while HDR is on),
    /// into the app's private
    /// `exports` folder, from where Kotlin moves it into the gallery.
    fn export(&mut self) {
        if self.exporting {
            return;
        }
        if !matches!(self.session.editor_readiness, EditorReadiness::Ready(_)) {
            self.finish_export(Err("Open a photo first".into()));
            return;
        }
        let settings = &mut self.session.export_settings;
        settings.format = ExportFormat::Jpeg;
        settings.color_space = OutputSpace::Srgb;
        settings.quality = EXPORT_QUALITY;
        settings.resize = false;
        settings.base_path = self.files.join("exports");
        settings.subfolder = String::new();
        // With HDR on, the file carries what the screen shows: Ultra HDR,
        // which looks as before wherever there is no HDR.
        settings.ultra_hdr = self.hdr;
        match self.session.start_export() {
            Some(fx) if self.session.is_exporting => {
                self.exporting = true;
                self.status("Exporting…");
                self.spawn(fx);
            }
            _ => {
                let why = self.session.status.clone();
                self.finish_export(Err(why));
            }
        }
    }

    /// The export ended: hand the result to Kotlin.
    fn check_export(&mut self) {
        if !self.exporting || self.session.is_exporting {
            return;
        }
        self.exporting = false;
        let result = match self.session.export_jobs.first().map(|j| &j.state) {
            Some(ExportJobState::Done(path)) => Ok(path.clone()),
            Some(ExportJobState::Failed(e)) => Err(e.clone()),
            _ => Err(self.session.status.clone()),
        };
        self.finish_export(result);
    }

    fn finish_export(&mut self, result: Result<PathBuf, String>) {
        match &result {
            Ok(path) => tracing::info!("exported {}", path.display()),
            Err(e) => tracing::error!("export failed: {e}"),
        }
        self.status(match &result {
            Ok(_) => "Exported".to_string(),
            Err(e) => format!("Error: export failed: {e}"),
        });
        self.stats.lock().unwrap_or_else(|e| e.into_inner()).exports.push_back(result);
    }

    /// The selected photo is ready (or failed): say so, and hand its saved
    /// exposure to the UI's slider.
    /// Keep the session's crop tool as the UI wants it, once the photo is
    /// ready (opening a photo turns it off), its whole frame fitted inside
    /// the insets: again after a turn (which returns to fit) or a new surface.
    fn sync_cropping(&mut self) {
        if !matches!(self.session.editor_readiness, EditorReadiness::Ready(_)) {
            return;
        }
        if self.session.is_cropping != self.cropping {
            let fx = self.session.toggle_crop();
            self.spawn(fx);
            self.inset_for = None;
            if !self.cropping {
                // Back to fit: the inset view is the crop tool's alone.
                let fx = self.session.reset_view();
                self.spawn(fx);
            }
        }
        if self.session.is_cropping {
            let key = (self.session.image_display_dims(), self.session.viewport_size, self.crop_insets);
            if self.inset_for != Some(key) {
                self.inset_for = Some(key);
                self.session.inset_view(self.crop_insets);
            }
        }
    }

    /// The crop frame for the UI's handles, when it moved.
    fn check_crop_frame(&mut self) {
        let frame = self.session.is_cropping.then(|| {
            let (dw, dh) = self.session.image_display_dims();
            let (vw, vh) = self.session.viewport_size;
            let r = raw_editor_session::core::viewport::image_rect(dw, dh, vw, vh, self.session.zoom, self.session.pan_offset);
            let c = self.session.current_edit_params.crop;
            [r.x, r.y, r.width, r.height, c[0], c[1], c[2], c[3]]
        });
        if frame != self.crop_frame {
            self.crop_frame = frame;
            self.stats.lock().unwrap_or_else(|e| e.into_inner()).crop_frame = frame;
        }
    }

    /// Undo, redo or reset changed the edit: render it, and move the
    /// sliders to it.
    fn history_moved(&mut self, fx: Effects<SessionEvent>) -> Result<bool, String> {
        self.spawn(fx);
        self.publish_values();
        Ok(false)
    }

    /// The sliders' values, after the edit changed under them.
    fn publish_values(&mut self) {
        let mut stats = self.stats.lock().unwrap_or_else(|e| e.into_inner());
        stats.values = crate::controls::values(&self.session.current_edit_params);
        stats.values_version = stats.values_version.wrapping_add(1);
    }

    /// Undo/Redo availability, for the buttons, when it changed.
    fn check_history(&mut self) {
        let steps = self.session.history_steps();
        if steps != self.history {
            self.history = steps;
            self.stats.lock().unwrap_or_else(|e| e.into_inner()).history = steps;
        }
    }

    fn check_opened(&mut self) {
        let Some(Opening::Loading { id, name, since }) = &self.opening else { return };
        let ms = since.elapsed().as_millis();
        match &self.session.editor_readiness {
            EditorReadiness::Ready(r) if r == id => {
                tracing::info!("opened {name} in {ms} ms");
                let mut stats = self.stats.lock().unwrap_or_else(|e| e.into_inner());
                stats.status = format!("{name}, opened in {ms} ms");
                stats.values = crate::controls::values(&self.session.current_edit_params);
                stats.values_version = stats.values_version.wrapping_add(1);
                stats.as_shot = self.session.as_shot_wb.unwrap_or((5000.0, 0.0));
                stats.opened += 1;
            }
            EditorReadiness::Failed(f, e) if f == id => self.status(format!("Error: {name} did not load: {e}")),
            _ => return,
        }
        self.opening = None;
    }

    fn view_format(&self) -> Option<wgpu::TextureFormat> {
        self.live.as_ref().map(|l| l.view_format)
    }

    /// Switch the live surface between SDR and HDR after the switch moved;
    /// true when it changed (and needs a frame).
    fn reconfigure(&mut self) -> bool {
        let Some(live) = &mut self.live else { return false };
        let Some((format, view_format)) = choose_format(&live.formats, self.hdr) else { return false };
        if format == live.config.format && view_format == live.view_format {
            return false;
        }
        live.config.format = format;
        live.config.view_formats = if view_format != format { vec![view_format] } else { vec![] };
        live.view_format = view_format;
        live.surface.configure(&self.context.device, &live.config);
        tracing::info!("surface now {format:?} drawn as {view_format:?}");
        self.stats.lock().unwrap_or_else(|e| e.into_inner()).hdr_active = view_format == HDR_FORMAT;
        true
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
        // Rgba16Float here is Android's HDR surface (extended linear sRGB).
        tracing::info!("surface formats offered: {:?}", caps.formats);
        let (format, view_format) = choose_format(&caps.formats, self.hdr)
            .ok_or_else(|| format!("no usable surface format in {:?}", caps.formats))?;
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
        self.stats.lock().unwrap_or_else(|e| e.into_inner()).hdr_active = view_format == HDR_FORMAT;
        self.live = Some(Live { _window: window, surface, config, view_format, formats: caps.formats });
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
            background: background(live.view_format),
            headroom: self.headroom,
        };
        let out = render_to_target(
            &frame.context,
            &frame.display,
            frame.overview.as_deref(),
            target,
            frame.overview_dims,
            Default::default(),
            move || texture.present(),
        )?;
        let fx = self.session.surface_frame_finished(&frame, out);
        self.stats.lock().unwrap_or_else(|e| e.into_inner()).frame(t.elapsed().as_secs_f32() * 1000.0);
        self.spawn(fx);
        Ok(())
    }
}
