//! Lens corrections: geometric distortion, lateral chromatic aberration (TCA)
//! and vignetting — from a lensfun calibration database when one is installed,
//! plus manual sliders that work without one.
//!
//! ## Profiles
//! The lensfun database (CC-BY-SA, shipped by darktable/RawTherapee/the
//! `lensfun` package) is read at runtime from the user's own folder
//! (`paths::lensfun_dir`), the copy the app downloads on request
//! (`core::lensfun`, into `paths::lensfun_updates_dir`), and the usual system
//! install locations. Nothing is bundled: without it only the manual
//! controls are available.
//!
//! ## Coordinates
//! lensfun stores coefficients in Hugin's convention: radius 1 = half the
//! SHORTER side of the calibration sensor (sized from its crop factor and
//! aspect ratio) for distortion and TCA, and half the DIAGONAL for
//! vignetting. Converting this image's pixels into those units needs this
//! camera's own crop factor (pixel → millimetre on its sensor). All of that is
//! folded into one `vec2` scale on the CPU so the shader works in calibration
//! units directly.
//!
//! The shader maps each OUTPUT (corrected) position to the SOURCE position it
//! comes from — exactly the direction lensfun's models are written in:
//! `r_d = r_u · f(r_u)` for geometry, then per-channel TCA on the distorted
//! coordinate.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

/// The user's lens settings. Lives in `EditParams`.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq)]
#[serde(default)]
pub struct LensParams {
    /// Apply the matched profile's distortion and vignetting.
    pub profile: bool,
    /// Apply the profile's lateral CA correction.
    pub profile_ca: bool,
    /// Scale of the profile's distortion correction (0..2, 1 = as calibrated).
    pub profile_distortion: f32,
    /// Scale of the profile's vignetting correction (0..2).
    pub profile_vignetting: f32,
    /// Manual distortion (-1..1): positive corrects barrel, negative pincushion.
    pub distortion: f32,
    /// Manual vignetting (-1..1): positive brightens the corners.
    pub vignetting: f32,
    /// Manual lateral CA, red/cyan and blue/yellow (-1..1).
    pub ca_red: f32,
    pub ca_blue: f32,
    /// Scale the corrected image so no empty (black) border shows.
    pub constrain: bool,
}

impl Default for LensParams {
    fn default() -> Self {
        Self {
            profile: false,
            profile_ca: false,
            profile_distortion: 1.0,
            profile_vignetting: 1.0,
            distortion: 0.0,
            vignetting: 0.0,
            ca_red: 0.0,
            ca_blue: 0.0,
            constrain: true,
        }
    }
}

/// Lightroom-style defringing of axial (longitudinal) chromatic aberration:
/// purple and green fringes beside high-contrast edges, which no radial lens
/// model can remove. Hues are HSV hue angles of the linear-sRGB colour.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq)]
#[serde(default)]
pub struct DefringeParams {
    /// 0..1
    pub purple_amount: f32,
    /// Centre of the purple band, degrees.
    pub purple_hue: f32,
    pub green_amount: f32,
    pub green_hue: f32,
}

impl Default for DefringeParams {
    fn default() -> Self {
        Self { purple_amount: 0.0, purple_hue: 285.0, green_amount: 0.0, green_hue: 110.0 }
    }
}

impl DefringeParams {
    pub fn is_active(&self) -> bool {
        self.purple_amount > 0.0 || self.green_amount > 0.0
    }
}

/// One lens's calibration data as read from the database.
#[derive(Debug, Clone, PartialEq)]
pub struct LensCalibration {
    pub maker: String,
    pub model: String,
    pub crop_factor: f32,
    pub aspect_ratio: f32,
    /// focal → `r_d / r_u` polynomial `p0 + p1 r + p2 r² + p3 r³ + p4 r⁴`.
    pub distortion: Vec<(f32, [f32; 5])>,
    /// focal → (vr, cr, br, vb, cb, bb): `r_d,c = r · (v + c·r + b·r²)`.
    pub tca: Vec<(f32, [f32; 6])>,
    /// (focal, aperture, distance) → PA terms k1..k3.
    pub vignetting: Vec<(f32, f32, f32, [f32; 3])>,
}

/// A camera's sensor crop factor, for images whose EXIF lacks a 35 mm
/// equivalent focal length.
#[derive(Debug, Clone, PartialEq)]
pub struct CameraEntry {
    pub maker: String,
    pub model: String,
    pub crop_factor: f32,
}

/// The process-wide database. `None` until first needed; replaced by
/// [`LensDb::reload`] after a download or when the user rescans.
static DB: RwLock<Option<Arc<LensDb>>> = RwLock::new(None);

#[derive(Debug, Default)]
pub struct LensDb {
    pub lenses: Vec<LensCalibration>,
    pub cameras: Vec<CameraEntry>,
    /// Every folder that contributed, in search order.
    pub sources: Vec<DbSource>,
}

/// One folder the database was read from.
#[derive(Debug, Clone, PartialEq)]
pub struct DbSource {
    pub dir: PathBuf,
    pub files: usize,
    /// lensfun's `timestamp.txt` (seconds since the epoch), written by its
    /// own updater and by `core::lensfun`; absent for packaged copies.
    pub timestamp: Option<i64>,
}

/// Profile coefficients interpolated to one shot's focal length and aperture.
#[derive(Debug, Clone, PartialEq)]
pub struct ProfileCorrection {
    pub lens_name: String,
    pub crop_factor: f32,
    pub aspect_ratio: f32,
    pub distortion: Option<[f32; 5]>,
    pub tca: Option<[f32; 6]>,
    pub vignetting: Option<[f32; 3]>,
}

