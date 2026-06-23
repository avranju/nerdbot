//! Zulip attachment processing.
//!
//! Zulip embeds uploaded files as markdown links in message content,
//! e.g., `[file.pdf](/user_uploads/1/99/abc/file.pdf)`. This module
//! extracts those links, downloads them authenticated, and converts
//! them to ContentPart structures for the LLM.

use std::sync::LazyLock;

use genai::chat::ContentPart;
use regex::Regex;
use tracing::warn;

use super::bot::ZulipBot;
use crate::attachments;
use crate::channel::AttachmentInfo;
use crate::error::AgentError;

/// Regex to match Zulip user-upload markdown links.
///
/// Matches patterns like `[filename.pdf](/user_uploads/1/99/abc/filename.pdf)`
static UPLOAD_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\[([^\]]+)\]\((/user_uploads/[^\)]+)\)").unwrap());

/// Process inbound Zulip message content for attachments.
///
/// Downloads each user-upload attachment, classifies it as binary or text,
/// and returns cleaned text with attachment parts.
///
/// # Arguments
/// * `raw_content` — Raw Zulip message markdown content
/// * `bot` — Authenticated Zulip bot client
/// * `max_bytes` — Maximum attachment download size
/// * `max_chars` — Maximum text document character limit
///
/// Returns `(clean_text, attachment_parts, attachment_infos)`.
pub async fn process_inbound_attachments(
    raw_content: &str,
    bot: &ZulipBot,
    max_bytes: usize,
    max_chars: usize,
) -> Result<(String, Vec<ContentPart>, Vec<AttachmentInfo>), AgentError> {
    let mut clean_text = raw_content.to_string();
    let mut attachment_parts = Vec::new();
    let mut attachment_infos = Vec::new();

    for cap in UPLOAD_REGEX.captures_iter(raw_content) {
        let filename = cap
            .get(1)
            .map(|m| m.as_str().to_string())
            .unwrap_or_default();
        let path = cap
            .get(2)
            .map(|m| m.as_str().to_string())
            .unwrap_or_default();

        // Sanitize filename (strip path separators, null bytes)
        let sanitized_filename = sanitize_filename(&filename);

        // Always clean the text by replacing the markdown link with a safe reference
        let original_md = format!("[{filename}]({path})");
        let replacement = format!("[Attachment: {sanitized_filename}]");
        clean_text = clean_text.replace(&original_md, &replacement);

        // 1. Download attachment bytes (bounded by max_bytes)
        let bytes = match bot.download_file(&path, max_bytes).await {
            Ok(b) => b,
            Err(e) => {
                warn!(
                    filename = sanitized_filename,
                    error = %e,
                    "Attachment download failed; skipping attachment processing"
                );
                attachment_infos.push(AttachmentInfo {
                    display_name: sanitized_filename.clone(),
                    mime_type: "application/octet-stream".to_string(),
                    size_bytes: 0,
                    downloaded: false,
                    persistence_marker: path.clone(),
                    extracted_text: None,
                });
                continue;
            }
        };
        let size_bytes = bytes.len() as u64;

        if bytes.len() > max_bytes {
            warn!(
                filename = sanitized_filename,
                file_size = bytes.len(),
                max_bytes,
                "Attachment file size exceeds limit; skipping download"
            );
            attachment_infos.push(AttachmentInfo {
                display_name: sanitized_filename.clone(),
                mime_type: infer_mime_type(&sanitized_filename, &bytes),
                size_bytes,
                downloaded: false,
                persistence_marker: path.clone(),
                extracted_text: None,
            });
            continue;
        }

        // 2. Map MIME type from extension / file signature
        let mime_type = infer_mime_type(&sanitized_filename, &bytes);

        // 3. Classify and process
        let is_text = mime_type.starts_with("text/")
            || sanitized_filename.ends_with(".json")
            || sanitized_filename.ends_with(".csv")
            || sanitized_filename.ends_with(".md")
            || sanitized_filename.ends_with(".txt")
            || sanitized_filename.ends_with(".yaml")
            || sanitized_filename.ends_with(".yml")
            || sanitized_filename.ends_with(".toml")
            || sanitized_filename.ends_with(".xml")
            || sanitized_filename.ends_with(".html")
            || sanitized_filename.ends_with(".css")
            || sanitized_filename.ends_with(".js")
            || sanitized_filename.ends_with(".ts")
            || sanitized_filename.ends_with(".py")
            || sanitized_filename.ends_with(".rs")
            || sanitized_filename.ends_with(".sh");

        // Check if this is a HEIC/HEIF image that needs conversion
        let is_heic = mime_type == "image/heic"
            || attachments::is_heic_filename(&sanitized_filename)
            || attachments::inspect_heic_signature(&bytes).is_some();

        let is_image = mime_type.starts_with("image/");
        let is_pdf = mime_type == "application/pdf";

        if is_text {
            let text_val = String::from_utf8_lossy(&bytes).into_owned();
            let (truncated_text, _actual_len) = if text_val.chars().count() > max_chars {
                let truncated: String = text_val.chars().take(max_chars).collect();
                (
                    format!("{truncated}\n\n[Truncated — exceeded {max_chars} character limit]"),
                    text_val.chars().count(),
                )
            } else {
                let len = text_val.chars().count();
                (text_val, len)
            };

            attachment_parts.push(ContentPart::Text(format!(
                "--- Attachment: {sanitized_filename} ---\n{}\n--- End Attachment ---\n",
                truncated_text
            )));

            attachment_infos.push(AttachmentInfo {
                display_name: sanitized_filename.clone(),
                mime_type,
                size_bytes,
                downloaded: true,
                persistence_marker: path.clone(),
                extracted_text: Some(truncated_text),
            });
        } else if is_heic {
            // Convert HEIC/HEIF to JPEG in memory
            match attachments::convert_heic_to_jpeg(&bytes, Some(&sanitized_filename)) {
                Ok(converted) => {
                    // Verify converted JPEG doesn't exceed the limit
                    if converted.bytes.len() > max_bytes {
                        warn!(
                            filename = sanitized_filename,
                            converted_size = converted.bytes.len(),
                            max_bytes,
                            "HEIC conversion produced output exceeding limit; skipping"
                        );
                        attachment_infos.push(AttachmentInfo {
                            display_name: sanitized_filename.clone(),
                            mime_type: "image/jpeg".to_string(),
                            size_bytes: converted.bytes.len() as u64,
                            downloaded: true,
                            persistence_marker: path.clone(),
                            extracted_text: None,
                        });
                        attachment_parts.push(ContentPart::Text(format!(
                            "⚠️ Attachment {sanitized_filename} converted from HEIC to JPEG, but the result ({}) exceeds the {max_bytes}-byte limit.",
                            converted.bytes.len()
                        )));
                        continue;
                    }

                    let base64_data = base64::Engine::encode(
                        &base64::engine::general_purpose::STANDARD,
                        &converted.bytes,
                    );
                    attachment_parts.push(ContentPart::from_binary_base64(
                        converted.mime_type.clone(),
                        base64_data,
                        converted.filename.clone(),
                    ));

                    attachment_infos.push(AttachmentInfo {
                        display_name: sanitized_filename.clone(),
                        mime_type: converted.mime_type,
                        size_bytes: converted.bytes.len() as u64,
                        downloaded: true,
                        persistence_marker: path.clone(),
                        extracted_text: None,
                    });
                }
                Err(e) => {
                    // Conversion failed — do not forward original HEIC bytes
                    warn!(
                        filename = sanitized_filename,
                        error = %e,
                        "HEIC to JPEG conversion failed"
                    );
                    attachment_infos.push(AttachmentInfo {
                        display_name: sanitized_filename.clone(),
                        mime_type: "image/heic".to_string(),
                        size_bytes,
                        downloaded: false,
                        persistence_marker: path.clone(),
                        extracted_text: None,
                    });
                    attachment_parts.push(ContentPart::Text(format!(
                        "⚠️ Failed to process HEIC attachment {sanitized_filename}: conversion failed. The image may be corrupted or use an unsupported HEIC variant."
                    )));
                }
            }
        } else if is_image {
            let base64_data =
                base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &bytes);
            attachment_parts.push(ContentPart::from_binary_base64(
                mime_type.clone(),
                base64_data,
                Some(sanitized_filename.clone()),
            ));

            attachment_infos.push(AttachmentInfo {
                display_name: sanitized_filename.clone(),
                mime_type,
                size_bytes,
                downloaded: true,
                persistence_marker: path.clone(),
                extracted_text: None,
            });
        } else if is_pdf {
            // PDFs are forwarded as binary content parts for the LLM to process
            let base64_data =
                base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &bytes);
            attachment_parts.push(ContentPart::from_binary_base64(
                mime_type.clone(),
                base64_data,
                Some(sanitized_filename.clone()),
            ));

            attachment_infos.push(AttachmentInfo {
                display_name: sanitized_filename.clone(),
                mime_type,
                size_bytes,
                downloaded: true,
                persistence_marker: path.clone(),
                extracted_text: None,
            });
        } else {
            // Unsupported binary — create a placeholder
            let kb = size_bytes / 1024;
            attachment_parts.push(ContentPart::Text(format!(
                "⚠️ Unsupported binary attachment: {sanitized_filename} ({mime_type}, {kb} KB).\n\
                 Supported: images (JPEG, PNG, WebP, GIF, HEIC/HEIF→JPEG), PDFs, and text documents."
            )));

            attachment_infos.push(AttachmentInfo {
                display_name: sanitized_filename.clone(),
                mime_type,
                size_bytes,
                downloaded: true,
                persistence_marker: path.clone(),
                extracted_text: None,
            });
        }
    }

    Ok((clean_text, attachment_parts, attachment_infos))
}

