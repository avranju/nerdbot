//! Telegram attachment DTOs, download, validation, and conversion helpers.
//!
//! This module handles:
//! - Telegram API types for photos, documents, and files
//! - Bounded file downloads from Telegram's CDN
//! - MIME-type and filename validation
//! - Converting supported attachments into genai `ContentPart` payloads

use genai::chat::ContentPart;
use serde::Deserialize;

use super::bot::PhotoSize;

// ── Telegram API JSON types for attachments ──────────────────────────

/// A Telegram document object.
#[derive(Debug, Clone, Deserialize, Default)]
pub struct Document {
    #[serde(default)]
    pub file_id: String,
    #[serde(default)]
    pub file_unique_id: String,
    #[serde(default)]
    pub file_size: Option<u64>,
    #[serde(default)]
    pub file_name: Option<String>,
    #[serde(default)]
    pub mime_type: Option<String>,
    #[serde(default, flatten)]
    pub thumbnail: Option<PhotoSize>,
}

/// Metadata about a downloaded attachment.
#[derive(Debug, Clone)]
pub struct AttachmentInfo {
    /// Telegram-provided filename (untrusted).
    pub filename: Option<String>,
    /// MIME type from Telegram (untrusted; may be validated further).
    pub mime_type: Option<String>,
    /// File size in bytes from Telegram metadata.
    pub size_bytes: u64,
}

// ── Supported MIME types ─────────────────────────────────────────────

/// MIME types accepted for binary (multimodal) attachments sent to the LLM.
const SUPPORTED_BINARY_MIME_TYPES: &[&str] = &[
    "image/jpeg",
    "image/png",
    "image/webp",
    "image/gif",
    "image/heic",
    "image/heif",
    "image/heic-sequence",
    "image/heif-sequence",
    "application/pdf",
];

/// MIME types accepted for text extraction (decoded and sent as text).
const SUPPORTED_TEXT_MIME_TYPES: &[&str] = &[
    "text/plain",
    "text/markdown",
    "text/csv",
    "text/html",
    "text/xml",
    "application/json",
    "application/xml",
    "application/javascript",
    "application/x-shellscript",
    "application/x-python",
];

/// Default text extensions that are treated as text documents.
const SUPPORTED_TEXT_EXTENSIONS: &[&str] = &[
    ".txt", ".md", ".json", ".csv", ".html", ".xml", ".js", ".ts", ".py", ".sh", ".bash", ".yaml",
    ".yml", ".toml", ".ini", ".cfg", ".conf", ".css", ".sql", ".log", ".rst", ".tex",
];

/// HEIC/HEIF extensions recognized as binary attachments.
const HEIC_EXTENSIONS: &[&str] = &[".heic", ".heif"];

// ── Validation helpers ───────────────────────────────────────────────

/// Check if a MIME type is supported for binary (multimodal) processing.
pub fn is_supported_binary_mime(mime: &str) -> bool {
    SUPPORTED_BINARY_MIME_TYPES.contains(&mime)
}

/// Check if a MIME type is supported for text extraction.
pub fn is_supported_text_mime(mime: &str) -> bool {
    SUPPORTED_TEXT_MIME_TYPES.contains(&mime)
}

/// Check if a filename extension suggests a text document.
pub fn is_text_extension(filename: &str) -> bool {
    let lower = filename.to_lowercase();
    SUPPORTED_TEXT_EXTENSIONS
        .iter()
        .any(|ext| lower.ends_with(ext))
}

/// Sanitize a filename for safe inclusion in prompts, logs, and markers.
///
/// - Replaces path separators, null bytes, and other dangerous characters with underscores
/// - Truncates to 200 characters
pub fn sanitize_filename(name: &str) -> String {
    name.chars()
        .map(|c| match c {
            '\0' | '/' | '\\' | '\n' | '\r' => '_',
            c => c,
        })
        .take(200)
        .collect()
}

