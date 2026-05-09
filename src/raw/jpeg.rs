use std::fs::File;
use std::io::Read;
use std::path::Path;

/// Extract the largest embedded JPEG byte sequence from a RAW file.
///
/// If `validator` is provided, each candidate JPEG is validated before
/// participating in the "largest" selection.
pub fn extract_largest_jpeg(
    raw_path: &Path,
    validator: Option<fn(&[u8]) -> bool>,
) -> Result<Option<Vec<u8>>, String> {
    let mut file = File::open(raw_path).map_err(|e| format!("Failed to open RAW file: {}", e))?;

    let mut buffer = Vec::new();
    file.read_to_end(&mut buffer)
        .map_err(|e| format!("Failed to read RAW file: {}", e))?;

    Ok(scan_for_largest_jpeg(&buffer, validator))
}

fn scan_for_largest_jpeg(
    buffer: &[u8],
    validator: Option<fn(&[u8]) -> bool>,
) -> Option<Vec<u8>> {
    if buffer.len() < 3 {
        return None;
    }

    let jpeg_start = b"\xff\xd8\xff"; // SOI + first marker byte
    let jpeg_end = b"\xff\xd9"; // EOI

    let mut largest_jpeg: Option<Vec<u8>> = None;
    let mut largest_size = 0usize;
    let mut pos = 0usize;

    while pos + 3 <= buffer.len() {
        if buffer[pos..].starts_with(jpeg_start) {
            if let Some(end_pos) = buffer[pos..]
                .windows(2)
                .position(|w| w == jpeg_end)
                .map(|p| pos + p + 2)
            {
                let candidate = buffer[pos..end_pos].to_vec();
                let is_valid = validator.map(|check| check(&candidate)).unwrap_or(true);

                if is_valid && candidate.len() > largest_size {
                    largest_size = candidate.len();
                    largest_jpeg = Some(candidate);
                }

                pos = end_pos;
            } else {
                pos += 1;
            }
        } else {
            pos += 1;
        }
    }

    largest_jpeg
}

#[cfg(test)]
mod tests {
    use super::scan_for_largest_jpeg;

    #[test]
    fn picks_largest_jpeg_candidate() {
        let small = [0xff, 0xd8, 0xff, 0x01, 0x02, 0xff, 0xd9];
        let large = [0xff, 0xd8, 0xff, 0x01, 0x02, 0x03, 0x04, 0xff, 0xd9];
        let mut bytes = vec![0x00, 0x11, 0x22];
        bytes.extend_from_slice(&small);
        bytes.extend_from_slice(&[0xaa, 0xbb]);
        bytes.extend_from_slice(&large);
        bytes.extend_from_slice(&[0x33, 0x44]);

        let largest = scan_for_largest_jpeg(&bytes, None).expect("expected embedded jpeg");
        assert_eq!(largest, large.to_vec());
    }

    #[test]
    fn applies_validator_when_provided() {
        let first = [0xff, 0xd8, 0xff, 0x01, 0x02, 0xff, 0xd9];
        let second = [0xff, 0xd8, 0xff, 0x03, 0x04, 0x05, 0xff, 0xd9];
        let mut bytes = vec![];
        bytes.extend_from_slice(&first);
        bytes.extend_from_slice(&second);

        fn validator(candidate: &[u8]) -> bool {
            candidate.get(3).copied() == Some(0x03)
        }

        let largest =
            scan_for_largest_jpeg(&bytes, Some(validator)).expect("expected validated jpeg");
        assert_eq!(largest, second.to_vec());
    }
}
