//! Locating the camera's embedded preview JPEG inside a RAW file.
//!
//! Every format this app imports stores at least one JPEG rendering beside
//! the sensor data. The cache tiers are cut from the largest one, which is
//! far cheaper than developing the RAW itself.

/// The largest well-formed JPEG embedded anywhere in `buffer`.
///
/// Container-agnostic on purpose: rather than walking each vendor's IFD /
/// box layout, it scans for SOI markers and measures each candidate.
///
/// Only structurally valid candidates compete. Sensor data is effectively
/// random bytes, so `FF D8 FF` turns up in it by chance; measured to the next
/// stray `FF D9`, such a "JPEG" can be megabytes long. The old first-EOI scan
/// let exactly that beat a D3300's real 6000×4000 preview (`_DSC0177.NEF`),
/// and the image never got a thumbnail. First-EOI measuring survives only as a
/// last resort for files with no well-formed JPEG at all.
pub fn largest_embedded_jpeg(buffer: &[u8]) -> Option<&[u8]> {
    const SOI: &[u8] = b"\xff\xd8\xff";

    let mut largest: Option<&[u8]> = None;
    let mut fallback: Option<&[u8]> = None;
    let mut pos = 0usize;
    while let Some(offset) = buffer
        .get(pos..)
        .and_then(|rest| rest.windows(SOI.len()).position(|w| w == SOI))
    {
        let start = pos + offset;
        let rest = &buffer[start..];
        if let Some(len) = jpeg_len(rest) {
            let candidate = &rest[..len];
            if is_picture(candidate) && largest.is_none_or(|best| candidate.len() > best.len()) {
                largest = Some(candidate);
            }
            // Skip the whole image, including any thumbnail nested in it.
            pos = start + len;
        } else {
            if fallback.is_none() {
                fallback = naive_jpeg_len(rest).map(|len| &rest[..len]);
            }
            // Never skip ahead over a candidate that failed to parse: if it
            // was noise, the real preview may start inside its span.
            pos = start + 1;
        }
    }
    largest.or(fallback)
}

/// Whether a well-formed JPEG is a picture rather than sensor data: an 8-bit
/// baseline/extended/progressive DCT frame (SOF0–2) of 1 or 3 components.
///
/// Canon's CR2 stores the mosaic itself as a 12-bit *lossless* JPEG (SOF3,
/// 2 components) — well-formed, and by far the largest JPEG in the file. It
/// used to win, fail to decode, and leave the image without any cache tier.
fn is_picture(jpeg: &[u8]) -> bool {
    let mut i = 2;
    while let (Some(&0xFF), Some(&marker)) = (jpeg.get(i), jpeg.get(i + 1)) {
        if marker == 0xFF {
            i += 1;
            continue;
        }
        i += 2;
        if marker == 0x01 || (0xD0..=0xD7).contains(&marker) {
            continue;
        }
        let (Some(&hi), Some(&lo)) = (jpeg.get(i), jpeg.get(i + 1)) else {
            return false;
        };
        match marker {
            0xC0..=0xC2 => {
                let precision = jpeg.get(i + 2).copied();
                let components = jpeg.get(i + 7).copied();
                return precision == Some(8) && matches!(components, Some(1 | 3));
            }
            // Any other frame type (lossless, hierarchical, arithmetic), or a
            // scan before any frame header.
            0xC3 | 0xC5..=0xC7 | 0xC9..=0xCB | 0xCD..=0xCF | 0xDA => return false,
            _ => i += u16::from_be_bytes([hi, lo]) as usize,
        }
    }
    false
}

/// Byte length of the JPEG that starts at `buf[0]`, found by walking its
/// segment structure; `None` if it is malformed.
///
/// Stopping at the first EOI marker — what this used to do — is wrong for any
/// preview carrying an EXIF thumbnail of its own (Panasonic RW2's, for one):
/// the thumbnail's EOI sits inside the preview's APP1 segment, so the scan cut
/// the preview off a few kilobytes in and produced an undecodable fragment.
/// Following segment lengths steps over APP1 wholesale.
fn jpeg_len(buf: &[u8]) -> Option<usize> {
    if buf.get(..2)? != [0xFF, 0xD8] {
        return None;
    }
    let mut i = 2;
    loop {
        if *buf.get(i)? != 0xFF {
            return None;
        }
        // Any number of 0xFF fill bytes may precede a marker.
        while *buf.get(i)? == 0xFF {
            i += 1;
        }
        let marker = buf[i];
        i += 1;
        match marker {
            0xD9 => return Some(i), // EOI
            // Standalone markers: no length field.
            0x01 | 0xD0..=0xD7 => {}
            // A marker byte of 0x00 here is stuffing outside a scan: malformed.
            0x00 => return None,
            _ => {
                let len = u16::from_be_bytes([*buf.get(i)?, *buf.get(i + 1)?]) as usize;
                if len < 2 {
                    return None;
                }
                i += len;
                if marker == 0xDA {
                    // Start of scan: entropy-coded data follows the header and
                    // runs until a marker that is neither a stuffed 0x00 nor a
                    // restart (progressive files then continue with more
                    // segments and scans, handled by the outer loop).
                    loop {
                        let ff = i + buf.get(i..)?.iter().position(|&b| b == 0xFF)?;
                        match *buf.get(ff + 1)? {
                            0x00 | 0xD0..=0xD7 => i = ff + 2,
                            0xFF => i = ff + 1,
                            _ => {
                                i = ff;
                                break;
                            }
                        }
                    }
                }
            }
        }
    }
}