/// Everything about this image's optics the renderer needs, resolved at load.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct LensContext {
    /// Human-readable lens name (from the makernotes or EXIF), if known.
    pub lens_name: Option<String>,
    /// Matched profile, if the database is installed and has this lens.
    pub profile: Option<ProfileCorrection>,
    /// This camera's crop factor; `None` = unknown (the profile's is assumed).
    pub camera_crop: Option<f32>,
    pub focal: Option<f32>,
    pub aperture: Option<f32>,
}

// ─────────────────────────────── database ─────────────────────────────────

/// Where lensfun databases live, in priority order.
pub fn search_dirs() -> Vec<PathBuf> {
    let mut dirs = vec![crate::core::paths::lensfun_dir(), crate::core::paths::lensfun_updates_dir()];
    if let Some(data) = dirs::data_dir() {
        dirs.push(data.join("lensfun").join("updates").join("version_1"));
        dirs.push(data.join("lensfun").join("version_1"));
    }
    for base in ["/usr/share/lensfun", "/usr/local/share/lensfun", "/var/lib/lensfun-updates"] {
        dirs.push(Path::new(base).join("version_1"));
        dirs.push(Path::new(base).to_path_buf());
    }
    dirs
}

impl LensDb {
    /// The process-wide database, loaded on first use. Reads files: call it
    /// only off the UI thread (the RAW loader does, inside `spawn_blocking`).
    pub fn global() -> Arc<LensDb> {
        if let Some(db) = Self::loaded() {
            return db;
        }
        Self::reload()
    }

    /// Read the search folders again and make the result the process-wide
    /// database — after a download, or when the user has added files.
    /// Blocking, like [`global`](Self::global).
    pub fn reload() -> Arc<LensDb> {
        crate::core::lensfun::restore_interrupted(&crate::core::paths::lensfun_updates_dir());
        let db = Arc::new(LensDb::load(&search_dirs()));
        tracing::info!("lens database: {} lenses, {} cameras", db.lenses.len(), db.cameras.len());
        *DB.write().unwrap_or_else(|e| e.into_inner()) = Some(db.clone());
        db
    }

