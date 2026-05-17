//! Context manager — assembles bounded model input.
//!
//! Implementations come in Phase 9.

use crate::context::budget::ContextBudget;
use crate::error::AgentError;
use crate::llm::types::ModelRequest;

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
        _messages: &[crate::llm::types::Message],
        tools: Vec<crate::llm::types::ToolSpec>,
        _model: &str,
    ) -> Result<ModelRequest, AgentError> {
        // Phase 1: return a basic request
        let request = ModelRequest::default()
            .with_messages(vec![crate::llm::types::Message::system(personality)])
            .with_tools(tools);

        Ok(request)
    }
}
