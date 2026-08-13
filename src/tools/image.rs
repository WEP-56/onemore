const PNG_SIGNATURE: &[u8] = b"\x89PNG\r\n\x1a\n";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum DetectedImage {
    Supported(&'static str),
    Unsupported(&'static str),
}

pub(super) fn detect_image(bytes: &[u8]) -> Option<DetectedImage> {
    if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        return (bytes.get(3) != Some(&0xf7)).then_some(DetectedImage::Supported("image/jpeg"));
    }
    if bytes.starts_with(PNG_SIGNATURE) && is_png(bytes) {
        return Some(if is_animated_png(bytes) {
            DetectedImage::Unsupported("image/apng")
        } else {
            DetectedImage::Supported("image/png")
        });
    }
    if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        return Some(DetectedImage::Supported("image/gif"));
    }
    if bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP") {
        return Some(DetectedImage::Supported("image/webp"));
    }
    if bytes.starts_with(b"BM") && is_bmp(bytes) {
        return Some(DetectedImage::Unsupported("image/bmp"));
    }
    None
}

fn is_png(bytes: &[u8]) -> bool {
    bytes.len() >= 24
        && read_u32_be(bytes, PNG_SIGNATURE.len()) == Some(13)
        && bytes.get(12..16) == Some(b"IHDR")
}

fn is_animated_png(bytes: &[u8]) -> bool {
    let mut offset = PNG_SIGNATURE.len();
    while offset.checked_add(8).is_some_and(|end| end <= bytes.len()) {
        let Some(chunk_len) = read_u32_be(bytes, offset).map(|value| value as usize) else {
            return false;
        };
        let kind_offset = offset + 4;
        if bytes.get(kind_offset..kind_offset + 4) == Some(b"acTL") {
            return true;
        }
        if bytes.get(kind_offset..kind_offset + 4) == Some(b"IDAT") {
            return false;
        }
        let Some(next) = offset
            .checked_add(12)
            .and_then(|value| value.checked_add(chunk_len))
        else {
            return false;
        };
        if next <= offset || next > bytes.len() {
            return false;
        }
        offset = next;
    }
    false
}

fn is_bmp(bytes: &[u8]) -> bool {
    if bytes.len() < 30 {
        return false;
    }
    let (Some(declared_size), Some(pixel_offset), Some(dib_size)) = (
        read_u32_le(bytes, 2),
        read_u32_le(bytes, 10),
        read_u32_le(bytes, 14),
    ) else {
        return false;
    };
    if declared_size != 0 && declared_size < 26 {
        return false;
    }
    if pixel_offset < 14_u32.saturating_add(dib_size) {
        return false;
    }
    if declared_size != 0 && pixel_offset >= declared_size {
        return false;
    }
    let (planes, bits) = match dib_size {
        12 => (read_u16_le(bytes, 22), read_u16_le(bytes, 24)),
        40..=124 => (read_u16_le(bytes, 26), read_u16_le(bytes, 28)),
        _ => return false,
    };
    planes == Some(1) && matches!(bits, Some(1 | 4 | 8 | 16 | 24 | 32))
}

fn read_u16_le(bytes: &[u8], offset: usize) -> Option<u16> {
    Some(u16::from_le_bytes(
        bytes.get(offset..offset + 2)?.try_into().ok()?,
    ))
}

fn read_u32_le(bytes: &[u8], offset: usize) -> Option<u32> {
    Some(u32::from_le_bytes(
        bytes.get(offset..offset + 4)?.try_into().ok()?,
    ))
}

fn read_u32_be(bytes: &[u8], offset: usize) -> Option<u32> {
    Some(u32::from_be_bytes(
        bytes.get(offset..offset + 4)?.try_into().ok()?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png_with_chunk(kind: &[u8; 4]) -> Vec<u8> {
        let mut bytes = PNG_SIGNATURE.to_vec();
        bytes.extend_from_slice(&13_u32.to_be_bytes());
        bytes.extend_from_slice(b"IHDR");
        bytes.extend_from_slice(&[0; 17]);
        bytes.extend_from_slice(&0_u32.to_be_bytes());
        bytes.extend_from_slice(kind);
        bytes.extend_from_slice(&[0; 4]);
        bytes
    }

    #[test]
    fn detects_supported_formats_by_signature() {
        assert_eq!(
            detect_image(&[0xff, 0xd8, 0xff, 0xe0]),
            Some(DetectedImage::Supported("image/jpeg"))
        );
        assert_eq!(
            detect_image(&png_with_chunk(b"IDAT")),
            Some(DetectedImage::Supported("image/png"))
        );
        assert_eq!(
            detect_image(b"GIF89a rest"),
            Some(DetectedImage::Supported("image/gif"))
        );
        assert_eq!(
            detect_image(b"RIFFxxxxWEBPrest"),
            Some(DetectedImage::Supported("image/webp"))
        );
    }

    #[test]
    fn rejects_animated_or_malformed_png() {
        assert_eq!(
            detect_image(&png_with_chunk(b"acTL")),
            Some(DetectedImage::Unsupported("image/apng"))
        );
        assert_eq!(detect_image(PNG_SIGNATURE), None);
    }

    #[test]
    fn ignores_unknown_binary_data() {
        assert_eq!(detect_image(b"not really a png"), None);
        assert_eq!(detect_image(&[0, 1, 2, 3]), None);
    }
}