/// Last-resort measure for files with no structurally valid JPEG: up to the
/// first EOI. Wrong for nested thumbnails (see `jpeg_len`) and fooled by
/// sensor noise, but better than nothing for a quirky encoder.
fn naive_jpeg_len(buf: &[u8]) -> Option<usize> {
    buf.windows(2).position(|w| w == b"\xff\xd9").map(|p| p + 2)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A minimal structurally valid baseline JPEG: SOI, one APPn segment with
    /// `app` as payload, SOS with `scan` as entropy data, EOI.
    fn jpeg(app: &[u8], scan: &[u8]) -> Vec<u8> {
        let mut v = vec![0xFF, 0xD8, 0xFF, 0xE1];
        v.extend_from_slice(&((app.len() + 2) as u16).to_be_bytes());
        v.extend_from_slice(app);
        v.extend_from_slice(&sof(0xC0, 8, 3));
        v.extend_from_slice(&[0xFF, 0xDA, 0x00, 0x02]);
        v.extend_from_slice(scan);
        v.extend_from_slice(&[0xFF, 0xD9]);
        v
    }

    /// A frame header: `marker`, sample `precision`, a 16×16 image of
    /// `components` components.
    fn sof(marker: u8, precision: u8, components: u8) -> Vec<u8> {
        let mut v = vec![0xFF, marker];
        v.extend_from_slice(&(8 + 3 * components as u16).to_be_bytes());
        v.extend_from_slice(&[precision, 0, 16, 0, 16, components]);
        for c in 0..components {
            v.extend_from_slice(&[c + 1, 0x11, 0]);
        }
        v
    }

    /// CR2: the sensor data is a lossless JPEG far larger than the preview.
    #[test]
    fn lossless_sensor_data_never_beats_the_preview() {
        let preview = jpeg(b"exif", &[0x42; 32]);
        let mut raw = vec![0xFF, 0xD8];
        raw.extend_from_slice(&sof(0xC3, 12, 2));
        raw.extend_from_slice(&[0xFF, 0xDA, 0x00, 0x02]);
        raw.extend_from_slice(&[0x13; 500]);
        raw.extend_from_slice(&[0xFF, 0xD9]);
        assert!(jpeg_len(&raw).is_some(), "the sensor JPEG is well-formed");

        let mut bytes = preview.clone();
        bytes.extend_from_slice(&raw);
        assert_eq!(largest_embedded_jpeg(&bytes), Some(&preview[..]));
    }

    #[test]
    fn picks_largest_jpeg_candidate() {
        let small = jpeg(b"a", b"\x01\x02");
        let large = jpeg(b"abc", b"\x01\x02\x03\x04");
        let mut bytes = vec![0x00, 0x11, 0x22];
        bytes.extend_from_slice(&small);
        bytes.extend_from_slice(&[0xaa, 0xbb]);
        bytes.extend_from_slice(&large);
        bytes.extend_from_slice(&[0x33, 0x44]);

        assert_eq!(largest_embedded_jpeg(&bytes), Some(&large[..]));
    }

    /// The bug the segment walk fixes: a thumbnail inside the preview's APP1
    /// must not end the preview at the thumbnail's EOI.
    #[test]
    fn a_thumbnail_nested_in_app1_does_not_truncate_the_preview() {
        let thumb = jpeg(b"t", b"\x07");
        let preview = jpeg(&thumb, &[0x55; 64]);
        let mut bytes = vec![0x00; 8];
        bytes.extend_from_slice(&preview);
        bytes.extend_from_slice(&[0x00; 8]);

        assert_eq!(largest_embedded_jpeg(&bytes), Some(&preview[..]));
    }

    /// 0xFF00 stuffing and RSTn markers inside the scan are data, not the end.
    #[test]
    fn stuffed_bytes_and_restart_markers_stay_inside_the_scan() {
        let preview = jpeg(b"x", &[0x10, 0xFF, 0x00, 0x20, 0xFF, 0xD3, 0x30]);
        assert_eq!(jpeg_len(&preview), Some(preview.len()));
    }

    /// Sensor noise that happens to contain `FF D8 FF ... FF D9` must not
    /// outrank a real preview, however long the noise span is.
    #[test]
    fn noise_that_looks_like_a_jpeg_never_beats_a_well_formed_one() {
        let preview = jpeg(b"exif", &[0x42; 32]);
        let mut bytes = preview.clone();
        bytes.extend_from_slice(&[0xFF, 0xD8, 0xFF, 0x00]); // fake SOI in "sensor data"
        bytes.extend_from_slice(&[0x13; 500]);
        bytes.extend_from_slice(&[0xFF, 0xD9]);
        assert_eq!(largest_embedded_jpeg(&bytes), Some(&preview[..]));
    }

    /// A noise SOI just before the real preview must not swallow it.
    #[test]
    fn noise_before_a_preview_does_not_hide_it() {
        let preview = jpeg(b"exif", &[0x42; 32]);
        let mut bytes = vec![0xFF, 0xD8, 0xFF, 0x00, 0x01];
        bytes.extend_from_slice(&preview);
        assert_eq!(largest_embedded_jpeg(&bytes), Some(&preview[..]));
    }

    #[test]
    fn malformed_jpeg_falls_back_to_the_first_eoi() {
        // SOI then garbage that is not a marker: structure walk gives up.
        let bytes = [0xFF, 0xD8, 0xFF, 0x00, 0x01, 0xFF, 0xD9, 0x00];
        assert_eq!(jpeg_len(&bytes), None);
        assert_eq!(largest_embedded_jpeg(&bytes), Some(&bytes[..7]));
    }

    #[test]
    fn no_jpeg_at_all() {
        assert_eq!(largest_embedded_jpeg(b"just some bytes"), None);
        assert_eq!(largest_embedded_jpeg(b""), None);
    }
}

