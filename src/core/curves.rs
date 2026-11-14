//! Point tone curves: an RGB master curve plus per-channel R, G, B curves.
//!
//! Applied in display-encoded space (after the sRGB transfer function), which
//! is where photographers expect a curve's midpoint to sit — moving the
//! 0.5/0.5 point there is a midtone move, not a shadow one.
//!
//! The curves are interpolated with a **monotone** cubic (Fritsch–Carlson):
//! an ordinary natural spline overshoots between closely spaced points, which
//! on a tone curve means a local contrast *reversal* — solarisation. Baked on
//! the CPU into a 1D LUT, so the shader pays one texture fetch per channel.

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// Control points per curve. Fixed so `EditParams` stays `Copy`.
pub const MAX_CURVE_POINTS: usize = 16;
/// Entries in the baked LUT.
pub const CURVE_LUT_SIZE: usize = 1024;
/// Minimum horizontal gap between neighbouring points: keeps the spline's
/// secant slopes finite.
pub const MIN_POINT_GAP: f32 = 0.01;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Curve {
    /// `(x, y)` in `[0, 1]`, sorted by `x`; only the first `count` are live.
    pub points: [[f32; 2]; MAX_CURVE_POINTS],
    pub count: u32,
}

impl Default for Curve {
    fn default() -> Self {
        let mut points = [[0.0; 2]; MAX_CURVE_POINTS];
        points[1] = [1.0, 1.0];
        Self { points, count: 2 }
    }
}

// Serialised as just the live points: unused slots would otherwise bloat
// every saved edit with fourteen dead pairs per channel.
impl Serialize for Curve {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        self.live().serialize(s)
    }
}

impl<'de> Deserialize<'de> for Curve {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let pts: Vec<[f32; 2]> = Vec::deserialize(d)?;
        Ok(Curve::from_points(&pts))
    }
}

impl Curve {
    pub fn live(&self) -> &[[f32; 2]] {
        &self.points[..(self.count as usize).min(MAX_CURVE_POINTS)]
    }

    /// Build from arbitrary points: clamped to the unit square, sorted,
    /// de-duplicated and capped. Fewer than two points gives the identity.
    pub fn from_points(pts: &[[f32; 2]]) -> Self {
        let mut v: Vec<[f32; 2]> = pts
            .iter()
            .filter(|p| p[0].is_finite() && p[1].is_finite())
            .map(|p| [p[0].clamp(0.0, 1.0), p[1].clamp(0.0, 1.0)])
            .collect();
        v.sort_by(|a, b| a[0].total_cmp(&b[0]));
        v.dedup_by(|b, a| (b[0] - a[0]).abs() < MIN_POINT_GAP * 0.5);
        v.truncate(MAX_CURVE_POINTS);
        if v.len() < 2 {
            return Self::default();
        }
        let mut c = Self { points: [[0.0; 2]; MAX_CURVE_POINTS], count: v.len() as u32 };
        c.points[..v.len()].copy_from_slice(&v);
        c
    }

    pub fn is_identity(&self) -> bool {
        // The curve is flat beyond its end points, so a curve that stops short
        // of x = 0 or x = 1 clips there even if every point sits on the diagonal.
        let pts = self.live();
        let (Some(first), Some(last)) = (pts.first(), pts.last()) else {
            return true;
        };
        first[0] < 1e-4
            && last[0] > 1.0 - 1e-4
            && pts.iter().all(|p| (p[0] - p[1]).abs() < 1e-4)
    }

    /// Insert a point, keeping order. Returns its index, or `None` when the
    /// curve is full or a point already sits within `MIN_POINT_GAP` of `x`.
    pub fn insert(&mut self, x: f32, y: f32) -> Option<usize> {
        let n = self.count as usize;
        if n >= MAX_CURVE_POINTS {
            return None;
        }
        let (x, y) = (x.clamp(0.0, 1.0), y.clamp(0.0, 1.0));
        if self.live().iter().any(|p| (p[0] - x).abs() < MIN_POINT_GAP) {
            return None;
        }
        let i = self.live().iter().position(|p| p[0] > x).unwrap_or(n);
        self.points.copy_within(i..n, i + 1);
        self.points[i] = [x, y];
        self.count += 1;
        Some(i)
    }

    /// Remove a point; the curve never drops below two.
    pub fn remove(&mut self, i: usize) {
        let n = self.count as usize;
        if n <= 2 || i >= n {
            return;
        }
        self.points.copy_within(i + 1..n, i);
        self.points[n - 1] = [0.0; 2];
        self.count -= 1;
    }

