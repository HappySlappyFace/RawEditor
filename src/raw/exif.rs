//! Carrying the RAW's EXIF into exported JPEGs.
//!
//! Without it an export has no capture date, camera, lens or exposure: photo
//! libraries file it under the export date and show nothing in the info
//! panel. Only the capture facts are copied — not the vendor MakerNote, which
//! is large, undocumented, and full of offsets that break once moved.

use exif::{Context, Field, In, Reader, Tag, Value};
use std::io::Cursor;
use std::path::Path;

/// Primary-image fields worth keeping. GPS fields are kept wholesale (see
/// `export_app1`).
const KEEP: &[Tag] = &[
    // Camera and authorship
    Tag::Make,
    Tag::Model,
    Tag::BodySerialNumber,
    Tag::Artist,
    Tag::Copyright,
    // When
    Tag::DateTime,
    Tag::DateTimeOriginal,
    Tag::DateTimeDigitized,
    Tag::SubSecTime,
    Tag::SubSecTimeOriginal,
    Tag::SubSecTimeDigitized,
    Tag::OffsetTime,
    Tag::OffsetTimeOriginal,
    Tag::OffsetTimeDigitized,
    // Exposure
    Tag::ExposureTime,
    Tag::FNumber,
    Tag::ExposureProgram,
    Tag::PhotographicSensitivity,
    Tag::ExposureBiasValue,
    Tag::MeteringMode,
    Tag::Flash,
    Tag::ExposureMode,
    Tag::WhiteBalance,
    // Lens
    Tag::FocalLength,
    Tag::FocalLengthIn35mmFilm,
    Tag::LensMake,
    Tag::LensModel,
    Tag::LensSpecification,
    // Required for a valid Exif sub-IFD
    Tag::ExifVersion,
];

/// The payload of a JPEG APP1 segment (`Exif\0\0` + TIFF structure) carrying
/// the capture metadata of the RAW at `raw_path`, or `None` when it has no
/// readable EXIF.
///
/// Orientation is written as 1 (upright): the export bakes the rotation into
/// the pixels, and repeating the camera's tag would make viewers rotate it a
/// second time. `color_space` is the export's EXIF ColorSpace (1 = sRGB,
/// 0xFFFF = uncalibrated, see `OutputSpace::exif_color_space`) — the
/// camera's own value describes its JPEG, not this file.
pub fn export_app1(raw_path: &Path, color_space: u16) -> Option<Vec<u8>> {
    let file = std::fs::File::open(raw_path).ok()?;
    let exif = Reader::new()
        .read_from_container(&mut std::io::BufReader::new(file))
        .ok()?;
    app1_from(&exif, color_space)
}

fn app1_from(exif: &exif::Exif, color_space: u16) -> Option<Vec<u8>> {
    let kept: Vec<&Field> = exif
        .fields()
        .filter(|f| f.ifd_num == In::PRIMARY)
        .filter(|f| f.tag.context() == Context::Gps || KEEP.contains(&f.tag))
        .collect();
    if kept.is_empty() {
        return None;
    }

    let orientation = Field {
        tag: Tag::Orientation,
        ifd_num: In::PRIMARY,
        value: Value::Short(vec![1]),
    };
    let software = Field {
        tag: Tag::Software,
        ifd_num: In::PRIMARY,
        value: Value::Ascii(vec![format!("RAW Editor {}", env!("CARGO_PKG_VERSION")).into_bytes()]),
    };
    let space = Field {
        tag: Tag::ColorSpace,
        ifd_num: In::PRIMARY,
        value: Value::Short(vec![color_space]),
    };
    let version = Field {
        tag: Tag::ExifVersion,
        ifd_num: In::PRIMARY,
        value: Value::Undefined(b"0230".to_vec(), 0),
    };

    let mut writer = exif::experimental::Writer::new();
    for field in &kept {
        writer.push_field(field);
    }
    writer.push_field(&orientation);
    writer.push_field(&software);
    writer.push_field(&space);
    if !kept.iter().any(|f| f.tag == Tag::ExifVersion) {
        writer.push_field(&version);
    }

    let mut tiff = Cursor::new(Vec::new());
    if let Err(e) = writer.write(&mut tiff, true) {
        tracing::warn!("Could not encode EXIF for export: {e}");
        return None;
    }
    let mut app1 = b"Exif\0\0".to_vec();
    app1.extend_from_slice(tiff.get_ref());
    // A JPEG segment's length field is 16 bits and counts itself.
    if app1.len() > u16::MAX as usize - 2 {
        tracing::warn!("EXIF block too large for one APP1 segment; exporting without it");
        return None;
    }
    Some(app1)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ascii(tag: Tag, s: &str) -> Field {
        Field { tag, ifd_num: In::PRIMARY, value: Value::Ascii(vec![s.as_bytes().to_vec()]) }
    }

    /// Build a source EXIF block with a mix of kept and dropped fields, run it
    /// through `app1_from`, and read the result back.
    #[test]
    fn keeps_capture_facts_drops_maker_data_and_resets_orientation() {
        let fields = [
            ascii(Tag::Make, "NIKON CORPORATION"),
            ascii(Tag::DateTimeOriginal, "2024:05:01 10:20:30"),
            Field { tag: Tag::Orientation, ifd_num: In::PRIMARY, value: Value::Short(vec![6]) },
            Field { tag: Tag::MakerNote, ifd_num: In::PRIMARY, value: Value::Undefined(vec![7; 64], 0) },
            Field {
                tag: Tag::FNumber,
                ifd_num: In::PRIMARY,
                value: Value::Rational(vec![exif::Rational { num: 56, denom: 10 }]),
            },
        ];
        let mut writer = exif::experimental::Writer::new();
        for f in &fields {
            writer.push_field(f);
        }
        let mut src = Cursor::new(Vec::new());
        writer.write(&mut src, true).unwrap();
        let source = Reader::new().read_raw(src.into_inner()).unwrap();

        let app1 = app1_from(&source, 0xFFFF).expect("has keepable fields");
        assert!(app1.starts_with(b"Exif\0\0"));
        let out = Reader::new().read_raw(app1[6..].to_vec()).unwrap();

        let get = |tag| out.get_field(tag, In::PRIMARY);
        assert_eq!(
            get(Tag::DateTimeOriginal).unwrap().display_value().to_string(),
            "2024-05-01 10:20:30"
        );
        assert!(get(Tag::Make).is_some());
        assert!(get(Tag::FNumber).is_some());
        assert_eq!(get(Tag::Orientation).unwrap().value.get_uint(0), Some(1));
        assert!(get(Tag::MakerNote).is_none(), "vendor data must not be copied");
        assert!(get(Tag::Software).is_some());
        assert_eq!(get(Tag::ColorSpace).unwrap().value.get_uint(0), Some(0xFFFF));
    }

    #[test]
    fn nothing_worth_keeping_means_no_segment() {
        let fields = [Field {
            tag: Tag::MakerNote,
            ifd_num: In::PRIMARY,
            value: Value::Undefined(vec![1, 2, 3], 0),
        }];
        let mut writer = exif::experimental::Writer::new();
        for f in &fields {
            writer.push_field(f);
        }
        let mut src = Cursor::new(Vec::new());
        writer.write(&mut src, true).unwrap();
        let source = Reader::new().read_raw(src.into_inner()).unwrap();
        assert!(app1_from(&source, 1).is_none());
    }
}