/// Sanitize a filename by removing path separators and null bytes.
fn sanitize_filename(name: &str) -> String {
    name.replace(['/', '\\', '\0'], "_")
        .chars()
        .take(200)
        .collect()
}

/// Infer MIME type from file extension and optional signature inspection.
/// For binary formats (images, PDFs), validates magic bytes against the extension
/// to prevent mislabeled or corrupted files from being forwarded.
/// Also recognizes HEIC/HEIF formats.
fn infer_mime_type(filename: &str, bytes: &[u8]) -> String {
    // Extract extension
    let ext = filename.split('.').next_back().map(|e| e.to_lowercase());

    // Check for HEIC/HEIF by extension first
    if attachments::is_heic_filename(filename) {
        return "image/heic".to_string();
    }

    // For binary formats, validate magic bytes against the extension
    if let Some(ref ext) = ext {
        let binary_ext = matches!(
            ext.as_str(),
            "jpg" | "jpeg" | "png" | "gif" | "webp" | "pdf"
        );
        if binary_ext {
            // Validate magic bytes for binary formats
            if ext == "png" && bytes.len() >= 4 && bytes[0..4] == [0x89, 0x50, 0x4E, 0x47] {
                return "image/png".to_string();
            }
            if (ext == "jpg" || ext == "jpeg") && bytes.len() >= 2 && bytes[0..2] == [0xFF, 0xD8] {
                return "image/jpeg".to_string();
            }
            if ext == "gif" && bytes.len() >= 3 && bytes[0..3] == [0x47, 0x49, 0x46] {
                return "image/gif".to_string();
            }
            if ext == "webp" && bytes.len() >= 4 && bytes[0..4] == [0x52, 0x49, 0x46, 0x46] {
                return "image/webp".to_string();
            }
            if ext == "pdf" && bytes.starts_with(b"%PDF") {
                return "application/pdf".to_string();
            }
            // Extension suggests binary but magic bytes don't match
            return "application/octet-stream".to_string();
        }
    }

    // Text formats: trust the extension
    if let Some(ref ext) = ext {
        match ext.as_str() {
            "txt" | "text" => return "text/plain".to_string(),
            "md" | "markdown" => return "text/markdown".to_string(),
            "json" => return "application/json".to_string(),
            "csv" => return "text/csv".to_string(),
            "xml" => return "application/xml".to_string(),
            "html" | "htm" => return "text/html".to_string(),
            "css" => return "text/css".to_string(),
            "js" => return "application/javascript".to_string(),
            "ts" => return "text/typescript".to_string(),
            "py" => return "text/x-python".to_string(),
            "rs" => return "text/x-rust".to_string(),
            "sh" => return "text/x-shellscript".to_string(),
            "yaml" | "yml" => return "text/yaml".to_string(),
            "toml" => return "text/toml".to_string(),
            _ => {}
        }
    }

    // Fallback: inspect magic bytes for unrecognized extensions
    if bytes.len() >= 12 {
        // Check for HEIC/HEIF ftyp signature
        if attachments::inspect_heic_signature(bytes).is_some() {
            return "image/heic".to_string();
        }
    }
    if bytes.len() >= 4 {
        if bytes[0..4] == [0x89, 0x50, 0x4E, 0x47] {
            return "image/png".to_string();
        }
        if bytes[0..2] == [0xFF, 0xD8] {
            return "image/jpeg".to_string();
        }
        if bytes[0..3] == [0x47, 0x49, 0x46] {
            return "image/gif".to_string();
        }
        if bytes[0..4] == [0x52, 0x49, 0x46, 0x46] {
            return "image/webp".to_string();
        }
        if bytes.starts_with(b"%PDF") {
            return "application/pdf".to_string();
        }
    }

    "application/octet-stream".to_string()
}
