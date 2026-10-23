//! Brush strokes for local adjustment masks.
//!
//! A stroke is a polyline in full-image display UV (the same space as mask
//! geometry and the crop) with a radius, feather and flow; it either adds to
//! or erases from one mask. Strokes are *data*, not pixels: they live in
//! `EditParams` (so undo, copy/paste and saving work unchanged) and the GPU
//! rasterises them into per-mask layers whenever they change (`gpu::brush`).
//!
//! Storage is fixed-capacity so `EditParams` stays `Copy`. Points are
//! decimated while painting and simplified (Ramer–Douglas–Peucker) when a
//! stroke ends, so a long gesture costs tens of points, not thousands.

use serde::{Deserialize, Deserializer, Serialize, Serializer};

pub const MAX_STROKES: usize = 128;
pub const MAX_POINTS: usize = 2048;

#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct BrushStroke {
    /// Index of the mask this stroke belongs to.
    pub mask: u32,
    /// Erase from the mask instead of adding to it.
    pub erase: bool,
    /// Radius as a fraction of the image HEIGHT (UV is not isotropic, so one
    /// axis has to be the unit).
    pub radius: f32,
    /// 0 = hard edge … 1 = falloff from the centre.
    pub feather: f32,
    /// Strength of this stroke, 0..1.
    pub flow: f32,
    /// Slice of `BrushData::points`.
    pub start: u32,
    pub len: u32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BrushData {
    pub strokes: [BrushStroke; MAX_STROKES],
    pub stroke_count: u32,
    pub points: [[f32; 2]; MAX_POINTS],
    pub point_count: u32,
}

impl Default for BrushData {
    fn default() -> Self {
        Self {
            strokes: [BrushStroke::default(); MAX_STROKES],
            stroke_count: 0,
            points: [[0.0; 2]; MAX_POINTS],
            point_count: 0,
        }
    }
}

/// Serialised form: just the live strokes, each with its own points.
#[derive(Serialize, Deserialize)]
struct StrokeJson {
    mask: u32,
    #[serde(default)]
    erase: bool,
    radius: f32,
    #[serde(default)]
    feather: f32,
    #[serde(default = "one")]
    flow: f32,
    points: Vec<[f32; 2]>,
}

fn one() -> f32 {
    1.0
}

impl Serialize for BrushData {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let strokes: Vec<StrokeJson> = self
            .live()
            .iter()
            .map(|st| StrokeJson {
                mask: st.mask,
                erase: st.erase,
                radius: st.radius,
                feather: st.feather,
                flow: st.flow,
                points: self.points_of(st).to_vec(),
            })
            .collect();
        strokes.serialize(s)
    }
}

impl<'de> Deserialize<'de> for BrushData {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let strokes: Vec<StrokeJson> = Vec::deserialize(d)?;
        let mut b = BrushData::default();
        for st in strokes {
            let Some(i) = b.begin(st.mask, st.erase, st.radius, st.feather, st.flow) else { break };
            for p in st.points {
                if !b.push_raw(p) {
                    break;
                }
            }
            if b.strokes[i].len == 0 {
                b.stroke_count -= 1;
            }
        }
        Ok(b)
    }
}

impl BrushData {
    pub fn live(&self) -> &[BrushStroke] {
        &self.strokes[..(self.stroke_count as usize).min(MAX_STROKES)]
    }

    pub fn points_of(&self, s: &BrushStroke) -> &[[f32; 2]] {
        let a = (s.start as usize).min(MAX_POINTS);
        let b = (s.start as usize + s.len as usize).min(MAX_POINTS);
        &self.points[a..b]
    }

    pub fn is_empty(&self) -> bool {
        self.stroke_count == 0
    }

    pub fn has_strokes_for(&self, mask: usize) -> bool {
        self.live().iter().any(|s| s.mask as usize == mask)
    }

    /// Start a stroke. `None` when the stroke or point budget is exhausted.
    pub fn begin(&mut self, mask: u32, erase: bool, radius: f32, feather: f32, flow: f32) -> Option<usize> {
        let i = self.stroke_count as usize;
        if i >= MAX_STROKES || self.point_count as usize >= MAX_POINTS {
            return None;
        }
        self.strokes[i] = BrushStroke {
            mask,
            erase,
            radius: radius.clamp(0.001, 1.0),
            feather: feather.clamp(0.0, 1.0),
            flow: flow.clamp(0.0, 1.0),
            start: self.point_count,
            len: 0,
        };
        self.stroke_count += 1;
        Some(i)
    }