    /// The database if it has been loaded already — never reads a file, so
    /// the UI thread can ask.
    pub fn loaded() -> Option<Arc<LensDb>> {
        DB.read().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// Lenses in the database, if it has been loaded already (see `loaded`).
    pub fn loaded_count() -> Option<usize> {
        Self::loaded().map(|db| db.lenses.len())
    }

    /// Load every `*.xml` in the first directory that has any; later
    /// directories only add lenses the earlier ones lack (an `updates` copy
    /// shadows the system one, as lensfun itself does).
    pub fn load(dirs: &[PathBuf]) -> Self {
        let mut db = LensDb::default();
        let mut seen: std::collections::HashSet<(String, String)> = std::collections::HashSet::new();
        for dir in dirs {
            let Ok(entries) = std::fs::read_dir(dir) else { continue };
            let mut files: Vec<PathBuf> = entries
                .filter_map(|e| e.ok().map(|e| e.path()))
                .filter(|p| p.extension().is_some_and(|x| x == "xml"))
                .collect();
            files.sort();
            if !files.is_empty() {
                let timestamp = std::fs::read_to_string(dir.join("timestamp.txt"))
                    .ok()
                    .and_then(|t| t.trim().parse().ok());
                db.sources.push(DbSource { dir: dir.clone(), files: files.len(), timestamp });
            }
            for f in files {
                match std::fs::read_to_string(&f) {
                    Ok(xml) => {
                        let (lenses, cameras) = parse(&xml);
                        for l in lenses {
                            if seen.insert((l.maker.clone(), l.model.clone())) {
                                db.lenses.push(l);
                            }
                        }
                        db.cameras.extend(cameras);
                    }
                    Err(e) => tracing::warn!("lens db {}: {e}", f.display()),
                }
            }
        }
        db
    }

    /// Best database entry for `maker` + `model`. Every number in the name
    /// (focal range, aperture) must agree exactly — "17-50mm f/2.8" must
    /// never match "17-70mm f/2.8-4" — and the remaining words are scored by
    /// overlap.
    pub fn find(&self, maker: &str, model: &str) -> Option<&LensCalibration> {
        let mut query = tokens(model);
        query.extend(tokens(maker));
        let maker_tokens = maker_words(maker);
        let (q_num, q_words) = split_numeric(&query);
        if q_num.is_empty() {
            return None;
        }
        self.lenses
            .iter()
            .filter_map(|l| {
                let mut cand = tokens(&l.model);
                cand.extend(tokens(&l.maker));
                let (c_num, c_words) = split_numeric(&cand);
                if c_num != q_num {
                    return None;
                }
                // A known maker must agree (Sigma's 17-50 is not Nikon's).
                let lm = maker_words(&l.maker);
                if !maker_tokens.is_empty() && !lm.is_empty() && !lm.iter().any(|t| maker_tokens.contains(t)) {
                    return None;
                }
                let inter = q_words.intersection(&c_words).count() as f32;
                let union = q_words.union(&c_words).count().max(1) as f32;
                Some((inter / union, l))
            })
            .filter(|(score, _)| *score >= 0.5)
            .max_by(|a, b| a.0.total_cmp(&b.0))
            .map(|(_, l)| l)
    }

    pub fn camera_crop(&self, maker: &str, model: &str) -> Option<f32> {
        let q = tokens(model);
        let qm = tokens(maker);
        self.cameras
            .iter()
            .find(|c| {
                let t = tokens(&c.model);
                let mk = tokens(&c.maker);
                // Model tokens minus maker words ("Nikon D3300" → {d, 3300}).
                let tm: std::collections::BTreeSet<String> =
                    t.difference(&mk).cloned().collect();
                let qmm: std::collections::BTreeSet<String> =
                    q.difference(&qm).cloned().collect();
                !tm.is_empty() && tm == qmm && mk.iter().any(|w| qm.contains(w))
            })
            .map(|c| c.crop_factor)
    }
}

/// Brand names a lens maker is also known by, as a maker word → the word
/// lensfun files it under. Panasonic's lenses report "Lumix" (rawler's lens
/// table, which is where RW2 lens names come from); Rokinon sells Samyang's
/// lenses and Laowa is Venus Optics'; HD Pentax lenses are Ricoh's now, and
/// OM Digital Solutions makes Olympus's.
const MAKER_ALIASES: &[(&str, &str)] =
    &[("lumix", "panasonic"), ("rokinon", "samyang"), ("laowa", "venus"), ("ricoh", "pentax"), ("om", "olympus")];

/// A maker's words for comparing makers, aliases included.
fn maker_words(maker: &str) -> TokenSet {
    let mut words = tokens(maker);
    for (alias, maker) in MAKER_ALIASES {
        if words.contains(*alias) {
            words.insert(maker.to_string());
        }
    }
    words
}

/// Latin letters with diacritics → their base letter, so "Voigtländer"
/// (lensfun) and "Voigtlander" (EXIF) are one word.
fn fold(c: char) -> char {
    match c {
        'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' => 'a',
        'ç' => 'c',
        'è' | 'é' | 'ê' | 'ë' => 'e',
        'ì' | 'í' | 'î' | 'ï' => 'i',
        'ñ' => 'n',
        'ò' | 'ó' | 'ô' | 'õ' | 'ö' | 'ø' => 'o',
        'ù' | 'ú' | 'û' | 'ü' => 'u',
        'ý' | 'ÿ' => 'y',
        _ => c,
    }
}

/// Lower-cased alphanumeric runs, split at letter/digit boundaries, with
/// decimals kept whole and diacritics dropped: "AF-S 18-55mm f/3.5-5.6G" →
/// af s 18 55 mm f 3.5 5.6 g.
pub(crate) fn tokens(s: &str) -> std::collections::BTreeSet<String> {
    let mut out = std::collections::BTreeSet::new();
    let mut cur = String::new();
    let mut cur_digit = false;
    let chars: Vec<char> = s.to_lowercase().chars().map(fold).collect();
    for (i, &c) in chars.iter().enumerate() {
        let is_decimal_point =
            c == '.' && cur_digit && chars.get(i + 1).is_some_and(|n| n.is_ascii_digit());
        if c.is_alphanumeric() || is_decimal_point {
            let d = c.is_ascii_digit() || c == '.';
            if !cur.is_empty() && d != cur_digit {
                out.insert(std::mem::take(&mut cur));
            }
            cur_digit = d;
            cur.push(c);
        } else if !cur.is_empty() {
            out.insert(std::mem::take(&mut cur));
        }
    }
    if !cur.is_empty() {
        out.insert(cur);
    }
    out
}

type TokenSet = std::collections::BTreeSet<String>;

fn split_numeric(t: &TokenSet) -> (TokenSet, TokenSet) {
    t.iter().cloned().partition(|w| w.chars().next().is_some_and(|c| c.is_ascii_digit()))
}

fn attr(node: roxmltree::Node, name: &str) -> Option<f32> {
    node.attribute(name).and_then(|v| v.trim().parse().ok())
}

fn parse_aspect(s: &str) -> Option<f32> {
    let s = s.trim();
    if let Some((a, b)) = s.split_once(':') {
        let (a, b): (f32, f32) = (a.trim().parse().ok()?, b.trim().parse().ok()?);
        return (b > 0.0).then(|| a.max(b) / a.min(b));
    }
    s.parse::<f32>().ok().filter(|v| *v > 0.0).map(|v| v.max(1.0 / v))
}

/// Parse one lensfun XML document. Malformed entries are skipped, never fatal.
pub fn parse(xml: &str) -> (Vec<LensCalibration>, Vec<CameraEntry>) {
    // lensfun's files open with a <!DOCTYPE>, which roxmltree refuses by
    // default; it is only a reference to the schema, so allow it.
    let opts = roxmltree::ParsingOptions { allow_dtd: true, ..Default::default() };
    let doc = match roxmltree::Document::parse_with_options(xml, opts) {
        Ok(d) => d,
        Err(e) => {
            tracing::warn!("lens db: unparseable XML: {e}");
            return (Vec::new(), Vec::new());
        }
    };
    // Prefer the un-localised text: <model> without a lang attribute.
    let child_text = |node: roxmltree::Node, tag: &str| -> Option<String> {
        let mut fallback = None;
        for c in node.children().filter(|c| c.has_tag_name(tag)) {
            let t = c.text().map(|t| t.trim().to_string()).filter(|t| !t.is_empty());
            if c.attribute("lang").is_none() {
                return t;
            }
            if fallback.is_none() {
                fallback = t;
            }
        }
        fallback
    };

    let mut lenses = Vec::new();
    let mut cameras = Vec::new();
    for node in doc.root_element().children().filter(|n| n.is_element()) {
        if node.has_tag_name("camera") {
            if let (Some(maker), Some(model), Some(crop)) = (
                child_text(node, "maker"),
                child_text(node, "model"),
                child_text(node, "cropfactor").and_then(|c| c.parse().ok()),
            ) {
                cameras.push(CameraEntry { maker, model, crop_factor: crop });
            }
            continue;
        }
        if !node.has_tag_name("lens") {
            continue;
        }
        let (Some(maker), Some(model)) = (child_text(node, "maker"), child_text(node, "model")) else {
            continue;
        };
        let mut lens = LensCalibration {
            maker,
            model,
            crop_factor: child_text(node, "cropfactor").and_then(|c| c.parse().ok()).unwrap_or(1.0),
            aspect_ratio: child_text(node, "aspect-ratio").and_then(|a| parse_aspect(&a)).unwrap_or(1.5),
            distortion: Vec::new(),
            tca: Vec::new(),
            vignetting: Vec::new(),
        };
        for cal in node.children().filter(|c| c.has_tag_name("calibration")) {
            for e in cal.children().filter(|c| c.is_element()) {
                let Some(focal) = attr(e, "focal") else { continue };
                match (e.tag_name().name(), e.attribute("model").unwrap_or("")) {
                    ("distortion", "ptlens") => {
                        let (a, b, c) = (
                            attr(e, "a").unwrap_or(0.0),
                            attr(e, "b").unwrap_or(0.0),
                            attr(e, "c").unwrap_or(0.0),
                        );
                        lens.distortion.push((focal, [1.0 - a - b - c, c, b, a, 0.0]));
                    }
                    ("distortion", "poly3") => {
                        let k1 = attr(e, "k1").unwrap_or(0.0);
                        lens.distortion.push((focal, [1.0 - k1, 0.0, k1, 0.0, 0.0]));
                    }
                    ("distortion", "poly5") => {
                        let (k1, k2) = (attr(e, "k1").unwrap_or(0.0), attr(e, "k2").unwrap_or(0.0));
                        lens.distortion.push((focal, [1.0, 0.0, k1, 0.0, k2]));
                    }
                    ("tca", "linear") => {
                        let (kr, kb) = (attr(e, "kr").unwrap_or(1.0), attr(e, "kb").unwrap_or(1.0));
                        lens.tca.push((focal, [kr, 0.0, 0.0, kb, 0.0, 0.0]));
                    }
                    ("tca", "poly3") => lens.tca.push((
                        focal,
                        [
                            attr(e, "vr").unwrap_or(1.0),
                            attr(e, "cr").unwrap_or(0.0),
                            attr(e, "br").unwrap_or(0.0),
                            attr(e, "vb").unwrap_or(1.0),
                            attr(e, "cb").unwrap_or(0.0),
                            attr(e, "bb").unwrap_or(0.0),
                        ],
                    )),
                    ("vignetting", "pa") => {
                        let (Some(ap), dist) = (attr(e, "aperture"), attr(e, "distance").unwrap_or(1000.0)) else {
                            continue;
                        };
                        lens.vignetting.push((
                            focal,
                            ap,
                            dist,
                            [attr(e, "k1").unwrap_or(0.0), attr(e, "k2").unwrap_or(0.0), attr(e, "k3").unwrap_or(0.0)],
                        ));
                    }
                    _ => {}
                }
            }
        }
        lens.distortion.sort_by(|a, b| a.0.total_cmp(&b.0));
        lens.tca.sort_by(|a, b| a.0.total_cmp(&b.0));
        lenses.push(lens);
    }
    (lenses, cameras)
}

// ───────────────────────────── interpolation ──────────────────────────────

/// Linear interpolation of a coefficient vector along focal length, clamped
/// to the calibrated range.
fn interp_focal<const N: usize>(table: &[(f32, [f32; N])], focal: f32) -> Option<[f32; N]> {
    let first = table.first()?;
    let last = table.last()?;
    if focal <= first.0 {
        return Some(first.1);
    }
    if focal >= last.0 {
        return Some(last.1);
    }
    let i = table.iter().rposition(|e| e.0 <= focal)?;
    let (a, b) = (&table[i], &table[(i + 1).min(table.len() - 1)]);
    let t = if b.0 > a.0 { (focal - a.0) / (b.0 - a.0) } else { 0.0 };
    Some(std::array::from_fn(|k| a.1[k] + (b.1[k] - a.1[k]) * t))
}

impl LensCalibration {
    /// Interpolate to `focal` (mm) and `aperture` (f-number).
    pub fn interpolate(&self, focal: f32, aperture: f32) -> ProfileCorrection {
        // Vignetting: per calibrated focal, interpolate along aperture in
        // stops (log2 of f-number — vignetting changes per stop, not per
        // f-number), using the farthest focus distance; then along focal.
        let mut by_focal: Vec<(f32, [f32; 3])> = Vec::new();
        let mut focals: Vec<f32> = self.vignetting.iter().map(|v| v.0).collect();
        focals.sort_by(|a, b| a.total_cmp(b));
        focals.dedup();
        for f in focals {
            let max_dist = self
                .vignetting
                .iter()
                .filter(|v| v.0 == f)
                .map(|v| v.2)
                .fold(0.0f32, f32::max);
            let mut by_ap: Vec<(f32, [f32; 3])> = self
                .vignetting
                .iter()
                .filter(|v| v.0 == f && v.2 == max_dist)
                .map(|v| (v.1.max(0.5).log2(), v.3))
                .collect();
            by_ap.sort_by(|a, b| a.0.total_cmp(&b.0));
            if let Some(k) = interp_focal(&by_ap, aperture.max(0.5).log2()) {
                by_focal.push((f, k));
            }
        }
        // lensfun model names usually already start with the maker.
        let lens_name = if self.model.to_lowercase().starts_with(&self.maker.to_lowercase()) {
            self.model.clone()
        } else {
            format!("{} {}", self.maker, self.model)
        };
        ProfileCorrection {
            lens_name,
            crop_factor: self.crop_factor,
            aspect_ratio: self.aspect_ratio,
            distortion: interp_focal(&self.distortion, focal),
            tca: interp_focal(&self.tca, focal),
            vignetting: interp_focal(&by_focal, focal),
        }
    }
}

/// Resolve this shot's optics. Reads the lens database on first use, so call
/// it from a blocking context (the RAW loader).
pub fn resolve_context(
    camera_make: &str,
    camera_model: &str,
    lens_make: Option<&str>,
    lens_model: Option<&str>,
    focal: Option<f32>,
    focal_35mm: Option<f32>,
    aperture: Option<f32>,
) -> LensContext {
    let db = LensDb::global();
    // The camera database first: EXIF's 35 mm equivalent is rounded to whole
    // millimetres, which at 17 mm puts the crop factor ~3% off.
    let camera_crop = db.camera_crop(camera_make, camera_model).or(match (focal, focal_35mm) {
        (Some(f), Some(f35)) if f > 0.0 && f35 > 0.0 => Some(f35 / f),
        _ => None,
    });
    let lens_name = lens_model.map(|m| match lens_make {
        Some(mk) if !m.to_lowercase().starts_with(&mk.to_lowercase()) => format!("{mk} {m}"),
        _ => m.to_string(),
    });
    let profile = match (lens_model, focal) {
        (Some(model), Some(f)) => db
            .find(lens_make.unwrap_or(""), model)
            .map(|cal| cal.interpolate(f, aperture.unwrap_or(5.6))),
        _ => None,
    };
    if let Some(p) = &profile {
        tracing::info!("lens profile: {} @ {:?}mm f/{:?}", p.lens_name, focal, aperture);
    } else if lens_name.is_some() {
        tracing::info!("no lens profile for {:?}", lens_name);
    }
    LensContext { lens_name, profile, camera_crop, focal, aperture }
}

// ──────────────────────────────── shader ──────────────────────────────────

/// The resolved correction the shader evaluates. All radii are in the
/// profile's distortion units (half the calibration sensor's shorter side).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GpuLens {
    pub active: bool,
    /// Sensor UV offset from centre → calibration units. Per axis because
    /// UV is not isotropic.
    pub scale: [f32; 2],
    /// `r_d / r_u = p0 + p1 r + p2 r² + p3 r³ + p4 r⁴`, with the constrain
    /// zoom folded in (see `gpu_params`).
    pub dist: [f32; 5],
    /// Red and blue: `r_c / r_d = v + c r + b r²`.
    pub tca_r: [f32; 3],
    pub tca_b: [f32; 3],
    /// PA vignetting k1..k3, and the factor taking distortion radius to
    /// vignetting radius (half-diagonal normalised).
    pub vig: [f32; 3],
    pub vig_ratio: f32,
    /// Manual vignetting amount and 1/(output corner radius).
    pub manual_vig: f32,
    pub inv_corner: f32,
}