    /// Move point `i`, constrained between its neighbours so order holds.
    pub fn move_point(&mut self, i: usize, x: f32, y: f32) {
        let n = self.count as usize;
        if i >= n {
            return;
        }
        let lo = if i == 0 { 0.0 } else { self.points[i - 1][0] + MIN_POINT_GAP };
        let hi = if i + 1 == n { 1.0 } else { self.points[i + 1][0] - MIN_POINT_GAP };
        self.points[i] = [x.clamp(lo, hi.max(lo)), y.clamp(0.0, 1.0)];
    }

    /// Evaluate at `x` (flat beyond the end points).
    pub fn eval(&self, x: f32) -> f32 {
        let pts = self.live();
        let tangents = tangents(pts);
        eval_with(pts, &tangents, x)
    }
}

/// Fritsch–Carlson tangents: secant averages, zeroed at local extrema and
/// limited so the Hermite segment cannot overshoot its end values.
fn tangents(p: &[[f32; 2]]) -> Vec<f32> {
    let n = p.len();
    if n < 2 {
        return vec![0.0; n];
    }
    let d: Vec<f32> = (0..n - 1)
        .map(|i| (p[i + 1][1] - p[i][1]) / (p[i + 1][0] - p[i][0]).max(1e-6))
        .collect();
    let mut m = vec![0.0f32; n];
    m[0] = d[0];
    m[n - 1] = d[n - 2];
    for i in 1..n - 1 {
        m[i] = if d[i - 1] * d[i] <= 0.0 { 0.0 } else { 0.5 * (d[i - 1] + d[i]) };
    }
    for i in 0..n - 1 {
        if d[i] == 0.0 {
            m[i] = 0.0;
            m[i + 1] = 0.0;
            continue;
        }
        let a = m[i] / d[i];
        let b = m[i + 1] / d[i];
        let s = a * a + b * b;
        if s > 9.0 {
            let t = 3.0 / s.sqrt();
            m[i] = t * a * d[i];
            m[i + 1] = t * b * d[i];
        }
    }
    m
}

fn eval_with(p: &[[f32; 2]], m: &[f32], x: f32) -> f32 {
    let n = p.len();
    if n == 0 {
        return x;
    }
    if x <= p[0][0] {
        return p[0][1];
    }
    if x >= p[n - 1][0] {
        return p[n - 1][1];
    }
    let i = p.iter().rposition(|q| q[0] <= x).unwrap_or(0).min(n - 2);
    let h = (p[i + 1][0] - p[i][0]).max(1e-6);
    let t = (x - p[i][0]) / h;
    let (t2, t3) = (t * t, t * t * t);
    let h00 = 2.0 * t3 - 3.0 * t2 + 1.0;
    let h10 = t3 - 2.0 * t2 + t;
    let h01 = -2.0 * t3 + 3.0 * t2;
    let h11 = t3 - t2;
    (h00 * p[i][1] + h10 * h * m[i] + h01 * p[i + 1][1] + h11 * h * m[i + 1]).clamp(0.0, 1.0)
}

/// Which curve the editor is showing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CurveChannel {
    #[default]
    Master,
    Red,
    Green,
    Blue,
}

impl CurveChannel {
    pub const ALL: [CurveChannel; 4] = [Self::Master, Self::Red, Self::Green, Self::Blue];

