//! Telegram inbound message conversion.
//!
//! Converts raw Telegram messages into channel-generic inbound payloads,
//! including attachment downloads and conversion to LLM content parts.

use genai::chat::ContentPart;
use tracing::warn;

use super::attachment::{self, AttachmentKind};
use super::bot::{Message, TelegramBot};
use crate::channel::AttachmentInfo;

/// Build an `InboundMessage` payload from a Telegram message.
///
/// Processes attachments (photos, documents) by:
/// 1. Selecting the largest photo variant
/// 2. Downloading supported files (bounded by config limits) via `TelegramBot::process_attachment`
/// 3. Converting to LLM content parts (binary or text)
/// 4. Building a user prompt from caption or text
///
/// Returns (user_prompt, content_parts, attachment_infos).
pub async fn build_inbound_message(
    msg: &Message,
    bot: &TelegramBot,
    max_attachment_bytes: usize,
    max_text_chars: usize,
) -> (String, Vec<ContentPart>, Vec<AttachmentInfo>) {
    let mut attachment_parts = Vec::new();
    let mut attachment_infos = Vec::new();
    let mut user_text = String::new();

    // Extract caption first (for photo/document messages)
    let caption = msg.caption.as_deref().unwrap_or("");

    // Process photos — select the largest variant
    if !msg.photo.is_empty()
        && let Some(largest) = attachment::largest_photo_size(&msg.photo)
    {
        let file_id = &largest.file_id;
        let size_bytes = largest.file_size.unwrap_or(0);

        // Download and process as binary via TelegramBot
        match bot
            .process_attachment(
                file_id,
                None,               // Photos don't have filenames
                Some("image/jpeg"), // Telegram photos are JPEG
                size_bytes,
                max_attachment_bytes,
                max_text_chars,
            )
            .await
        {
            Ok(processed) => {
                attachment_parts.push(processed.content_part);
                attachment_infos.push(AttachmentInfo {
                    display_name: format!(
                        "photo_{}x{}",
                        largest.width.unwrap_or(0),
                        largest.height.unwrap_or(0)
                    ),
                    mime_type: "image/jpeg".to_string(),
                    size_bytes,
                    downloaded: processed.downloaded,
                    persistence_marker: processed.persistence_marker,
                    extracted_text: processed.extracted_text,
                });
            }
            Err(e) => {
                warn!(chat_id = msg.chat.id, error = %e, "failed to process photo attachment");
                attachment_infos.push(AttachmentInfo {
                    display_name: "photo".to_string(),
                    mime_type: "image/jpeg".to_string(),
                    size_bytes,
                    downloaded: false,
                    persistence_marker: format!("[Attachment processing failed: photo, {e}]"),
                    extracted_text: None,
                });
                attachment_parts.push(ContentPart::Text(format!(
                    "⚠️ Failed to process photo: {e}"
                )));
            }
        }
    }

    // Track whether an unsupported warning was set (must not be overwritten).
    let mut unsupported_warn_set = false;

    // Process documents
    if let Some(doc) = &msg.document {
        let mime = doc.mime_type.as_deref();
        let filename = doc.file_name.as_deref();
        let size_bytes = doc.file_size.unwrap_or(0);

        // Validate before downloading
        let kind = attachment::classify_attachment(mime, filename);

        if kind == AttachmentKind::Unsupported {
            let display_name = attachment::sanitize_filename(filename.unwrap_or("document"));
            let mime_str = mime.unwrap_or("unknown");
            // For unsupported attachments, prepend the warning to the caption
            // so the LLM receives both the warning and any user text.
            let unsupported_msg = format!(
                "⚠️ Unsupported attachment type: {mime_str} ({display_name}).\n\
                 Supported: images (JPEG, PNG, WebP, GIF), PDFs, and text documents.",
            );
            if !caption.is_empty() {
                user_text = format!("{unsupported_msg}\n\n{caption}");
            } else if let Some(text) = &msg.text {
                user_text = format!("{unsupported_msg}\n\n{text}");
            } else {
                user_text = unsupported_msg;
            }
            attachment_infos.push(AttachmentInfo {
                display_name: display_name.clone(),
                mime_type: mime_str.to_string(),
                size_bytes,
                downloaded: false,
                persistence_marker: format!(
                    "[Unsupported attachment: {display_name}, type={mime_str}]"
                ),
                extracted_text: None,
            });
            unsupported_warn_set = true;
        } else {
            // Download and process via TelegramBot
            let file_id = &doc.file_id;
            match bot
                .process_attachment(
                    file_id,
                    filename,
                    mime,
                    size_bytes,
                    max_attachment_bytes,
                    max_text_chars,
                )
                .await
            {
                Ok(processed) => {
                    attachment_parts.push(processed.content_part);
                    // Use output filename when available (e.g., HEIC→JPEG conversion),
                    // falling back to the sanitized original filename.
                    let display_name = processed.output_filename.clone().unwrap_or_else(|| {
                        attachment::sanitize_filename(filename.unwrap_or("document"))
                    });
                    // Use output metadata when available (e.g., HEIC→JPEG conversion)
                    let output_mime = if !processed.output_mime_type.is_empty() {
                        processed.output_mime_type
                    } else {
                        mime.unwrap_or("application/octet-stream").to_string()
                    };
                    attachment_infos.push(AttachmentInfo {
                        display_name,
                        mime_type: output_mime,
                        size_bytes: processed.output_size_bytes,
                        downloaded: processed.downloaded,
                        persistence_marker: processed.persistence_marker,
                        extracted_text: processed.extracted_text,
                    });
                }
                Err(e) => {
                    warn!(chat_id = msg.chat.id, error = %e, "failed to process document attachment");
                    let display_name =
                        attachment::sanitize_filename(filename.unwrap_or("document"));
                    attachment_infos.push(AttachmentInfo {
                        display_name: display_name.clone(),
                        mime_type: mime.unwrap_or("application/octet-stream").to_string(),
                        size_bytes,
                        downloaded: false,
                        persistence_marker: format!(
                            "[Attachment processing failed: {display_name}, {e}]"
                        ),
                        extracted_text: None,
                    });
                    attachment_parts.push(ContentPart::Text(format!(
                        "⚠️ Failed to process document: {e}"
                    )));
                }
            }
        }
    }

    // Determine the user prompt text only if no unsupported warning was set.
    // Unsupported warnings already include the caption/text, so we must not
    // overwrite them.
    if !unsupported_warn_set {
        // Priority: caption > message text > default prompt
        if !caption.is_empty() {
            user_text = caption.to_string();
        } else if let Some(text) = &msg.text {
            user_text = text.clone();
        } else if !attachment_parts.is_empty() {
            // Attachment-only message: use default prompt
            user_text = "Please analyze the attached file(s).".to_string();
        }
    }

    (user_text, attachment_parts, attachment_infos)
}