impl Default for GpuLens {
    fn default() -> Self {
        Self {
            active: false,
            scale: [1.0, 1.0],
            dist: [1.0, 0.0, 0.0, 0.0, 0.0],
            tca_r: [1.0, 0.0, 0.0],
            tca_b: [1.0, 0.0, 0.0],
            vig: [0.0; 3],
            vig_ratio: 1.0,
            manual_vig: 0.0,
            inv_corner: 1.0,
        }
    }
}

const FULL_FRAME_DIAG_MM: f32 = 43.266_615; // hypot(36, 24)
/// Manual distortion at ±1 moves the frame corner by this fraction.
const MANUAL_DISTORTION: f32 = 0.12;
/// Manual CA at ±1 scales the channel by this much (≈ ±11 px at a 24 MP corner).
const MANUAL_CA: f32 = 0.003;

fn poly(p: &[f32; 5], r: f32) -> f32 {
    p[0] + r * (p[1] + r * (p[2] + r * (p[3] + r * p[4])))
}

/// Build the shader parameters for an image `sensor_w × sensor_h` pixels
/// (landscape sensor orientation, full resolution).
pub fn gpu_params(ctx: &LensContext, p: &LensParams, sensor_w: u32, sensor_h: u32) -> GpuLens {
    let (w, h) = (sensor_w.max(1) as f32, sensor_h.max(1) as f32);
    let profile = ctx.profile.as_ref();

    // Calibration geometry: lensfun's normalisation radii, in mm.
    let calib_crop = profile.map(|p| p.crop_factor).unwrap_or(1.5).max(0.1);
    let calib_aspect = profile.map(|p| p.aspect_ratio).unwrap_or(1.5);
    let hugin_mm = FULL_FRAME_DIAG_MM / calib_crop / calib_aspect.hypot(1.0) / 2.0;
    let vig_mm = FULL_FRAME_DIAG_MM / calib_crop / 2.0;

    // This sensor: pixels → mm.
    let crop = ctx.camera_crop.unwrap_or(calib_crop).max(0.1);
    let mm_per_px = (FULL_FRAME_DIAG_MM / crop) / w.hypot(h);
    // Sensor UV offset → calibration units.
    let base = [w * mm_per_px / hugin_mm, h * mm_per_px / hugin_mm];
    // Output frame corner, in calibration units.
    let corner = 0.5 * base[0].hypot(base[1]);

    let mut dist = [1.0, 0.0, 0.0, 0.0, 0.0];
    let mut tca_r = [1.0, 0.0, 0.0];
    let mut tca_b = [1.0, 0.0, 0.0];
    let mut vig = [0.0f32; 3];
    if let Some(pr) = profile {
        if p.profile {
            if let Some(d) = pr.distortion {
                // Scale the correction: interpolate the polynomial towards
                // identity (or past the profile, up to 2×).
                for k in 0..5 {
                    let id = if k == 0 { 1.0 } else { 0.0 };
                    dist[k] = id + (d[k] - id) * p.profile_distortion.clamp(0.0, 2.0);
                }
            }
            if let Some(v) = pr.vignetting {
                for k in 0..3 {
                    vig[k] = v[k] * p.profile_vignetting.clamp(0.0, 2.0);
                }
            }
        }
        if p.profile_ca {
            if let Some(t) = pr.tca {
                tca_r = [t[0], t[1], t[2]];
                tca_b = [t[3], t[4], t[5]];
            }
        }
    }
    // Manual distortion: a quadratic term sized so ±1 moves the corner by
    // MANUAL_DISTORTION. Barrel distortion has squeezed the edges inwards, so
    // undoing it means each output pixel near the edge samples from CLOSER to
    // the centre (f < 1): positive slider → negative term.
    dist[2] -= p.distortion.clamp(-1.0, 1.0) * MANUAL_DISTORTION / (corner * corner);
    tca_r[0] *= 1.0 + p.ca_red.clamp(-1.0, 1.0) * MANUAL_CA;
    tca_b[0] *= 1.0 + p.ca_blue.clamp(-1.0, 1.0) * MANUAL_CA;

    let geometric = dist != [1.0, 0.0, 0.0, 0.0, 0.0];
    let zoom = if p.constrain && geometric { constrain_scale(&dist, base) } else { 1.0 };
    // The zoom goes into the polynomial, not the UV scale. Zooming the
    // output by `s` means sampling `(p/s)·f(|p|/s)`; the shader evaluates
    // `p·f'(|p|)` and converts back to UV with the same scale it came in
    // by, so a zoom folded into that scale cancels on the way out (which is
    // how Constrain once did nothing). Folding it in here instead:
    // f'(r) = f(r/s)/s, i.e. d'ₖ = dₖ / s^(k+1).
    let dist = {
        let mut zoomed = dist;
        let mut s_pow = zoom;
        for d in &mut zoomed {
            *d /= s_pow;
            s_pow *= zoom;
        }
        zoomed
    };

    let active = geometric
        || tca_r != [1.0, 0.0, 0.0]
        || tca_b != [1.0, 0.0, 0.0]
        || vig != [0.0; 3]
        || p.vignetting != 0.0;

    GpuLens {
        active,
        scale: base,
        dist,
        tca_r,
        tca_b,
        vig,
        vig_ratio: hugin_mm / vig_mm,
        manual_vig: p.vignetting.clamp(-1.0, 1.0),
        inv_corner: 1.0 / corner,
    }
}

