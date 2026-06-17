//! Client and human-readable rendering for the local diagnostics socket.

use std::path::Path;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;

use crate::context::compaction_service::CompactionState;
use crate::diagnostics::protocol::{DiagnosticsRequest, DiagnosticsResponse, SessionDiagnostics};
use crate::error::AgentError;

/// Send one diagnostics request and read one newline-delimited JSON response.
pub async fn send_request(
    socket_path: &Path,
    request: &DiagnosticsRequest,
) -> Result<DiagnosticsResponse, AgentError> {
    let mut stream = UnixStream::connect(socket_path).await.map_err(|e| {
        AgentError::Diagnostics(format!(
            "Failed to connect to diagnostics socket {}: {e}",
            socket_path.display()
        ))
    })?;
    let request = serde_json::to_string(request).map_err(|e| {
        AgentError::Diagnostics(format!("Failed to encode diagnostics request: {e}"))
    })?;
    stream.write_all(request.as_bytes()).await.map_err(|e| {
        AgentError::Diagnostics(format!("Failed to write diagnostics request: {e}"))
    })?;
    stream.write_all(b"\n").await.map_err(|e| {
        AgentError::Diagnostics(format!("Failed to write diagnostics request: {e}"))
    })?;

    let mut response = String::new();
    BufReader::new(stream)
        .read_line(&mut response)
        .await
        .map_err(|e| {
            AgentError::Diagnostics(format!("Failed to read diagnostics response: {e}"))
        })?;
    if response.is_empty() {
        return Err(AgentError::Diagnostics(
            "Diagnostics server returned an empty response".to_string(),
        ));
    }

    let response: DiagnosticsResponse = serde_json::from_str(response.trim_end()).map_err(|e| {
        AgentError::Diagnostics(format!("Failed to decode diagnostics response: {e}"))
    })?;
    match response {
        DiagnosticsResponse::Error { message } => Err(AgentError::Diagnostics(message)),
        response => Ok(response),
    }
}

/// Render a diagnostics response for interactive terminal use.
pub fn render_human(response: &DiagnosticsResponse) -> String {
    match response {
        DiagnosticsResponse::Pong => "Diagnostics service is available.".to_string(),
        DiagnosticsResponse::Sessions { sessions } => render_sessions(sessions),
        DiagnosticsResponse::Session {
            session,
            context,
            compaction_state,
            effective_personality,
            summary_prompt,
            tool_spec,
        } => {
            let summary = context
                .latest_summary
                .as_ref()
                .map(|summary| {
                    format!(
                        "{} (covers through {})",
                        summary.id, summary.covers_through_message_id
                    )
                })
                .unwrap_or_else(|| "none".to_string());
            let mut output = format!(
                "Session: {}\n\
                 Channel: {} / {}\n\
                 Thread: {}\n\
                 Updated: {}\n\n\
                 Context usage (estimated):\n\
                   Uncompacted raw messages: {}\n\
                   Preserved raw messages:   {}\n\
                   Uncompacted tokens:       {}\n\
                   Usable input budget:      {}\n\
                   Hard bound threshold:     {}\n\
                   Remaining before bound:   {}\n\n\
                 Compaction:\n\
                   State:                    {}\n\
                   Soft threshold:           {}\n\
                   Remaining before run:     {}\n\
                   Pressure:                 {:.1}%\n\
                   Latest summary:           {}\n\n\
                 Prompts & Tools (estimated):\n\
                   Personality prompt:       {} tokens ({} chars, timezone={})\n\
                   Summary prompt:           {} tokens ({} chars)\n\
                   Registered tools:         {} tools, {} tokens",
                session.id,
                session.channel_id,
                session.conversation_id,
                session.thread_id.as_deref().unwrap_or("none"),
                session.updated_at,
                context.raw_message_count,
                context.preserved_raw_message_count,
                context.estimated_uncompacted_tokens,
                context.usable_input_budget,
                context.hard_threshold_tokens,
                context.remaining_before_hard_bound_tokens,
                render_compaction_state(compaction_state),
                context.soft_threshold_tokens,
                context.remaining_before_compaction_tokens,
                context.pressure * 100.0,
                summary,
                effective_personality.token_estimate,
                effective_personality.char_count,
                effective_personality.timezone,
                summary_prompt.token_estimate,
                summary_prompt.char_count,
                tool_spec.count,
                tool_spec.token_estimate,
            );

            if let Some(ref body) = effective_personality.body {
                output.push_str(&format!("\n\n--- Effective Personality Prompt ---\n{body}"));
            }
            if let Some(ref body) = summary_prompt.body {
                output.push_str(&format!("\n\n--- Compaction Summary Prompt ---\n{body}"));
            }
            if let Some(ref specs) = tool_spec.specs {
                output.push_str(&format!(
                    "\n\n--- Registered Tools Specs ---\n{}",
                    serde_json::to_string_pretty(specs).unwrap_or_default()
                ));
            }

            output
        }
        DiagnosticsResponse::Error { message } => format!("Diagnostics error: {message}"),
    }
}

/// Render a diagnostics response as pretty JSON for scripting.
pub fn render_json(response: &DiagnosticsResponse) -> Result<String, AgentError> {
    serde_json::to_string_pretty(response)
        .map_err(|e| AgentError::Diagnostics(format!("Failed to encode diagnostics output: {e}")))
}

fn render_sessions(sessions: &[SessionDiagnostics]) -> String {
    if sessions.is_empty() {
        return "No chat sessions found.".to_string();
    }

    let mut output = String::from("Sessions:\n");
    for session in sessions {
        let thread = session
            .thread_id
            .as_ref()
            .map(|thread| format!("#{thread}"))
            .unwrap_or_default();
        output.push_str(&format!(
            "  {}  conversation={}/{}{}  updated={}\n",
            session.id, session.channel_id, session.conversation_id, thread, session.updated_at
        ));
    }
    output.trim_end().to_string()
}

fn render_compaction_state(state: &CompactionState) -> String {
    match state {
        CompactionState::Idle => "idle".to_string(),
        CompactionState::Running {
            target_through_message_id,
        } => format!("running (target={target_through_message_id})"),
        CompactionState::RunningAndDirty {
            target_through_message_id,
        } => format!("running_and_dirty (target={target_through_message_id})"),
    }
}
