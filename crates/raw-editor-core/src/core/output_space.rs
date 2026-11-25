//! Export colour spaces: sRGB, Display P3, Adobe RGB (1998) and ProPhoto
//! RGB — the matrices the colour pass converts into, the transfer curves it
//! encodes with, and the ICC profiles that tell other software which space
//! a file is in.
//!
//! The colour pass works in extended-range linear sRGB (§B3), so a wider
//! output space is just a different final matrix and gamut-map target: the
//! colours a wide space can hold are still there at the end of the pass.
//! The display is always sRGB; only exports choose.
//!
//! ## Why the profiles are generated, not bundled
//! A matrix/TRC display profile is three primaries, a white point and a
//! curve — a few hundred bytes. Building them here from the published
//! primaries avoids shipping third-party profile files (and their
//! licences), and guarantees the profile matches the matrices the shader
//! actually used, because both come from `rgb_to_xyz` below. They are ICC
//! v4 with parametric (`para`) curves, which every current colour-managed
//! viewer reads.

/// One export colour space. `shader_id` is what reaches the GPU
/// (`GpuEditParams.output_space`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OutputSpace {
    #[default]
    Srgb,
    DisplayP3,
    AdobeRgb,
    ProPhoto,
}

/// Adobe RGB (1998)'s encoding gamma, 563/256 (the spec's exact value).
///
/// **Mirrored in `gpu/shaders.rs` as `ADOBE_RGB_GAMMA`.**
pub const ADOBE_RGB_GAMMA: f32 = 563.0 / 256.0;

/// ROMM (ProPhoto) RGB's encoding gamma, with a linear segment of slope 16
/// below `PROPHOTO_LINEAR_EDGE` (ISO 22028-2).
///
/// **Mirrored as `PROPHOTO_GAMMA` / `PROPHOTO_LINEAR_EDGE`.**
pub const PROPHOTO_GAMMA: f32 = 1.8;
pub const PROPHOTO_LINEAR_EDGE: f32 = 1.0 / 512.0;

/// CIE 1931 xy chromaticities.
const D65: [f64; 2] = [0.3127, 0.3290];
/// The ICC profile connection space white (D50) as the spec states it, in
/// XYZ. Using it rather than D50's chromaticity makes a profile's three
/// primaries sum to exactly the PCS white, which validators check.
const PCS_D50: [f64; 3] = [0.9642, 1.0, 0.8249];

const BRADFORD: M3 = [
    [0.8951, 0.2664, -0.1614],
    [-0.7502, 1.7135, 0.0367],
    [0.0389, -0.0685, 1.0296],
];

type M3 = [[f64; 3]; 3];

impl OutputSpace {
    pub const ALL: [OutputSpace; 4] =
        [OutputSpace::Srgb, OutputSpace::DisplayP3, OutputSpace::AdobeRgb, OutputSpace::ProPhoto];

    pub fn shader_id(self) -> u32 {
        match self {
            OutputSpace::Srgb => 0,
            OutputSpace::DisplayP3 => 1,
            OutputSpace::AdobeRgb => 2,
            OutputSpace::ProPhoto => 3,
        }
    }

    /// EXIF `ColorSpace`: 1 means sRGB; anything else is "Uncalibrated"
    /// (0xFFFF), and the embedded ICC profile says what it is.
    pub fn exif_color_space(self) -> u16 {
        if self == OutputSpace::Srgb {
            1
        } else {
            0xFFFF
        }
    }

    /// Red, green, blue primaries (xy).
    fn primaries(self) -> [[f64; 2]; 3] {
        match self {
            OutputSpace::Srgb => [[0.64, 0.33], [0.30, 0.60], [0.15, 0.06]],
            OutputSpace::DisplayP3 => [[0.680, 0.320], [0.265, 0.690], [0.150, 0.060]],
            OutputSpace::AdobeRgb => [[0.64, 0.33], [0.21, 0.71], [0.15, 0.06]],
            OutputSpace::ProPhoto => [[0.7347, 0.2653], [0.1596, 0.8404], [0.0366, 0.0001]],
        }
    }

    /// Native white, XYZ (Y = 1).
    fn white(self) -> [f64; 3] {
        match self {
            OutputSpace::ProPhoto => PCS_D50,
            _ => xy_to_xyz(D65),
        }
    }

    /// Linear RGB → XYZ under the space's own white.
    fn to_xyz(self) -> M3 {
        rgb_to_xyz(self.primaries(), self.white())
    }