    pub fn label(self) -> &'static str {
        match self {
            Self::Master => "RGB",
            Self::Red => "R",
            Self::Green => "G",
            Self::Blue => "B",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct ToneCurves {
    #[serde(default)]
    pub master: Curve,
    #[serde(default)]
    pub red: Curve,
    #[serde(default)]
    pub green: Curve,
    #[serde(default)]
    pub blue: Curve,
}

impl ToneCurves {
    pub fn get(&self, ch: CurveChannel) -> &Curve {
        match ch {
            CurveChannel::Master => &self.master,
            CurveChannel::Red => &self.red,
            CurveChannel::Green => &self.green,
            CurveChannel::Blue => &self.blue,
        }
    }

    pub fn get_mut(&mut self, ch: CurveChannel) -> &mut Curve {
        match ch {
            CurveChannel::Master => &mut self.master,
            CurveChannel::Red => &mut self.red,
            CurveChannel::Green => &mut self.green,
            CurveChannel::Blue => &mut self.blue,
        }
    }

    pub fn is_identity(&self) -> bool {
        CurveChannel::ALL.iter().all(|&c| self.get(c).is_identity())
    }

    /// Bake to `CURVE_LUT_SIZE` RGBA entries: (R∘M, G∘M, B∘M, M) sampled at
    /// `i / (N − 1)`. The per-channel curves are composed with the master
    /// here, so the shader needs one lookup per channel, not two.
    pub fn bake(&self) -> Vec<[f32; 4]> {
        let lut = |c: &Curve| -> Vec<f32> {
            let pts = c.live();
            let m = tangents(pts);
            (0..CURVE_LUT_SIZE)
                .map(|i| eval_with(pts, &m, i as f32 / (CURVE_LUT_SIZE - 1) as f32))
                .collect()
        };
        let master = lut(&self.master);
        let (r, g, b) = (lut(&self.red), lut(&self.green), lut(&self.blue));
        let at = |table: &[f32], x: f32| -> f32 {
            let f = x.clamp(0.0, 1.0) * (CURVE_LUT_SIZE - 1) as f32;
            let i = (f.floor() as usize).min(CURVE_LUT_SIZE - 2);
            let t = f - i as f32;
            table[i] * (1.0 - t) + table[i + 1] * t
        };
        (0..CURVE_LUT_SIZE)
            .map(|i| {
                let mv = master[i];
                [at(&r, mv), at(&g, mv), at(&b, mv), mv]
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_identity_and_bakes_to_identity() {
        let c = ToneCurves::default();
        assert!(c.is_identity());
        for (i, e) in c.bake().iter().enumerate() {
            let x = i as f32 / (CURVE_LUT_SIZE - 1) as f32;
            for v in e {
                assert!((v - x).abs() < 1e-4, "entry {i}: {v} vs {x}");
            }
        }
    }

    /// A diagonal that stops short of the ends clips there, so it is an edit.
    #[test]
    fn diagonal_curve_with_inset_end_points_is_not_identity() {
        let c = Curve::from_points(&[[0.2, 0.2], [0.8, 0.8]]);
        assert!(!c.is_identity());
        assert!((c.eval(0.0) - 0.2).abs() < 1e-5, "shadows clip to 0.2");
        assert!(Curve::from_points(&[[0.0, 0.0], [0.5, 0.5], [1.0, 1.0]]).is_identity());
    }

    /// The point of the monotone spline: a steep S-curve must not overshoot
    /// or reverse between its points.
    #[test]
    fn s_curve_is_monotone_and_hits_its_points() {
        let c = Curve::from_points(&[[0.0, 0.0], [0.25, 0.15], [0.3, 0.6], [0.75, 0.85], [1.0, 1.0]]);
        let mut prev = -1.0;
        for i in 0..=1000 {
            let y = c.eval(i as f32 / 1000.0);
            assert!(y >= prev - 1e-6, "reversal at {i}");
            assert!((0.0..=1.0).contains(&y));
            prev = y;
        }
        for p in c.live() {
            assert!((c.eval(p[0]) - p[1]).abs() < 1e-5);
        }
    }

    #[test]
    fn insert_keeps_order_and_rejects_crowding() {
        let mut c = Curve::default();
        assert_eq!(c.insert(0.5, 0.6), Some(1));
        assert_eq!(c.insert(0.25, 0.2), Some(1));
        assert_eq!(c.insert(0.505, 0.9), None, "too close to an existing point");
        let xs: Vec<f32> = c.live().iter().map(|p| p[0]).collect();
        assert_eq!(xs, vec![0.0, 0.25, 0.5, 1.0]);
    }

    #[test]
    fn move_point_cannot_cross_neighbours() {
        let mut c = Curve::from_points(&[[0.0, 0.0], [0.5, 0.5], [1.0, 1.0]]);
        c.move_point(1, 2.0, 0.7);
        assert!(c.points[1][0] <= 1.0 - MIN_POINT_GAP + 1e-6);
        c.move_point(1, -1.0, 0.7);
        assert!(c.points[1][0] >= MIN_POINT_GAP - 1e-6);
    }

    #[test]
    fn remove_never_goes_below_two_points() {
        let mut c = Curve::default();
        c.remove(0);
        assert_eq!(c.count, 2);
    }

    #[test]
    fn serialises_only_live_points_and_round_trips() {
        let mut t = ToneCurves::default();
        t.master.insert(0.5, 0.6);
        let json = serde_json::to_string(&t).unwrap();
        assert!(json.contains("[[0.0,0.0],[0.5,0.6],[1.0,1.0]]"), "{json}");
        let back: ToneCurves = serde_json::from_str(&json).unwrap();
        assert_eq!(back, t);
    }

    /// Master and channel compose: R∘M, applied once in the shader.
    #[test]
    fn bake_composes_channel_after_master() {
        let mut t = ToneCurves::default();
        t.master = Curve::from_points(&[[0.0, 0.0], [1.0, 0.5]]);
        t.red = Curve::from_points(&[[0.0, 0.0], [0.5, 1.0], [1.0, 1.0]]);
        let lut = t.bake();
        let top = lut[CURVE_LUT_SIZE - 1];
        assert!((top[3] - 0.5).abs() < 1e-4, "master");
        assert!((top[0] - 1.0).abs() < 1e-3, "red applied to master output");
        assert!((top[1] - 0.5).abs() < 1e-3, "green untouched");
    }
}
