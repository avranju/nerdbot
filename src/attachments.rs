//! Shared HEIC/HEIF detection and conversion helpers.
//!
//! This module provides:
//! - MIME type and filename detection for HEIC/HEIF
//! - ftyp brand inspection for binary data
//! - JPEG output filename generation
//! - One-time libheif image hook registration
//! - In-memory HEIC-to-JPEG conversion

use std::sync::OnceLock;

use image::codecs::jpeg::JpegEncoder;

// ── HEIC MIME type detection ─────────────────────────────────────────

/// HEIC/HEIF MIME types that indicate a HEIC image.
const HEIC_MIME_TYPES: &[&str] = &[
    "image/heic",
    "image/heif",
    "image/heic-sequence",
    "image/heif-sequence",
];

/// Check if a MIME type is a HEIC/HEIF variant.
pub fn is_heic_mime(mime: &str) -> bool {
    HEIC_MIME_TYPES.contains(&mime)
}

// ── HEIC filename detection ──────────────────────────────────────────

/// Check if a filename ends with .heic or .heif (case-insensitive).
pub fn is_heic_filename(filename: &str) -> bool {
    let lower = filename.to_lowercase();
    lower.ends_with(".heic") || lower.ends_with(".heif")
}

// ── HEIC signature inspection ────────────────────────────────────────

/// HEIC/HEIF ftyp brand strings to look for at offset 4.
#[allow(dead_code)]
const HEIC_FTIPL_BRANDS: &[&[u8]] = &[b"heic", b"heix", b"hevc", b"hevx", b"mif1", b"msf1"];

/// Inspect the first bytes of a buffer to detect HEIC/HEIF format.
///
/// Looks for the ISO BMFF `ftyp` box header followed by a known HEIC brand.
/// Returns the detected brand string if found, `None` otherwise.
pub fn inspect_heic_signature(data: &[u8]) -> Option<&'static str> {
    // Need at least 12 bytes: size(4) + "ftyp"(4) + brand(4)
    if data.len() < 12 {
        return None;
    }

    // Check for ftyp box at offset 4
    if data[4..8] != *b"ftyp" {
        return None;
    }

    // Check the brand at offset 8-11
    let brand_bytes = &data[8..12];
    if brand_bytes == b"heic" {
        return Some("heic");
    }
    if brand_bytes == b"heix" {
        return Some("heix");
    }
    if brand_bytes == b"hevc" {
        return Some("hevc");
    }
    if brand_bytes == b"hevx" {
        return Some("hevx");
    }
    if brand_bytes == b"mif1" {
        return Some("mif1");
    }
    if brand_bytes == b"msf1" {
        return Some("msf1");
    }

    None
}

// ── JPEG filename generation ─────────────────────────────────────────

/// Generate a safe JPEG filename by replacing .heic/.heif with .jpg.
///
/// If the filename doesn't end with .heic/.heif, appends .jpg instead.
pub fn jpeg_filename_for(filename: Option<&str>) -> Option<String> {
    let name = filename?;
    let lower = name.to_lowercase();

    if lower.ends_with(".heic") || lower.ends_with(".heif") {
        Some(name[..name.len() - 5].to_string() + ".jpg")
    } else {
        Some(format!("{name}.jpg"))
    }
}

// ── Conversion result ────────────────────────────────────────────────

/// Result of converting a HEIC/HEIF image to JPEG.
pub struct ConvertedHeicImage {
    /// JPEG-encoded bytes.
    pub bytes: Vec<u8>,
    /// MIME type (always "image/jpeg").
    pub mime_type: String,
    /// Output filename (with .jpg extension).
    pub filename: Option<String>,
}

// ── libheif hook registration ────────────────────────────────────────

/// One-time registration of libheif image hooks with the `image` crate.
/// This must be called before any HEIC decoding via the image crate.
static HOOK_REGISTRAR: OnceLock<Result<(), String>> = OnceLock::new();

fn ensure_hooks_registered() -> Result<(), String> {
    // The OnceLock stores a Result<(), String>. We need to get a reference
    // to the stored value and clone it (since Result<(), String> doesn't implement Copy).
    let result = HOOK_REGISTRAR.get_or_init(|| {
        // Register libheif image hooks for HEIC/HEIF decoding.
        // This registers hooks with the `image` crate so that
        // `image::io::Reader` can automatically decode HEIC/HEIF files.
        libheif_rs::integration::image::register_heic_decoding_hook();
        libheif_rs::integration::image::register_heif_decoding_hook();
        Ok(())
    });
    // Clone the Result to return it
    result.clone()
}

// ── HEIC to JPEG conversion ──────────────────────────────────────────

