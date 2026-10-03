//! Provider-independent validation of replayed history and tool-exchange boundaries.

use std::collections::{HashMap, HashSet};
use std::ops::Range;

use genai::chat::{ChatMessage, ChatRole, ContentPart, MessageContent};

/// A chronological history unit. A complete call batch and all its outputs are
/// indivisible; `source` maps it back to the original rows for summary boundaries.
pub struct HistoryUnit {
    pub source: Range<usize>,
    pub messages: Vec<ChatMessage>,
}

/// Preserve complete, adjacent tool exchanges. Convert malformed protocol into
/// labeled ordinary text, without guessing calls/arguments or modifying storage.
/// A batch must have unique nonempty IDs and exactly one output per call before
/// any ordinary message. Reused IDs in later complete batches are namespaced
/// with their matching outputs, since providers may generate response-local IDs.
pub fn history_units(messages: &[ChatMessage]) -> Vec<HistoryUnit> {
    let mut units = Vec::new();
    let mut seen = HashSet::new();
    let mut index = 0;
    while index < messages.len() {
        let start = index;
        let message = &messages[index];
        let calls = message.content.tool_calls();
        if message.role == ChatRole::Assistant && !calls.is_empty() {
            index += 1;
            while index < messages.len() && messages[index].role == ChatRole::Tool {
                index += 1;
            }
            let mut expected = HashSet::new();
            let mut valid = true;
            for call in &calls {
                valid &= !call.call_id.is_empty() && expected.insert(call.call_id.clone());
            }
            // Assistant messages cannot themselves contain tool outputs.
            valid &= !message
                .content
                .parts()
                .iter()
                .any(|p| matches!(p, ContentPart::ToolResponse(_)));
            let mut answered = HashSet::new();
            for output in &messages[start + 1..index] {
                for part in output.content.parts() {
                    match part {
                        ContentPart::ToolResponse(response) => {
                            valid &= expected.contains(&response.call_id)
                                && answered.insert(response.call_id.clone());
                        }
                        // Mixed tool/text payloads are retained as historical
                        // text because providers differ in how they accept them.
                        _ => valid = false,
                    }
                }
            }
            valid &= answered == expected;
            let mut retained = messages[start..index].to_vec();
            if valid {
                let mut renamed = HashMap::new();
                for (ordinal, call) in calls.into_iter().enumerate() {
                    if !seen.insert(call.call_id.clone()) {
                        let mut candidate = format!("nerdbot_history_{start}_{ordinal}");
                        let mut suffix = 0;
                        while !seen.insert(candidate.clone()) {
                            suffix += 1;
                            candidate = format!("nerdbot_history_{start}_{ordinal}_{suffix}");
                        }
                        renamed.insert(call.call_id.clone(), (candidate, call.fn_name.clone()));
                    }
                }
                for message in &mut retained {
                    let mut parts = message.content.parts().clone();
                    for part in &mut parts {
                        match part {
                            ContentPart::ToolCall(call) => {
                                if let Some((id, _)) = renamed.get(&call.call_id) {
                                    call.call_id.clone_from(id);
                                }
                            }
                            ContentPart::ToolResponse(response) => {
                                if let Some((id, name)) = renamed.get(&response.call_id) {
                                    response.call_id.clone_from(id);
                                    // Gemini's legacy outputs may infer the name
                                    // from synthetic IDs. Keep the known call's
                                    // name explicitly when rewriting that ID.
                                    response.fn_name = Some(name.clone());
                                }
                            }
                            _ => {}
                        }
                    }
                    message.content = MessageContent::from_parts(parts);
                }
            } else {
                retained = retained.iter().map(historical_text).collect();
            }
            units.push(HistoryUnit {
                source: start..index,
                messages: retained,
            });
        } else {
            index += 1;
            let unsafe_protocol =
                message.role == ChatRole::Tool
                    || message.content.parts().iter().any(|p| {
                        matches!(p, ContentPart::ToolCall(_) | ContentPart::ToolResponse(_))
                    });
            units.push(HistoryUnit {
                source: start..index,
                messages: vec![if unsafe_protocol {
                    historical_text(message)
                } else {
                    message.clone()
                }],
            });
        }
    }
    units
}

pub fn normalize_history(messages: &[ChatMessage]) -> Vec<ChatMessage> {
    history_units(messages)
        .into_iter()
        .flat_map(|unit| unit.messages)
        .collect()
}

/// Render historical information for ordinary text replay and summarization.
/// Never put tool protocol or provider reasoning signatures in the result.
pub fn historical_text(message: &ChatMessage) -> ChatMessage {
    render_historical_text(message, "; incomplete or invalid exchange")
}

/// Ordinary text for a summary prompt, omitting opaque signatures so useful
/// arguments and outcomes remain visible within the per-row text limit.
pub fn summary_text(message: &ChatMessage) -> String {
    render_historical_text(message, "")
        .content
        .joined_texts()
        .unwrap_or_default()
}

fn render_historical_text(message: &ChatMessage, annotation: &str) -> ChatMessage {
    let mut parts: Vec<_> = message
        .content
        .parts()
        .iter()
        // Signatures are only meaningful in structured replay. Rendering an
        // opaque signature as text can crowd arguments out of summary limits.
        .filter(|part| !matches!(part, ContentPart::ThoughtSignature(_)))
        .map(|part| match part {
            ContentPart::ToolCall(call) => ContentPart::Text(format!(
                "[Historical tool call: {} (id: {}){annotation}]\nArguments: {}",
                call.fn_name, call.call_id, call.fn_arguments
            )),
            ContentPart::ToolResponse(response) => ContentPart::Text(format!(
                "[Historical tool output (id: {}){annotation}]\n{}",
                response.call_id, response.content
            )),
            ContentPart::ReasoningContent(_) | ContentPart::Custom(_) => {
                ContentPart::Text(format!(
                    "[Historical assistant metadata: {}]",
                    serde_json::to_string(part).unwrap_or_default()
                ))
            }
            _ => part.clone(),
        })
        .collect();
    if message.role == ChatRole::Tool
        && !message
            .content
            .parts()
            .iter()
            .any(|part| matches!(part, ContentPart::ToolResponse(_)))
    {
        parts.insert(0, ContentPart::Text("[Historical tool message]".into()));
    }
    let role = if message.role == ChatRole::Tool {
        ChatRole::User
    } else {
        message.role.clone()
    };
    ChatMessage::new(role, MessageContent::from_parts(parts))
}

/// Choose a chronological compaction prefix without crossing a tool exchange.
/// The recent-message window is rounded outward to preserve the entire batch.
pub fn compaction_prefix_len(messages: &[ChatMessage], preserve: usize) -> usize {
    let cutoff = messages.len().saturating_sub(preserve);
    history_units(messages)
        .iter()
        .take_while(|unit| unit.source.end <= cutoff)
        .last()
        .map(|unit| unit.source.end)
        .unwrap_or(0)
}

/// Retain a recent row window rounded outward to include complete exchanges.
pub fn recent_history(messages: &[ChatMessage], rows: usize) -> Vec<ChatMessage> {
    if rows == 0 {
        return Vec::new();
    }
    let mut retained = Vec::new();
    let mut count = 0;
    for unit in history_units(messages).into_iter().rev() {
        if count >= rows {
            break;
        }
        count += unit.source.len();
        retained.push(unit.messages);
    }
    retained.into_iter().rev().flatten().collect()
}