    /// Append to the last stroke unconditionally. False when full.
    fn push_raw(&mut self, p: [f32; 2]) -> bool {
        let n = self.point_count as usize;
        if n >= MAX_POINTS || self.stroke_count == 0 {
            return false;
        }
        self.points[n] = p;
        self.point_count += 1;
        self.strokes[self.stroke_count as usize - 1].len += 1;
        true
    }

    /// Append to the stroke being painted, skipping points closer than a
    /// quarter radius to the previous one (the GPU draws capsules between
    /// points, so denser sampling adds nothing). `aspect` = width / height of
    /// the image, to measure distance isotropically. False when full.
    pub fn extend(&mut self, p: [f32; 2], aspect: f32) -> bool {
        let Some(last) = self.live().last().copied() else { return false };
        if let Some(prev) = self.points_of(&last).last() {
            let dx = (p[0] - prev[0]) * aspect;
            let dy = p[1] - prev[1];
            if (dx * dx + dy * dy).sqrt() < last.radius * 0.25 {
                return true;
            }
        }
        self.push_raw(p)
    }

    /// Simplify the last stroke in place (RDP, tolerance ~ a tenth of the
    /// radius): the rasteriser draws capsules, so collinear interior points
    /// are pure cost.
    pub fn finish(&mut self, aspect: f32) {
        let Some(&last) = self.live().last() else { return };
        let pts: Vec<[f32; 2]> = self.points_of(&last).to_vec();
        if pts.len() < 3 {
            return;
        }
        let tol = last.radius * 0.1;
        let mut keep = vec![false; pts.len()];
        keep[0] = true;
        keep[pts.len() - 1] = true;
        rdp(&pts, 0, pts.len() - 1, tol, aspect, &mut keep);
        let simplified: Vec<[f32; 2]> = pts.iter().zip(&keep).filter(|(_, k)| **k).map(|(p, _)| *p).collect();
        let start = last.start as usize;
        self.points[start..start + simplified.len()].copy_from_slice(&simplified);
        let i = self.stroke_count as usize - 1;
        self.strokes[i].len = simplified.len() as u32;
        self.point_count = (start + simplified.len()) as u32;
    }

    /// Drop every stroke of `mask` and renumber strokes of later masks, so
    /// they follow their mask down a slot when one is deleted.
    pub fn remove_mask(&mut self, mask: usize) {
        let old = *self;
        *self = BrushData::default();
        for st in old.live() {
            let m = st.mask as usize;
            if m == mask {
                continue;
            }
            let new_mask = if m > mask { m - 1 } else { m } as u32;
            if self.begin(new_mask, st.erase, st.radius, st.feather, st.flow).is_none() {
                break;
            }
            for p in old.points_of(st) {
                self.push_raw(*p);
            }
        }
    }

    /// Follow the frame through a quarter turn (`EditParams::turn_quarter`):
    /// points turn rigidly, and radii — fractions of the image HEIGHT, which
    /// the turn swaps with the width — are rescaled to keep their size in
    /// pixels. `aspect` = width / height before the turn.
    pub fn turn_quarter(&mut self, clockwise: bool, aspect: f32) {
        let n = (self.point_count as usize).min(MAX_POINTS);
        for p in &mut self.points[..n] {
            *p = if clockwise { [1.0 - p[1], p[0]] } else { [p[1], 1.0 - p[0]] };
        }
        let n = (self.stroke_count as usize).min(MAX_STROKES);
        for s in &mut self.strokes[..n] {
            s.radius = (s.radius / aspect.max(1e-6)).clamp(0.001, 1.0);
        }
    }

    /// Remove every stroke of `mask` (the panel's "Clear brush"), keeping
    /// mask numbering.
    pub fn clear_mask(&mut self, mask: usize) {
        let old = *self;
        *self = BrushData::default();
        for st in old.live().iter().filter(|s| s.mask as usize != mask) {
            if self.begin(st.mask, st.erase, st.radius, st.feather, st.flow).is_none() {
                break;
            }
            for p in old.points_of(st) {
                self.push_raw(*p);
            }
        }
    }
}