    /// The colour pass's linear sRGB → this space's linear RGB, row-major.
    /// sRGB is the exact identity (so the default output is untouched);
    /// ProPhoto, whose white is D50, adapts D65 → D50 by Bradford — the
    /// inverse of what the DCP path does to get from ProPhoto to sRGB.
    #[cfg(test)]
    pub fn srgb_to_space_matrix(self) -> [f32; 9] {
        if self == OutputSpace::Srgb {
            return [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0];
        }
        let srgb_to_xyz = OutputSpace::Srgb.to_xyz();
        let adapt = bradford(xy_to_xyz(D65), self.white());
        let m = mul(&inv(&self.to_xyz()), &mul(&adapt, &srgb_to_xyz));
        flatten(&m)
    }

    /// Display-linear → encoded, per channel (the space's transfer curve).
    #[cfg(test)]
    pub fn encode(self, x: f32) -> f32 {
        let x = x.max(0.0);
        match self {
            OutputSpace::Srgb | OutputSpace::DisplayP3 => {
                if x <= 0.003_130_8 {
                    12.92 * x
                } else {
                    1.055 * x.powf(1.0 / 2.4) - 0.055
                }
            }
            OutputSpace::AdobeRgb => x.powf(1.0 / ADOBE_RGB_GAMMA),
            OutputSpace::ProPhoto => {
                if x < PROPHOTO_LINEAR_EDGE {
                    16.0 * x
                } else {
                    x.powf(1.0 / PROPHOTO_GAMMA)
                }
            }
        }
    }

    /// The inverse of `encode`, for tests that read an export back.
    #[cfg(test)]
    pub fn decode(self, e: f32) -> f32 {
        let (kind, g) = self.trc();
        let x = e.max(0.0) as f64;
        (match kind {
            0 => x.powf(g[0]),
            _ if x >= g[4] => (g[1] * x + g[2]).powf(g[0]),
            _ => g[3] * x,
        }) as f32
    }

    /// The inverse of `srgb_to_space_matrix`.
    #[cfg(test)]
    pub fn space_to_srgb_matrix(self) -> [f32; 9] {
        let m = self.srgb_to_space_matrix();
        let m: M3 = [0, 1, 2].map(|i| [0, 1, 2].map(|j| m[i * 3 + j] as f64));
        flatten(&inv(&m))
    }

    /// The ICC `para` curve (decoding direction: encoded → linear): its
    /// function type and parameters.
    fn trc(self) -> (u16, Vec<f64>) {
        match self {
            // Y = ((X + 0.055)/1.055)^2.4 above 0.04045, X/12.92 below.
            OutputSpace::Srgb | OutputSpace::DisplayP3 => {
                (3, vec![2.4, 1.0 / 1.055, 0.055 / 1.055, 1.0 / 12.92, 0.04045])
            }
            OutputSpace::AdobeRgb => (0, vec![ADOBE_RGB_GAMMA as f64]),
            // Y = X^1.8 above 16/512, X/16 below.
            OutputSpace::ProPhoto => {
                (3, vec![PROPHOTO_GAMMA as f64, 1.0, 0.0, 1.0 / 16.0, 16.0 * PROPHOTO_LINEAR_EDGE as f64])
            }
        }
    }

    fn description(self) -> &'static str {
        match self {
            OutputSpace::Srgb => "sRGB (RAW Editor)",
            OutputSpace::DisplayP3 => "Display P3 (RAW Editor)",
            OutputSpace::AdobeRgb => "Adobe RGB (1998) compatible (RAW Editor)",
            OutputSpace::ProPhoto => "ProPhoto RGB / ROMM (RAW Editor)",
        }
    }

    /// A complete ICC v4 RGB display profile for this space.
    pub fn icc_profile(self) -> Vec<u8> {
        let adapt = bradford(self.white(), PCS_D50);
        let pcs = mul(&adapt, &self.to_xyz());
        let column = |i: usize| xyz_tag([pcs[0][i], pcs[1][i], pcs[2][i]]);
        let (kind, params) = self.trc();
        let trc = para_tag(kind, &params);

        let tags: Vec<([u8; 4], Vec<u8>)> = vec![
            (*b"desc", mluc_tag(self.description())),
            (*b"cprt", mluc_tag("No copyright, use freely")),
            (*b"wtpt", xyz_tag(PCS_D50)),
            (*b"chad", sf32_tag(&adapt)),
            (*b"rXYZ", column(0)),
            (*b"gXYZ", column(1)),
            (*b"bXYZ", column(2)),
            (*b"rTRC", trc.clone()),
            (*b"gTRC", trc.clone()),
            (*b"bTRC", trc),
        ];
        build_profile(&tags)
    }
}