/// Inspect the first bytes of a buffer to infer the actual MIME type.
///
/// Supports magic-byte detection for PNG, JPEG, GIF, WebP, PDF, and HEIC/HEIF.
/// Returns `None` when no magic bytes are recognized (pass-through).
pub fn inspect_mime_signature(data: &[u8]) -> Option<&'static str> {
    // JPEG needs only 3 bytes; PNG/GIF need 4; WebP needs 12; PDF needs 5.
    if data.len() < 3 {
        return None;
    }

    // HEIC/HEIF: check for ftyp box with HEIC brand (needs 12 bytes)
    if data.len() >= 12 && data[4..8] == *b"ftyp" {
        for &brand in &[b"heic", b"heix", b"hevc", b"hevx", b"mif1", b"msf1"] {
            if &data[8..12] == brand {
                return Some("image/heic");
            }
        }
    }

    // JPEG: FF D8 FF (3 bytes)
    if data.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return Some("image/jpeg");
    }

    // PNG: 89 50 4E 47 (4 bytes)
    if data.len() >= 4 && data.starts_with(&[0x89, 0x50, 0x4E, 0x47]) {
        return Some("image/png");
    }

    // GIF87a / GIF89a (4 bytes)
    if data.starts_with(b"GIF8") {
        return Some("image/gif");
    }

    // WebP: RIFF....WEBP (12 bytes)
    if data.len() >= 12 && data.starts_with(b"RIFF") && data[8..12] == *b"WEBP" {
        return Some("image/webp");
    }

    // PDF: %PDF- (5 bytes)
    if data.len() >= 5 && data.starts_with(b"%PDF-") {
        return Some("application/pdf");
    }

    None
}

// ── Attachment classification ────────────────────────────────────────

/// What kind of processing an attachment should receive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttachmentKind {
    /// Binary image or PDF to be base64-encoded and sent to the LLM.
    Binary,
    /// Text document to be decoded and sent as plain text.
    Text,
    /// Unsupported type — the handler should reject with a user message.
    Unsupported,
}

/// Classify an attachment based on MIME type and filename.
pub fn classify_attachment(mime: Option<&str>, filename: Option<&str>) -> AttachmentKind {
    // Prefer MIME type classification
    if let Some(m) = mime {
        if is_supported_binary_mime(m) {
            return AttachmentKind::Binary;
        }
        if is_supported_text_mime(m) {
            return AttachmentKind::Text;
        }
        // HEIC/HEIF MIME types are binary (will be converted to JPEG)
        if crate::attachments::is_heic_mime(m) {
            return AttachmentKind::Binary;
        }
    }

    // Fall back to filename extension
    if let Some(f) = filename {
        if is_text_extension(f) {
            return AttachmentKind::Text;
        }
        // HEIC/HEIF filenames are binary (will be converted to JPEG)
        if HEIC_EXTENSIONS
            .iter()
            .any(|ext| f.to_lowercase().ends_with(ext))
        {
            return AttachmentKind::Binary;
        }
    }

    AttachmentKind::Unsupported
}

/// Get the largest photo size from a list of `PhotoSize` objects.
///
/// Returns the file_id of the largest photo (by width × height).
pub fn largest_photo_size(photoes: &[PhotoSize]) -> Option<&PhotoSize> {
    photoes.iter().max_by(|a, b| {
        let area_a = a.width.unwrap_or(0) * a.height.unwrap_or(0);
        let area_b = b.width.unwrap_or(0) * b.height.unwrap_or(0);
        area_a.cmp(&area_b)
    })
}

/// Get the largest photo file_id from a list of `PhotoSize` objects.
pub fn largest_photo_file_id(photoes: &[PhotoSize]) -> Option<String> {
    largest_photo_size(photoes).map(|p| p.file_id.clone())
}

// ── Attachment processing ────────────────────────────────────────────