/// Convert HEIC/HEIF bytes to JPEG in memory.
///
/// Uses libheif to decode the HEIC/HEIF data and libjpeg to encode
/// the result as a JPEG. Returns the JPEG bytes and metadata.
///
/// # Arguments
/// * `data` — Raw HEIC/HEIF file bytes.
/// * `original_filename` — Original filename for output naming.
///
/// # Returns
/// `Ok(ConvertedHeicImage)` with JPEG bytes, or `Err` with an error message.
///
/// # Errors
/// Returns an error if:
/// - libheif hooks are not registered
/// - The data is not valid HEIC/HEIF
/// - Decoding fails
/// - JPEG encoding fails
pub fn convert_heic_to_jpeg(
    data: &[u8],
    original_filename: Option<&str>,
) -> Result<ConvertedHeicImage, String> {
    ensure_hooks_registered()?;

    // Decode HEIC/HEIF using the image crate (hooks registered by libheif-rs)
    let img = image::load_from_memory(data)
        .map_err(|e| format!("Failed to decode HEIC/HEIF image: {e}"))?;

    // Convert to RGB for JPEG encoding (removes alpha if present)
    let rgb_image = img.to_rgb8();

    // Encode as JPEG
    let mut jpeg_output = Vec::new();
    let mut encoder = JpegEncoder::new_with_quality(&mut jpeg_output, 90);
    encoder
        .encode(
            rgb_image.as_raw(),
            rgb_image.width(),
            rgb_image.height(),
            image::ExtendedColorType::Rgb8,
        )
        .map_err(|e| format!("Failed to encode JPEG: {e}"))?;

    Ok(ConvertedHeicImage {
        bytes: jpeg_output,
        mime_type: "image/jpeg".to_string(),
        filename: jpeg_filename_for(original_filename),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── MIME type tests ──────────────────────────────────────────────

    #[test]
    fn test_is_heic_mime_heic() {
        assert!(is_heic_mime("image/heic"));
    }

    #[test]
    fn test_is_heic_mime_heif() {
        assert!(is_heic_mime("image/heif"));
    }

    #[test]
    fn test_is_heic_mime_heic_sequence() {
        assert!(is_heic_mime("image/heic-sequence"));
    }

    #[test]
    fn test_is_heic_mime_heif_sequence() {
        assert!(is_heic_mime("image/heif-sequence"));
    }

    #[test]
    fn test_is_heic_mime_not_image() {
        assert!(!is_heic_mime("image/jpeg"));
        assert!(!is_heic_mime("image/png"));
        assert!(!is_heic_mime("application/pdf"));
    }

    // ── Filename tests ───────────────────────────────────────────────

    #[test]
    fn test_is_heic_filename_heic() {
        assert!(is_heic_filename("photo.heic"));
    }

    #[test]
    fn test_is_heic_filename_heif() {
        assert!(is_heic_filename("photo.heif"));
    }

    #[test]
    fn test_is_heic_filename_case_insensitive() {
        assert!(is_heic_filename("photo.HEIC"));
        assert!(is_heic_filename("photo.Heif"));
    }

    #[test]
    fn test_is_heic_filename_not_heic() {
        assert!(!is_heic_filename("photo.jpg"));
        assert!(!is_heic_filename("photo.png"));
        assert!(!is_heic_filename("photo.heicx"));
    }

    // ── Signature tests ──────────────────────────────────────────────

    #[test]
    fn test_inspect_heic_signature_heic_brand() {
        // size(4) + "ftyp"(4) + "heic"(4) + brand brand(4)
        let data: Vec<u8> = vec![
            0, 0, 0, 12, // size = 12
            b'f', b't', b'y', b'p', // "ftyp"
            b'h', b'e', b'i', b'c', // brand
            0, 0, 0, 0, // compatible brands
        ];
        assert_eq!(inspect_heic_signature(&data), Some("heic"));
    }

    #[test]
    fn test_inspect_heic_signature_hevc_brand() {
        let data: Vec<u8> = vec![
            0, 0, 0, 12, b'f', b't', b'y', b'p', b'h', b'e', b'v', b'c', 0, 0, 0, 0,
        ];
        assert_eq!(inspect_heic_signature(&data), Some("hevc"));
    }

    #[test]
    fn test_inspect_heic_signature_mif1_brand() {
        let data: Vec<u8> = vec![
            0, 0, 0, 12, b'f', b't', b'y', b'p', b'm', b'i', b'f', b'1', 0, 0, 0, 0,
        ];
        assert_eq!(inspect_heic_signature(&data), Some("mif1"));
    }

    #[test]
    fn test_inspect_heic_signature_no_ftyp() {
        let data: Vec<u8> = vec![0, 0, 0, 12, b'x', b't', b'y', b'p'];
        assert_eq!(inspect_heic_signature(&data), None);
    }

    #[test]
    fn test_inspect_heic_signature_too_short() {
        assert_eq!(inspect_heic_signature(b"hello"), None);
        assert_eq!(inspect_heic_signature(&[0, 0, 0, 12, b'f', b't']), None);
    }

    #[test]
    fn test_inspect_heic_signature_unknown_brand() {
        let data: Vec<u8> = vec![
            0, 0, 0, 12, b'f', b't', b'y', b'p', b'j', b'p', b'g', b' ', // unknown brand
            0, 0, 0, 0,
        ];
        assert_eq!(inspect_heic_signature(&data), None);
    }

    // ── Filename generation tests ────────────────────────────────────

    #[test]
    fn test_jpeg_filename_for_heic() {
        assert_eq!(
            jpeg_filename_for(Some("photo.heic")),
            Some("photo.jpg".to_string())
        );
    }

    #[test]
    fn test_jpeg_filename_for_heif() {
        assert_eq!(
            jpeg_filename_for(Some("photo.heif")),
            Some("photo.jpg".to_string())
        );
    }

    #[test]
    fn test_jpeg_filename_for_non_heic() {
        assert_eq!(
            jpeg_filename_for(Some("photo.png")),
            Some("photo.png.jpg".to_string())
        );
    }

    #[test]
    fn test_jpeg_filename_for_none() {
        assert_eq!(jpeg_filename_for(None), None);
    }

    // ── Conversion error tests ───────────────────────────────────────

    #[test]
    fn test_convert_heic_invalid_data() {
        // Invalid HEIC data should fail gracefully
        let result = convert_heic_to_jpeg(b"not a heic file", Some("test.heic"));
        assert!(result.is_err());
    }

    #[test]
    fn test_convert_heic_empty_data() {
        let result = convert_heic_to_jpeg(b"", Some("test.heic"));
        assert!(result.is_err());
    }
}