impl std::fmt::Display for OutputSpace {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            OutputSpace::Srgb => "sRGB",
            OutputSpace::DisplayP3 => "Display P3",
            OutputSpace::AdobeRgb => "Adobe RGB (1998)",
            OutputSpace::ProPhoto => "ProPhoto RGB",
        })
    }
}

// ── Matrix maths (f64; only ever run per export, never per pixel) ──────────

fn xy_to_xyz(xy: [f64; 2]) -> [f64; 3] {
    [xy[0] / xy[1], 1.0, (1.0 - xy[0] - xy[1]) / xy[1]]
}

fn mul(a: &M3, b: &M3) -> M3 {
    let mut m = [[0.0; 3]; 3];
    for (i, row) in m.iter_mut().enumerate() {
        for (j, v) in row.iter_mut().enumerate() {
            *v = (0..3).map(|k| a[i][k] * b[k][j]).sum();
        }
    }
    m
}

fn apply(m: &M3, v: [f64; 3]) -> [f64; 3] {
    [0, 1, 2].map(|i| m[i][0] * v[0] + m[i][1] * v[1] + m[i][2] * v[2])
}

fn inv(m: &M3) -> M3 {
    let det = m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1])
        - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
        + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0]);
    let c = |r0: usize, c0: usize, r1: usize, c1: usize| m[r0][c0] * m[r1][c1] - m[r0][c1] * m[r1][c0];
    [
        [c(1, 1, 2, 2) / det, -c(0, 1, 2, 2) / det, c(0, 1, 1, 2) / det],
        [-c(1, 0, 2, 2) / det, c(0, 0, 2, 2) / det, -c(0, 0, 1, 2) / det],
        [c(1, 0, 2, 1) / det, -c(0, 0, 2, 1) / det, c(0, 0, 1, 1) / det],
    ]
}

/// To row-major `f32`, with round-off around zero snapped to it: spaces
/// that share a primary with sRGB (P3's and Adobe RGB's blue, Adobe RGB's
/// red) have exact zeros there, and a stray 1e-17 would only be noise.
#[cfg(test)]
fn flatten(m: &M3) -> [f32; 9] {
    [0, 1, 2, 3, 4, 5, 6, 7, 8].map(|i| {
        let v = m[i / 3][i % 3];
        if v.abs() < 1e-9 {
            0.0
        } else {
            v as f32
        }
    })
}

/// The standard construction: primaries as XYZ columns, scaled so that RGB
/// (1, 1, 1) lands on the white.
fn rgb_to_xyz(primaries: [[f64; 2]; 3], white: [f64; 3]) -> M3 {
    let p = primaries.map(xy_to_xyz);
    let cols: M3 = [
        [p[0][0], p[1][0], p[2][0]],
        [p[0][1], p[1][1], p[2][1]],
        [p[0][2], p[1][2], p[2][2]],
    ];
    let s = apply(&inv(&cols), white);
    [0, 1, 2].map(|i| [0, 1, 2].map(|j| cols[i][j] * s[j]))
}

/// Bradford chromatic adaptation from white `src` to white `dst` (XYZ).
fn bradford(src: [f64; 3], dst: [f64; 3]) -> M3 {
    let s = apply(&BRADFORD, src);
    let d = apply(&BRADFORD, dst);
    let scale: M3 = [[d[0] / s[0], 0.0, 0.0], [0.0, d[1] / s[1], 0.0], [0.0, 0.0, d[2] / s[2]]];
    mul(&inv(&BRADFORD), &mul(&scale, &BRADFORD))
}

// ── ICC encoding ────────────────────────────────────────────────────────────

fn s15f16(v: f64) -> [u8; 4] {
    ((v * 65536.0).round() as i32).to_be_bytes()
}

fn xyz_tag(xyz: [f64; 3]) -> Vec<u8> {
    let mut t = b"XYZ \0\0\0\0".to_vec();
    for v in xyz {
        t.extend_from_slice(&s15f16(v));
    }
    t
}

fn sf32_tag(m: &M3) -> Vec<u8> {
    let mut t = b"sf32\0\0\0\0".to_vec();
    for v in m.iter().flatten() {
        t.extend_from_slice(&s15f16(*v));
    }
    t
}