fn rdp(pts: &[[f32; 2]], a: usize, b: usize, tol: f32, aspect: f32, keep: &mut [bool]) {
    if b <= a + 1 {
        return;
    }
    let (ax, ay) = (pts[a][0] * aspect, pts[a][1]);
    let (bx, by) = (pts[b][0] * aspect, pts[b][1]);
    let (dx, dy) = (bx - ax, by - ay);
    let len2 = (dx * dx + dy * dy).max(1e-12);
    let mut worst = (0.0f32, a);
    for (i, p) in pts.iter().enumerate().take(b).skip(a + 1) {
        let (px, py) = (p[0] * aspect, p[1]);
        let t = (((px - ax) * dx + (py - ay) * dy) / len2).clamp(0.0, 1.0);
        let (qx, qy) = (ax + t * dx, ay + t * dy);
        let d = ((px - qx).powi(2) + (py - qy).powi(2)).sqrt();
        if d > worst.0 {
            worst = (d, i);
        }
    }
    if worst.0 > tol {
        keep[worst.1] = true;
        rdp(pts, a, worst.1, tol, aspect, keep);
        rdp(pts, worst.1, b, tol, aspect, keep);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stroke(b: &mut BrushData, mask: u32, pts: &[[f32; 2]]) {
        b.begin(mask, false, 0.05, 0.5, 1.0).unwrap();
        for p in pts {
            b.extend(*p, 1.5);
        }
    }

    #[test]
    fn decimation_skips_points_closer_than_a_quarter_radius() {
        let mut b = BrushData::default();
        stroke(&mut b, 0, &[[0.1, 0.1], [0.1001, 0.1], [0.2, 0.1]]);
        assert_eq!(b.strokes[0].len, 2);
    }

    #[test]
    fn finishing_a_straight_stroke_keeps_only_its_ends() {
        let mut b = BrushData::default();
        let pts: Vec<[f32; 2]> = (0..50).map(|i| [0.1 + i as f32 * 0.01, 0.5]).collect();
        stroke(&mut b, 0, &pts);
        assert!(b.strokes[0].len > 10);
        b.finish(1.5);
        assert_eq!(b.strokes[0].len, 2);
        assert_eq!(b.point_count, 2);
        assert_eq!(b.points_of(&b.strokes[0]), &[[0.1, 0.5], [0.59, 0.5]]);
    }

    #[test]
    fn finishing_keeps_corners() {
        let mut b = BrushData::default();
        let mut pts: Vec<[f32; 2]> = (0..20).map(|i| [0.1 + i as f32 * 0.02, 0.2]).collect();
        pts.extend((1..20).map(|i| [0.48, 0.2 + i as f32 * 0.02]));
        stroke(&mut b, 0, &pts);
        b.finish(1.0);
        let kept = b.points_of(&b.strokes[0]);
        assert_eq!(kept.len(), 3, "{kept:?}");
    }

    #[test]
    fn removing_a_mask_drops_its_strokes_and_renumbers_the_rest() {
        let mut b = BrushData::default();
        stroke(&mut b, 0, &[[0.1, 0.1], [0.3, 0.1]]);
        stroke(&mut b, 1, &[[0.5, 0.5], [0.7, 0.5]]);
        stroke(&mut b, 2, &[[0.2, 0.8], [0.4, 0.8]]);
        b.remove_mask(1);
        assert_eq!(b.stroke_count, 2);
        assert_eq!(b.strokes[0].mask, 0);
        assert_eq!(b.strokes[1].mask, 1, "mask 2 moves down to slot 1");
        assert_eq!(b.points_of(&b.strokes[1])[0], [0.2, 0.8]);
        assert_eq!(b.point_count, 4);
    }

    #[test]
    fn serde_round_trips_only_live_data() {
        let mut b = BrushData::default();
        stroke(&mut b, 3, &[[0.1, 0.1], [0.3, 0.2]]);
        b.strokes[0].erase = true;
        let json = serde_json::to_string(&b).unwrap();
        assert!(json.len() < 200, "{json}");
        let back: BrushData = serde_json::from_str(&json).unwrap();
        assert_eq!(back, b);
    }

    #[test]
    fn capacity_is_enforced_not_overflowed() {
        let mut b = BrushData::default();
        for _ in 0..MAX_STROKES {
            b.begin(0, false, 0.05, 0.5, 1.0).unwrap();
        }
        assert!(b.begin(0, false, 0.05, 0.5, 1.0).is_none());
    }
}
