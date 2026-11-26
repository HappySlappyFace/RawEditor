//! Ultra HDR JPEG (Android's JPEG_R, read by Google Photos, Chrome and
//! Android 14+): an ordinary SDR JPEG that every viewer shows as before,
//! followed by a grey JPEG, the GAIN MAP, which an HDR display multiplies
//! the SDR image by (`core::hdr`). Two pieces of metadata tie them
//! together:
//!
//! - XMP in each image (Adobe's gain-map namespace, `hdrgm`): in the
//!   primary, a container directory naming the gain map and its length; in
//!   the gain map, how its values map to gains;
//! - an MPF index (CIPA DC-007, APP2) in the primary: where each image
//!   starts and how long it is.
//!
//! The gain map stores `log2(gain) / log2(headroom)`, gamma 1, offsets 0:
//! a display reconstructs `HDR = SDR × gain`, exactly what the screen
//! showed.

/// Quality of the gain map's own JPEG. It is smooth and small (a quarter
/// of the image's width and height), so this costs little.
pub const GAIN_MAP_QUALITY: u8 = 85;

/// The gain map is this many times smaller than the image on each side.
pub const GAIN_MAP_SCALE: u32 = 4;

const XMP_NAMESPACE: &[u8] = b"http://ns.adobe.com/xap/1.0/\0";

fn xmp(description: &str) -> Vec<u8> {
    let packet = format!(
        "<x:xmpmeta xmlns:x=\"adobe:ns:meta/\" x:xmptk=\"RawEditor\">\
         <rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">{description}</rdf:RDF>\
         </x:xmpmeta>"
    );
    [XMP_NAMESPACE, packet.as_bytes()].concat()
}

/// The primary image's XMP (an APP1 payload): the container directory.
pub fn primary_xmp(gain_map_len: usize) -> Vec<u8> {
    xmp(&format!(
        "<rdf:Description \
         xmlns:Container=\"http://ns.google.com/photos/1.0/container/\" \
         xmlns:Item=\"http://ns.google.com/photos/1.0/container/item/\" \
         xmlns:hdrgm=\"http://ns.adobe.com/hdr-gain-map/1.0/\" \
         hdrgm:Version=\"1.0\">\
         <Container:Directory><rdf:Seq>\
         <rdf:li rdf:parseType=\"Resource\">\
         <Container:Item Item:Semantic=\"Primary\" Item:Mime=\"image/jpeg\"/></rdf:li>\
         <rdf:li rdf:parseType=\"Resource\">\
         <Container:Item Item:Semantic=\"GainMap\" Item:Mime=\"image/jpeg\" Item:Length=\"{gain_map_len}\"/></rdf:li>\
         </rdf:Seq></Container:Directory>\
         </rdf:Description>"
    ))
}

/// The gain map's XMP (an APP1 payload): gains from 1 to `headroom`, the
/// whole of it applied on a display that has it.
pub fn gain_map_xmp(headroom: f32) -> Vec<u8> {
    let stops = headroom.max(1.0).log2();
    xmp(&format!(
        "<rdf:Description xmlns:hdrgm=\"http://ns.adobe.com/hdr-gain-map/1.0/\" \
         hdrgm:Version=\"1.0\" hdrgm:GainMapMin=\"0\" hdrgm:GainMapMax=\"{stops}\" \
         hdrgm:Gamma=\"1\" hdrgm:OffsetSDR=\"0\" hdrgm:OffsetHDR=\"0\" \
         hdrgm:HDRCapacityMin=\"0\" hdrgm:HDRCapacityMax=\"{stops}\" \
         hdrgm:BaseRenditionIsHDR=\"False\"/>"
    ))
}

/// Length of the MPF segment [`mpf_segment`] writes, marker included.
const MPF_LEN: usize = 90;

/// The MPF APP2 segment: two images, the primary at 0 and the gain map at
/// `gain_map_offset`, which MPF counts from the start of this segment's
/// TIFF header (8 bytes in).
fn mpf_segment(primary_len: u32, gain_map_len: u32, gain_map_offset: u32) -> Vec<u8> {
    let mut v = Vec::with_capacity(MPF_LEN);
    v.extend([0xFF, 0xE2]);
    v.extend(((MPF_LEN - 2) as u16).to_be_bytes());
    v.extend(b"MPF\0");
    // TIFF header, big-endian, first IFD right after it.
    v.extend(b"MM\0\x2A");
    v.extend(8u32.to_be_bytes());
    let entry = |v: &mut Vec<u8>, tag: u16, kind: u16, count: u32, value: [u8; 4]| {
        v.extend(tag.to_be_bytes());
        v.extend(kind.to_be_bytes());
        v.extend(count.to_be_bytes());
        v.extend(value);
    };
    const UNDEFINED: u16 = 7;
    const LONG: u16 = 4;
    v.extend(3u16.to_be_bytes());
    entry(&mut v, 0xB000, UNDEFINED, 4, *b"0100"); // MPFVersion
    entry(&mut v, 0xB001, LONG, 1, 2u32.to_be_bytes()); // NumberOfImages
    // MPEntry: two 16-byte entries, after this IFD (header 8 + count 2 +
    // three entries 36 + next-IFD 4).
    entry(&mut v, 0xB002, UNDEFINED, 32, 50u32.to_be_bytes());
    v.extend(0u32.to_be_bytes()); // no next IFD
    // Primary: baseline MP primary image (0x030000), JPEG.
    v.extend(0x0003_0000u32.to_be_bytes());
    v.extend(primary_len.to_be_bytes());
    v.extend(0u32.to_be_bytes());
    v.extend([0; 4]);
    // Gain map.
    v.extend(0u32.to_be_bytes());
    v.extend(gain_map_len.to_be_bytes());
    v.extend(gain_map_offset.to_be_bytes());
    v.extend([0; 4]);
    debug_assert_eq!(v.len(), MPF_LEN);
    v
}