fn para_tag(kind: u16, params: &[f64]) -> Vec<u8> {
    let mut t = b"para\0\0\0\0".to_vec();
    t.extend_from_slice(&kind.to_be_bytes());
    t.extend_from_slice(&[0, 0]);
    for &v in params {
        t.extend_from_slice(&s15f16(v));
    }
    t
}

/// A one-record (en-US) multiLocalizedUnicode tag.
fn mluc_tag(s: &str) -> Vec<u8> {
    let utf16: Vec<u8> = s.encode_utf16().flat_map(u16::to_be_bytes).collect();
    let mut t = b"mluc\0\0\0\0".to_vec();
    t.extend_from_slice(&1u32.to_be_bytes()); // records
    t.extend_from_slice(&12u32.to_be_bytes()); // record size
    t.extend_from_slice(b"enUS");
    t.extend_from_slice(&(utf16.len() as u32).to_be_bytes());
    t.extend_from_slice(&28u32.to_be_bytes()); // offset from the tag start
    t.extend_from_slice(&utf16);
    t
}

fn build_profile(tags: &[([u8; 4], Vec<u8>)]) -> Vec<u8> {
    let table_len = 4 + 12 * tags.len();
    let mut data = Vec::new();
    let mut table = Vec::with_capacity(table_len);
    table.extend_from_slice(&(tags.len() as u32).to_be_bytes());
    // Identical tag data (the three TRCs) is stored once and shared.
    let mut placed: Vec<(&Vec<u8>, u32)> = Vec::new();
    for (sig, body) in tags {
        let offset = match placed.iter().find(|(b, _)| *b == body) {
            Some(&(_, off)) => off,
            None => {
                let off = (128 + table_len + data.len()) as u32;
                data.extend_from_slice(body);
                while data.len() % 4 != 0 {
                    data.push(0);
                }
                placed.push((body, off));
                off
            }
        };
        table.extend_from_slice(sig);
        table.extend_from_slice(&offset.to_be_bytes());
        table.extend_from_slice(&(body.len() as u32).to_be_bytes());
    }

    let size = (128 + table_len + data.len()) as u32;
    let mut header = vec![0u8; 128];
    header[0..4].copy_from_slice(&size.to_be_bytes());
    header[8..12].copy_from_slice(&[4, 0x30, 0, 0]); // v4.3
    header[12..16].copy_from_slice(b"mntr");
    header[16..20].copy_from_slice(b"RGB ");
    header[20..24].copy_from_slice(b"XYZ ");
    // Creation date, fixed so a profile is byte-for-byte reproducible.
    for (i, v) in [2026u16, 1, 1, 0, 0, 0].iter().enumerate() {
        header[24 + 2 * i..26 + 2 * i].copy_from_slice(&v.to_be_bytes());
    }
    header[36..40].copy_from_slice(b"acsp");
    for (i, v) in PCS_D50.iter().enumerate() {
        header[68 + 4 * i..72 + 4 * i].copy_from_slice(&s15f16(*v));
    }

    let mut out = header;
    out.extend_from_slice(&table);
    out.extend_from_slice(&data);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn be32(b: &[u8], at: usize) -> u32 {
        u32::from_be_bytes(b[at..at + 4].try_into().unwrap())
    }

    fn fixed(b: &[u8], at: usize) -> f64 {
        i32::from_be_bytes(b[at..at + 4].try_into().unwrap()) as f64 / 65536.0
    }

    /// (offset, size) of tag `sig`.
    fn tag(p: &[u8], sig: &[u8; 4]) -> Option<(usize, usize)> {
        (0..be32(p, 128) as usize).map(|i| 132 + 12 * i).find_map(|e| {
            (&p[e..e + 4] == sig).then(|| (be32(p, e + 4) as usize, be32(p, e + 8) as usize))
        })
    }

    #[test]
    fn white_maps_to_white_in_every_space() {
        for space in OutputSpace::ALL {
            let w = crate::color::mat3_apply(&space.srgb_to_space_matrix(), [1.0; 3]);
            assert!(w.iter().all(|v| (v - 1.0).abs() < 1e-4), "{space}: {w:?}");
        }
    }

    #[test]
    fn srgb_output_is_the_exact_identity() {
        let rgb = [0.123_456, 0.654_321, 0.5];
        assert_eq!(crate::color::mat3_apply(&OutputSpace::Srgb.srgb_to_space_matrix(), rgb), rgb);
    }

    /// The colour pass reaches sRGB from ProPhoto with a fixed matrix (the
    /// DCP path's `prophoto_to_srgb`, column-major in the shader); the
    /// ProPhoto export must undo exactly that.
    #[test]
    fn prophoto_output_inverts_the_dcp_paths_prophoto_to_srgb() {
        let pp_to_srgb: M3 = [
            [2.034_075_8, -0.727_334_1, -0.306_741_8],
            [-0.228_813_1, 1.231_730_1, -0.002_916_8],
            [-0.008_569_8, -0.153_286_6, 1.161_856_4],
        ];
        let ours = OutputSpace::ProPhoto.srgb_to_space_matrix();
        let round_trip = mul(&inv(&pp_to_srgb), &[[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]]);
        for i in 0..9 {
            assert!((ours[i] as f64 - round_trip[i / 3][i % 3]).abs() < 2e-3, "{i}: {ours:?}");
        }
    }

    #[test]
    fn srgb_primaries_are_inside_every_wider_space() {
        for space in [OutputSpace::DisplayP3, OutputSpace::AdobeRgb, OutputSpace::ProPhoto] {
            let m = space.srgb_to_space_matrix();
            for p in [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]] {
                let v = crate::color::mat3_apply(&m, p);
                assert!(v.iter().all(|&c| (-1e-4..=1.0 + 1e-4).contains(&c)), "{space}: {p:?} → {v:?}");
            }
        }
    }

    #[test]
    fn profile_is_well_formed() {
        for space in OutputSpace::ALL {
            let p = space.icc_profile();
            assert_eq!(be32(&p, 0) as usize, p.len(), "{space}: size field");
            assert_eq!(&p[36..40], b"acsp");
            assert_eq!(p.len() % 4, 0);
            for sig in [b"desc", b"cprt", b"wtpt", b"rXYZ", b"gXYZ", b"bXYZ", b"rTRC", b"gTRC", b"bTRC", b"chad"] {
                let (off, size) = tag(&p, sig).unwrap_or_else(|| panic!("{space}: no {sig:?}"));
                assert_eq!(off % 4, 0);
                assert!(off + size <= p.len());
            }
        }
    }

    /// Validators check that the adapted primaries sum to the PCS white.
    #[test]
    fn profile_primaries_sum_to_the_pcs_white() {
        for space in OutputSpace::ALL {
            let p = space.icc_profile();
            let mut sum = [0.0; 3];
            for sig in [b"rXYZ", b"gXYZ", b"bXYZ"] {
                let (off, _) = tag(&p, sig).unwrap();
                for (i, s) in sum.iter_mut().enumerate() {
                    *s += fixed(&p, off + 8 + 4 * i);
                }
            }
            for i in 0..3 {
                assert!((sum[i] - PCS_D50[i]).abs() < 1e-4, "{space}: {sum:?}");
            }
        }
    }

    /// Published D50-adapted sRGB primaries (the ICC's sRGB v4 profile).
    #[test]
    fn srgb_profile_matches_the_reference_primaries() {
        let p = OutputSpace::Srgb.icc_profile();
        let (off, _) = tag(&p, b"rXYZ").unwrap();
        let r = [0, 1, 2].map(|i| fixed(&p, off + 8 + 4 * i));
        for (got, want) in r.iter().zip([0.4361, 0.2225, 0.0139]) {
            assert!((got - want).abs() < 2e-4, "{r:?}");
        }
    }

    /// Writes the four profiles to `$ICC_DIR`, for checking them with an
    /// outside implementation (LittleCMS, a validator, a viewer).
    #[test]
    #[ignore]
    fn write_profiles() {
        let Ok(dir) = std::env::var("ICC_DIR") else {
            return;
        };
        for space in OutputSpace::ALL {
            let name = format!("{dir}/{}.icc", space.to_string().replace([' ', '(', ')'], ""));
            std::fs::write(&name, space.icc_profile()).unwrap();
        }
    }

    /// The curve in the profile must decode what `encode` (the shader's
    /// curve) wrote.
    #[test]
    fn profile_curve_inverts_the_encoding() {
        for space in OutputSpace::ALL {
            let (kind, g) = space.trc();
            let decode = |x: f64| match kind {
                0 => x.powf(g[0]),
                _ => {
                    if x >= g[4] {
                        (g[1] * x + g[2]).powf(g[0])
                    } else {
                        g[3] * x
                    }
                }
            };
            for i in 0..=100 {
                let v = i as f32 / 100.0;
                let back = decode(space.encode(v) as f64);
                assert!((back - v as f64).abs() < 1e-4, "{space}: {v} → {back}");
            }
        }
    }
}