/// The zoom at which the corrected frame exactly fills the source with no
/// empty border — >1 for pincushion corrections (which pull the edges in),
/// <1 for barrel ones (which can reveal more of the frame). Found by
/// bisection over points along the output border.
fn constrain_scale(dist: &[f32; 5], base: [f32; 2]) -> f32 {
    let (hx, hy) = (0.5 * base[0], 0.5 * base[1]);
    let border: Vec<(f32, f32)> = (0..=32)
        .flat_map(|i| {
            let t = i as f32 / 32.0;
            let x = -hx + 2.0 * hx * t;
            let y = -hy + 2.0 * hy * t;
            [(x, -hy), (x, hy), (-hx, y), (hx, y)]
        })
        .collect();
    let fits = |s: f32| {
        border.iter().all(|&(x, y)| {
            let (px, py) = (x / s, y / s);
            let f = poly(dist, px.hypot(py));
            // A hair inside rather than on the edge: a sample exactly at
            // it is one float error from a black pixel.
            (px * f).abs() <= hx * (1.0 - 1e-5) && (py * f).abs() <= hy * (1.0 - 1e-5)
        })
    };
    let (mut lo, mut hi) = (0.5f32, 2.0f32);
    if !fits(hi) {
        return hi;
    }
    for _ in 0..40 {
        let mid = 0.5 * (lo + hi);
        if fits(mid) {
            hi = mid;
        } else {
            lo = mid;
        }
    }
    hi
}