/// Where the run of APPn segments after SOI ends: MPF goes there, after
/// the EXIF, XMP and ICC segments a reader looks for first.
fn after_app_segments(jpeg: &[u8]) -> Result<usize, String> {
    if jpeg.get(..2) != Some(&[0xFF, 0xD8]) {
        return Err("not a JPEG".into());
    }
    let mut at = 2;
    while let Some(&[0xFF, marker, hi, lo, ..]) = jpeg.get(at..) {
        if !(0xE0..=0xEF).contains(&marker) {
            return Ok(at);
        }
        at += 2 + u16::from_be_bytes([hi, lo]) as usize;
    }
    Err("the JPEG ends inside its header".into())
}

/// The Ultra HDR file: `primary` (an SDR JPEG already carrying
/// [`primary_xmp`] for this gain map) with an MPF index added, followed by
/// `gain_map` (a JPEG carrying [`gain_map_xmp`]).
pub fn assemble(primary: &[u8], gain_map: &[u8]) -> Result<Vec<u8>, String> {
    let at = after_app_segments(primary)?;
    let primary_len = primary.len() + MPF_LEN;
    let tiff_header = at + 8;
    let too_big = |_| "the image is too large for Ultra HDR (4 GB)".to_string();
    let mpf = mpf_segment(
        u32::try_from(primary_len).map_err(too_big)?,
        u32::try_from(gain_map.len()).map_err(too_big)?,
        u32::try_from(primary_len - tiff_header).map_err(too_big)?,
    );
    let mut out = Vec::with_capacity(primary_len + gain_map.len());
    out.extend_from_slice(&primary[..at]);
    out.extend_from_slice(&mpf);
    out.extend_from_slice(&primary[at..]);
    out.extend_from_slice(gain_map);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn jpeg(width: u16, height: u16, color: jpeg_encoder::ColorType, xmp: &[u8]) -> Vec<u8> {
        let channels = match color {
            jpeg_encoder::ColorType::Luma => 1,
            _ => 3,
        };
        let pixels = vec![128u8; width as usize * height as usize * channels];
        let mut out = Vec::new();
        let mut encoder = jpeg_encoder::Encoder::new(&mut out, 90);
        encoder.add_app_segment(1, xmp).expect("xmp");
        encoder.encode(&pixels, width, height, color).expect("encode");
        out
    }

    fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
        haystack.windows(needle.len()).position(|w| w == needle)
    }

    #[test]
    fn mpf_points_at_the_gain_map() {
        let gain_map = jpeg(8, 6, jpeg_encoder::ColorType::Luma, &gain_map_xmp(4.0));
        let primary = jpeg(32, 24, jpeg_encoder::ColorType::Rgb, &primary_xmp(gain_map.len()));
        let file = assemble(&primary, &gain_map).expect("assemble");
        assert_eq!(file.len(), primary.len() + MPF_LEN + gain_map.len());

        let mpf = find(&file, b"MPF\0").expect("an MPF segment");
        let tiff = mpf + 4;
        let entries = tiff + 50;
        let be = |at: usize| u32::from_be_bytes(file[at..at + 4].try_into().unwrap());
        let (primary_len, primary_offset) = (be(entries + 4) as usize, be(entries + 8));
        let (gain_len, gain_offset) = (be(entries + 20) as usize, be(entries + 24) as usize);
        assert_eq!(primary_offset, 0);
        assert_eq!(primary_len, primary.len() + MPF_LEN);
        assert_eq!(gain_len, gain_map.len());
        assert_eq!(&file[tiff + gain_offset..], &gain_map[..], "MPF's offset is not the gain map");
        // The primary is still a well-formed JPEG ending where MPF says.
        assert_eq!(&file[primary_len - 2..primary_len], &[0xFF, 0xD9]);
    }

    #[test]
    fn mpf_goes_after_the_app_segments() {
        let gain_map = jpeg(4, 4, jpeg_encoder::ColorType::Luma, &gain_map_xmp(4.0));
        let primary = jpeg(16, 16, jpeg_encoder::ColorType::Rgb, &primary_xmp(gain_map.len()));
        let file = assemble(&primary, &gain_map).expect("assemble");
        let mpf = find(&file, b"MPF\0").expect("MPF") - 4;
        let xmp = find(&file, XMP_NAMESPACE).expect("XMP");
        assert!(xmp < mpf, "MPF before the XMP");
        assert_eq!(file[mpf + MPF_LEN], 0xFF);
        assert!(!(0xE0..=0xEF).contains(&file[mpf + MPF_LEN + 1]), "an APP segment after MPF");
    }

    #[test]
    fn metadata_names_the_gain_map_and_its_range() {
        let primary = String::from_utf8_lossy(&primary_xmp(1234)).into_owned();
        assert!(primary.contains("Item:Semantic=\"GainMap\""));
        assert!(primary.contains("Item:Length=\"1234\""));
        assert!(primary.contains("hdrgm:Version=\"1.0\""));
        let gain = String::from_utf8_lossy(&gain_map_xmp(4.0)).into_owned();
        assert!(gain.contains("hdrgm:GainMapMax=\"2\""));
        assert!(gain.contains("hdrgm:HDRCapacityMax=\"2\""));
        assert!(gain.contains("hdrgm:BaseRenditionIsHDR=\"False\""));
    }

    #[test]
    fn a_non_jpeg_is_refused() {
        assert!(assemble(b"not a jpeg", &[]).is_err());
        assert!(assemble(&[0xFF, 0xD8, 0xFF, 0xE1, 0x00], &[]).is_err());
    }
}