/// Result of processing an attachment for the LLM.
#[derive(Debug, Clone)]
pub struct ProcessedAttachment {
    /// The genai ContentPart to include in the user message.
    /// For binary types, this is a ContentPart::Binary.
    /// For text types, this is a ContentPart::Text with extracted content.
    pub content_part: ContentPart,
    /// Compact marker text for persistence (e.g. "[Attached PDF: report.pdf, 842 KB]").
    pub persistence_marker: String,
    /// Whether the attachment was actually downloaded (false if unsupported).
    pub downloaded: bool,
    /// Extracted text content for text documents (bounded by max_text_chars).
    /// Only populated for text-type attachments.
    pub extracted_text: Option<String>,
    /// MIME type of the output (may differ from input for converted HEIC → JPEG).
    pub output_mime_type: String,
    /// Output filename (may differ from input for converted HEIC → JPEG).
    pub output_filename: Option<String>,
    /// Size of the output bytes (may differ from input for converted HEIC → JPEG).
    pub output_size_bytes: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Validation tests ─────────────────────────────────────────────

    #[test]
    fn test_is_supported_binary_mime() {
        assert!(is_supported_binary_mime("image/jpeg"));
        assert!(is_supported_binary_mime("image/png"));
        assert!(is_supported_binary_mime("image/webp"));
        assert!(is_supported_binary_mime("image/gif"));
        assert!(is_supported_binary_mime("application/pdf"));
        assert!(!is_supported_binary_mime("text/plain"));
        assert!(!is_supported_binary_mime("application/zip"));
    }

    #[test]
    fn test_is_supported_binary_mime_heic() {
        assert!(is_supported_binary_mime("image/heic"));
        assert!(is_supported_binary_mime("image/heif"));
        assert!(is_supported_binary_mime("image/heic-sequence"));
        assert!(is_supported_binary_mime("image/heif-sequence"));
    }

    #[test]
    fn test_is_supported_text_mime() {
        assert!(is_supported_text_mime("text/plain"));
        assert!(is_supported_text_mime("text/markdown"));
        assert!(is_supported_text_mime("application/json"));
        assert!(is_supported_text_mime("text/csv"));
        assert!(!is_supported_text_mime("image/png"));
        assert!(!is_supported_text_mime("application/pdf"));
    }

    #[test]
    fn test_is_text_extension() {
        assert!(is_text_extension("hello.txt"));
        assert!(is_text_extension("notes.md"));
        assert!(is_text_extension("data.json"));
        assert!(is_text_extension("data.csv"));
        assert!(is_text_extension("script.py"));
        assert!(is_text_extension("config.toml"));
        assert!(!is_text_extension("photo.jpg"));
        assert!(!is_text_extension("document.pdf"));
        assert!(!is_text_extension("archive.zip"));
    }

    #[test]
    fn test_sanitize_filename() {
        assert_eq!(sanitize_filename("hello.txt"), "hello.txt");
        assert_eq!(sanitize_filename("path/to/file.txt"), "path_to_file.txt");
        assert_eq!(
            sanitize_filename("file\0with\0nulls.txt"),
            "file_with_nulls.txt"
        );
        assert_eq!(
            sanitize_filename("path\\backslash.txt"),
            "path_backslash.txt"
        );
        assert_eq!(sanitize_filename(&"a".repeat(300)), "a".repeat(200));
    }

    #[test]
    fn test_inspect_mime_signature_png() {
        let png_header: [u8; 4] = [0x89, 0x50, 0x4E, 0x47];
        assert_eq!(inspect_mime_signature(&png_header), Some("image/png"));
    }

    #[test]
    fn test_inspect_mime_signature_jpeg() {
        let jpeg_header: [u8; 3] = [0xFF, 0xD8, 0xFF];
        assert_eq!(inspect_mime_signature(&jpeg_header), Some("image/jpeg"));
    }

    #[test]
    fn test_inspect_mime_signature_gif() {
        let gif_header: [u8; 4] = *b"GIF8";
        assert_eq!(inspect_mime_signature(&gif_header), Some("image/gif"));
    }

    #[test]
    fn test_inspect_mime_signature_webp() {
        let webp_header: [u8; 12] = *b"RIFF\x00\x00\x00\x00WEBP";
        assert_eq!(inspect_mime_signature(&webp_header), Some("image/webp"));
    }

    #[test]
    fn test_inspect_mime_signature_unknown() {
        assert_eq!(inspect_mime_signature(b"hello"), None);
        assert_eq!(inspect_mime_signature(b"hi"), None);
    }