#[cfg(test)]
mod tests {
    use super::*;

    const XML: &str = r#"<!DOCTYPE lensdatabase SYSTEM "lensfun-database.dtd">
<lensdatabase version="2">
    <camera>
        <maker>Nikon Corporation</maker>
        <maker lang="en">Nikon</maker>
        <model>Nikon D3300</model>
        <model lang="en">D3300</model>
        <cropfactor>1.523</cropfactor>
    </camera>
    <lens>
        <maker>Sigma</maker>
        <model>Sigma 17-50mm f/2.8 EX DC OS HSM</model>
        <mount>Nikon F AF</mount>
        <cropfactor>1.523</cropfactor>
        <calibration>
            <distortion model="ptlens" focal="17" a="0.03046" b="-0.085" c="0.04461"/>
            <distortion model="ptlens" focal="50" a="0.00161" b="0.00176" c="0.00235"/>
            <tca model="poly3" focal="17" br="-0.0000069" vr="1.0005013" bb="-0.0000010" vb="1.0000428"/>
            <vignetting model="pa" focal="17" aperture="2.8" distance="10" k1="-0.2563" k2="-0.2256" k3="-0.0574"/>
            <vignetting model="pa" focal="17" aperture="2.8" distance="1000" k1="-0.25" k2="-0.22" k3="-0.05"/>
            <vignetting model="pa" focal="17" aperture="5.6" distance="1000" k1="-0.1" k2="0.0" k3="0.0"/>
        </calibration>
    </lens>
    <lens>
        <maker>Sigma</maker>
        <model>Sigma 17-70mm f/2.8-4 DC Macro OS HSM</model>
        <cropfactor>1.523</cropfactor>
    </lens>
    </lensdatabase>"#;

    fn db() -> LensDb {
        let (lenses, cameras) = parse(XML);
        LensDb { lenses, cameras, sources: Vec::new() }
    }

