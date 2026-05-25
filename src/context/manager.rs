//! Context manager — assembles bounded model input.
//!
//! Implementations come in Phase 9.

use genai::chat::{ChatMessage, ChatRequest, MessageContent, Tool};

use crate::context::budget::ContextBudget;
use crate::error::AgentError;

/// Manages context assembly for model requests.
///
/// Phase 1 stub.
pub struct ContextManager {
    budget: ContextBudget,
}

impl ContextManager {
    pub fn new(budget: ContextBudget) -> Self {
        Self { budget }
    }

    /// Assemble a bounded model request from conversation state.
    ///
    /// Phase 1 stub — full implementation in Phase 9.
    pub async fn assemble_request(
        &self,
        personality: &str,
        _messages: &[ChatMessage],
        tools: Vec<Tool>,
        _model: &str,
    ) -> Result<ChatRequest, AgentError> {
        let mut request = ChatRequest::new(vec![ChatMessage::system(MessageContent::from_text(personality))]);
        if !tools.is_empty() {
            request = request.with_tools(tools);
        }
        Ok(request)
    }
}