    #[test]
    fn test_inspect_mime_signature_heic() {
        let heic_header: Vec<u8> = vec![
            0, 0, 0, 12, b'f', b't', b'y', b'p', b'h', b'e', b'i', b'c', 0, 0, 0, 0,
        ];
        assert_eq!(inspect_mime_signature(&heic_header), Some("image/heic"));
    }

    #[test]
    fn test_inspect_mime_signature_hevc_brand() {
        let data: Vec<u8> = vec![
            0, 0, 0, 12, b'f', b't', b'y', b'p', b'h', b'e', b'v', b'c', 0, 0, 0, 0,
        ];
        assert_eq!(inspect_mime_signature(&data), Some("image/heic"));
    }

    #[test]
    fn test_classify_binary_mime() {
        assert_eq!(
            classify_attachment(Some("image/jpeg"), None),
            AttachmentKind::Binary
        );
        assert_eq!(
            classify_attachment(Some("application/pdf"), None),
            AttachmentKind::Binary
        );
    }

    #[test]
    fn test_classify_text_mime() {
        assert_eq!(
            classify_attachment(Some("text/plain"), None),
            AttachmentKind::Text
        );
        assert_eq!(
            classify_attachment(Some("application/json"), None),
            AttachmentKind::Text
        );
    }

    #[test]
    fn test_classify_text_extension_fallback() {
        assert_eq!(
            classify_attachment(None, Some("report.txt")),
            AttachmentKind::Text
        );
        assert_eq!(
            classify_attachment(None, Some("data.csv")),
            AttachmentKind::Text
        );
    }

    #[test]
    fn test_classify_unsupported() {
        assert_eq!(
            classify_attachment(Some("application/zip"), None),
            AttachmentKind::Unsupported
        );
        assert_eq!(
            classify_attachment(None, Some("archive.zip")),
            AttachmentKind::Unsupported
        );
    }

    #[test]
    fn test_classify_heic_mime() {
        assert_eq!(
            classify_attachment(Some("image/heic"), None),
            AttachmentKind::Binary
        );
        assert_eq!(
            classify_attachment(Some("image/heif"), None),
            AttachmentKind::Binary
        );
    }

    #[test]
    fn test_classify_heic_filename() {
        assert_eq!(
            classify_attachment(None, Some("photo.heic")),
            AttachmentKind::Binary
        );
        assert_eq!(
            classify_attachment(None, Some("photo.heif")),
            AttachmentKind::Binary
        );
        assert_eq!(
            classify_attachment(None, Some("photo.HEIC")),
            AttachmentKind::Binary
        );
    }

    #[test]
    fn test_classify_mime_wins_over_extension() {
        // A .txt file with a PDF MIME type should be treated as binary
        assert_eq!(
            classify_attachment(Some("application/pdf"), Some("fake.txt")),
            AttachmentKind::Binary
        );
    }

    #[test]
    fn test_largest_photo_size() {
        let photos = vec![
            PhotoSize {
                width: Some(100),
                height: Some(100),
                ..Default::default()
            },
            PhotoSize {
                width: Some(640),
                height: Some(480),
                ..Default::default()
            },
            PhotoSize {
                width: Some(320),
                height: Some(240),
                ..Default::default()
            },
        ];

        let largest = largest_photo_size(&photos).unwrap();
        assert_eq!(largest.width, Some(640));
        assert_eq!(largest.height, Some(480));
    }

    #[test]
    fn test_largest_photo_empty() {
        assert!(largest_photo_file_id(&[]).is_none());
    }

    #[test]
    fn test_largest_photo_file_id() {
        let photos = vec![
            PhotoSize {
                file_id: "small".into(),
                width: Some(100),
                height: Some(100),
                ..Default::default()
            },
            PhotoSize {
                file_id: "large".into(),
                width: Some(640),
                height: Some(480),
                ..Default::default()
            },
        ];

        assert_eq!(largest_photo_file_id(&photos), Some("large".into()));
    }
}