    #[test]
    fn parses_lenses_cameras_and_models() {
        let db = db();
        assert_eq!(db.lenses.len(), 2);
        assert_eq!(db.cameras[0].crop_factor, 1.523);
        let l = &db.lenses[0];
        assert_eq!(l.distortion.len(), 2);
        // ptlens a,b,c → p0 = 1-a-b-c, p1 = c, p2 = b, p3 = a.
        let d = l.distortion[0].1;
        assert!((d[0] - (1.0 - 0.03046 + 0.085 - 0.04461)).abs() < 1e-6);
        assert_eq!((d[1], d[2], d[3]), (0.04461, -0.085, 0.03046));
        assert_eq!(l.vignetting.len(), 3);
    }

    /// The maker-note name and the database name differ in spelling, but
    /// every number must match: the 17-70 must never stand in for the 17-50.
    #[test]
    fn finds_the_lens_despite_spelling_and_never_a_neighbour() {
        let db = db();
        let l = db.find("Sigma", "17-50mm F2.8 EX DC OS HSM").expect("match");
        assert!(l.model.contains("17-50"));
        assert!(db.find("Sigma", "17-50mm F4").is_none(), "aperture must match");
        assert!(db.find("Nikon", "17-50mm F2.8 EX DC OS HSM").is_none(), "maker must match");
    }

    /// RW2 lens names come from rawler's table, which calls Panasonic's
    /// lenses "Lumix" — lensfun files them under "Panasonic", and spells
    /// apertures "f/3.5" where the camera says "F3.5". Accents differ too.
    #[test]
    fn finds_lenses_under_another_brand_name_or_spelling() {
        let (lenses, cameras) = parse(
            r#"<lensdatabase>
            <lens><maker>Panasonic</maker><model>Lumix G Vario 12-32mm f/3.5-5.6 Asph. Mega OIS</model>
                <mount>Micro 4/3 System</mount><cropfactor>2</cropfactor></lens>
            <lens><maker>Panasonic</maker><model>Lumix G Vario 14-42mm f/3.5-5.6 II Asph. Mega OIS</model>
                <mount>Micro 4/3 System</mount><cropfactor>2</cropfactor></lens>
            <lens><maker>Voigtländer</maker><model>Voigtlander Nokton 58mm F1.4 SLII</model>
                <mount>Nikon F AF</mount><cropfactor>1</cropfactor></lens>
            </lensdatabase>"#,
        );
        let db = LensDb { lenses, cameras, sources: Vec::new() };
        let l = db.find("Lumix", "G Vario 12-32mm F3.5-5.6 Asph. Mega OIS").expect("Lumix is Panasonic");
        assert!(l.model.contains("12-32mm"));
        assert!(db.find("Voigtlander", "Nokton 58mm F1.4 SLII").is_some(), "the umlaut kept them apart");
        // Aliases widen the maker, never the numbers.
        assert!(db.find("Lumix", "G Vario 12-60mm F3.5-5.6 Asph. Power OIS").is_none());
        assert!(db.find("Olympus", "G Vario 12-32mm F3.5-5.6 Asph. Mega OIS").is_none(), "maker must match");
    }

    #[test]
    fn tokens_split_letters_from_digits_but_keep_decimals() {
        let t = tokens("AF-S DX 18-55mm f/3.5-5.6G");
        for w in ["af", "s", "dx", "18", "55", "mm", "f", "3.5", "5.6", "g"] {
            assert!(t.contains(w), "{w} missing from {t:?}");
        }
    }

    #[test]
    fn camera_crop_from_database() {
        let db = db();
        assert_eq!(db.camera_crop("NIKON CORPORATION", "NIKON D3300"), Some(1.523));
        assert_eq!(db.camera_crop("Nikon", "D3400"), None);
    }

    #[test]
    fn interpolation_clamps_and_blends() {
        let l = &db().lenses[0];
        let at17 = l.interpolate(17.0, 2.8);
        let at10 = l.interpolate(10.0, 2.8);
        assert_eq!(at17.distortion, at10.distortion, "clamped below range");
        let mid = l.interpolate(33.5, 2.8).distortion.unwrap();
        let (a, b) = (l.distortion[0].1, l.distortion[1].1);
        assert!((mid[3] - 0.5 * (a[3] + b[3])).abs() < 1e-6);
        // Vignetting uses the farthest focus distance, and weakens stopped down.
        let v28 = l.interpolate(17.0, 2.8).vignetting.unwrap();
        assert_eq!(v28, [-0.25, -0.22, -0.05]);
        let v4 = l.interpolate(17.0, 4.0).vignetting.unwrap();
        assert!(v4[0] > v28[0] && v4[0] < -0.1);
    }

    /// Positive manual distortion removes barrel: edge pixels must sample
    /// from nearer the centre (f < 1 there), as a barrel profile does.
    #[test]
    fn positive_manual_distortion_corrects_barrel() {
        let p = LensParams { distortion: 1.0, constrain: false, ..Default::default() };
        let g = gpu_params(&LensContext::default(), &p, 6000, 4000);
        let corner = 0.5 * g.scale[0].hypot(g.scale[1]);
        let f = poly(&g.dist, corner);
        assert!((f - (1.0 - MANUAL_DISTORTION)).abs() < 1e-4, "corner factor {f}");
    }

    #[test]
    fn neutral_settings_are_inactive() {
        let g = gpu_params(&LensContext::default(), &LensParams::default(), 6000, 4000);
        assert!(!g.active);
        assert_eq!(g.dist, [1.0, 0.0, 0.0, 0.0, 0.0]);
    }

    /// A profile only applies once enabled — saved edits from before lens
    /// corrections existed must render unchanged.
    #[test]
    fn profile_is_opt_in() {
        let l = &db().lenses[0];
        let ctx = LensContext { profile: Some(l.interpolate(17.0, 2.8)), camera_crop: Some(1.523), ..Default::default() };
        assert!(!gpu_params(&ctx, &LensParams::default(), 6000, 4000).active);
        let on = LensParams { profile: true, ..Default::default() };
        assert!(gpu_params(&ctx, &on, 6000, 4000).active);
    }

    /// Same sensor as the calibration: the shorter half-side is exactly 1.
    #[test]
    fn same_sensor_normalises_to_half_the_short_side() {
        let ctx = LensContext { profile: Some(db().lenses[0].interpolate(17.0, 2.8)), camera_crop: Some(1.523), ..Default::default() };
        let p = LensParams { profile: true, constrain: false, ..Default::default() };
        let g = gpu_params(&ctx, &p, 6000, 4000);
        // UV half-height (0.5) maps to radius 1.
        assert!((0.5 * g.scale[1] - 1.0).abs() < 1e-3, "{:?}", g.scale);
        assert!((0.5 * g.scale[0] - 1.5).abs() < 1e-3);
    }

    /// Where the colour shader's Step 0 samples the source for output UV
    /// `uv` (green channel), computed from the same `GpuLens` fields the
    /// shader reads. Kept line for line with `shaders.rs`: the bug this
    /// guards against was a mismatch between what `gpu_params` assumed the
    /// shader did and what it does.
    fn shader_source_uv(g: &GpuLens, uv: [f32; 2]) -> [f32; 2] {
        let p = [(uv[0] - 0.5) * g.scale[0], (uv[1] - 0.5) * g.scale[1]];
        let ru = p[0].hypot(p[1]);
        let f = poly(&g.dist, ru);
        let pd = [p[0] * f, p[1] * f];
        [pd[0] / g.scale[0] + 0.5, pd[1] / g.scale[1] + 0.5]
    }

    /// Output UVs all the way round the frame's edge.
    fn output_border() -> Vec<[f32; 2]> {
        (0..=64)
            .flat_map(|i| {
                let t = i as f32 / 64.0;
                [[t, 0.0], [t, 1.0], [0.0, t], [1.0, t]]
            })
            .collect()
    }

    fn inside(uv: [f32; 2]) -> bool {
        (0.0..=1.0).contains(&uv[0]) && (0.0..=1.0).contains(&uv[1])
    }

    /// The point of Constrain, end to end through the shader's own mapping:
    /// a correction that pulls the frame's edges inwards leaves empty
    /// borders without it, and none with it.
    #[test]
    fn constrain_leaves_no_empty_border_in_the_shader() {
        let ctx = LensContext::default();
        // Manual distortion −1 undoes pincushion: edges sample from further
        // out than the frame has.
        for distortion in [-1.0f32, -0.4] {
            let off = gpu_params(
                &ctx,
                &LensParams { distortion, constrain: false, ..Default::default() },
                6000,
                4000,
            );
            assert!(
                output_border().iter().any(|&uv| !inside(shader_source_uv(&off, uv))),
                "test needs a correction that leaves empty borders"
            );
            let on = gpu_params(
                &ctx,
                &LensParams { distortion, constrain: true, ..Default::default() },
                6000,
                4000,
            );
            for uv in output_border() {
                let src = shader_source_uv(&on, uv);
                assert!(inside(src), "distortion {distortion}: border {uv:?} samples {src:?}");
            }
            // And it zooms no further than it has to: some border point
            // lands on the source's edge.
            let tightest = output_border()
                .iter()
                .map(|&uv| {
                    let s = shader_source_uv(&on, uv);
                    s[0].min(1.0 - s[0]).min(s[1]).min(1.0 - s[1])
                })
                .fold(f32::MAX, f32::min);
            assert!(tightest < 1e-3, "over-zoomed: closest border sample {tightest} from the edge");
        }
    }

    /// Barrel-correcting pulls nothing in from outside, and constraining it
    /// zooms *out* to use the frame the correction would otherwise discard.
    #[test]
    fn constrain_on_a_barrel_correction_reveals_more_without_borders() {
        let ctx = LensContext::default();
        let on = gpu_params(
            &ctx,
            &LensParams { distortion: 1.0, constrain: true, ..Default::default() },
            6000,
            4000,
        );
        let off = gpu_params(
            &ctx,
            &LensParams { distortion: 1.0, constrain: false, ..Default::default() },
            6000,
            4000,
        );
        for uv in output_border() {
            assert!(inside(shader_source_uv(&on, uv)), "border {uv:?}");
        }
        // The corner reaches further into the source than without.
        let reach = |g: &GpuLens| (shader_source_uv(g, [0.0, 0.0])[0] - 0.5).abs();
        assert!(reach(&on) > reach(&off) + 1e-3, "on {} off {}", reach(&on), reach(&off));
    }

    /// Constrain must leave every border point inside the source.
    #[test]
    fn constrain_removes_empty_borders_both_ways() {
        let base = [3.0, 2.0];
        for k in [-0.05f32, 0.05] {
            let d = [1.0, 0.0, k, 0.0, 0.0];
            let s = constrain_scale(&d, base);
            if k > 0.0 {
                assert!(s > 1.0, "pincushion-correcting needs zoom in: {s}");
            } else {
                assert!(s < 1.0, "barrel-correcting can zoom out: {s}");
            }
            for (x, y) in [(1.5f32, 1.0f32), (1.5, 0.0), (0.0, 1.0)] {
                let (px, py) = (x / s, y / s);
                let f = poly(&d, px.hypot(py));
                assert!((px * f).abs() <= 1.5 * 1.001 && (py * f).abs() <= 1.0 * 1.001);
            }
        }
    }
}
