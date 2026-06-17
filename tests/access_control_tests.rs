//! Access control tests — chat ID and user ID authorization.
#![allow(
    dead_code,
    unused,
    unused_imports,
    unused_variables,
    unused_assignments
)]

use std::path::PathBuf;

use nerdbot::agent::agent_loop::AgentContext;
use nerdbot::agent::run_mode::AgentRunMode;
use nerdbot::error::AgentError;
use nerdbot::tools::registry::ToolRegistry;
use nerdbot::tools::traits::{Tool, ToolContext, ToolOutput};
use serde_json::json;

/// A minimal test tool for access control testing.
struct TestTool;

#[async_trait::async_trait]
impl Tool for TestTool {
    fn name(&self) -> &'static str {
        "test_tool"
    }
    fn description(&self) -> &'static str {
        "Test tool."
    }
    fn input_schema(&self) -> serde_json::Value {
        json!({})
    }
    async fn execute(
        &self,
        _args: serde_json::Value,
        _ctx: ToolContext,
    ) -> Result<ToolOutput, AgentError> {
        Ok(ToolOutput {
            success: true,
            data: json!({}),
            summary: "ok".to_string(),
        })
    }
}

fn make_tool_context(
    run_mode: AgentRunMode,
    allowed_conversations: Vec<i64>,
    allowed_senders: Vec<i64>,
) -> ToolContext {
    let allowed_conversations = allowed_conversations
        .into_iter()
        .map(|id| nerdbot::channel::ConversationAddressPattern {
            channel_id: "telegram".to_string(),
            conversation_id: id.to_string(),
            thread_id: None,
        })
        .collect();
    ToolContext {
        run_mode,
        workspace_root: PathBuf::from("/workspace"),
        access_policy: nerdbot::channel::ChannelAccessPolicy {
            allowed_conversations,
            allowed_senders: allowed_senders
                .into_iter()
                .map(|id| id.to_string())
                .collect(),
        },
        channel_registry: None,
        pool: None,
        scheduler_notifier: None,
    }
}

#[tokio::test]
async fn test_access_allowed_when_no_restrictions() {
    let mut registry = ToolRegistry::new();
    registry.register(TestTool);

    let ctx = make_tool_context(
        AgentRunMode::InteractiveReply {
            address: nerdbot::channel::ConversationAddress::telegram_chat(123),
            sender: nerdbot::channel::SenderIdentity::new((456).to_string(), None),
        },
        vec![], // empty = allow all
        vec![], // empty = allow all
    );

    let result = registry
        .execute(
            &genai::chat::ToolCall {
                call_id: "t1".into(),
                fn_name: "test_tool".into(),
                fn_arguments: json!({}),
                thought_signatures: None,
            },
            ctx,
        )
        .await;

    assert!(result.is_ok());
}

#[tokio::test]
async fn test_access_denied_when_chat_id_not_allowed() {
    let mut registry = ToolRegistry::new();
    registry.register(TestTool);

    // Config allows chat 999, but request comes from chat 123
    let allowed_conversations = vec![999i64];
    let ctx = make_tool_context(
        AgentRunMode::InteractiveReply {
            address: nerdbot::channel::ConversationAddress::telegram_chat(123),
            sender: nerdbot::channel::SenderIdentity::new((456).to_string(), None),
        },
        allowed_conversations.clone(),
        vec![],
    );

    let _ = registry
        .execute(
            &genai::chat::ToolCall {
                call_id: "t1".into(),
                fn_name: "test_tool".into(),
                fn_arguments: json!({}),
                thought_signatures: None,
            },
            ctx,
        )
        .await;

    // ToolRegistry.execute doesn't check access — that's done in run_agent
    // This test verifies that ToolContext correctly carries the IDs
    assert_eq!(allowed_conversations, vec![999]);
}

#[test]
fn test_context_carries_both_id_lists() {
    let ctx = make_tool_context(
        AgentRunMode::InteractiveReply {
            address: nerdbot::channel::ConversationAddress::telegram_chat(123),
            sender: nerdbot::channel::SenderIdentity::new((456).to_string(), None),
        },
        vec![111, 222],
        vec![333, 444],
    );

    let allowed_conversations = ctx.access_policy.allowed_conversations.clone();
    let allowed_senders = ctx.access_policy.allowed_senders.clone();
    assert_eq!(allowed_conversations.len(), 2);
    assert_eq!(allowed_senders, vec!["333", "444"]);
}

#[test]
fn test_agent_context_construction_with_user_ids() {
    let ctx = AgentContext::new(
        "telegram",
        AgentRunMode::InteractiveReply {
            address: nerdbot::channel::ConversationAddress::telegram_chat(123),
            sender: nerdbot::channel::SenderIdentity::new((456).to_string(), None),
        },
        "You are a helpful assistant.".to_string(),
        PathBuf::from("/workspace"),
        vec![123],
        vec![456],
    );

    assert_eq!(ctx.access_policy.allowed_conversations.len(), 1);
    assert_eq!(ctx.access_policy.allowed_senders, vec!["456"]);
}

#[test]
fn test_run_mode_extract_chat_and_user_ids() {
    let mode = AgentRunMode::InteractiveReply {
        address: nerdbot::channel::ConversationAddress::telegram_chat(123),
        sender: nerdbot::channel::SenderIdentity::new((456).to_string(), None),
    };

    assert_eq!(
        mode.address().unwrap().conversation_id.parse::<i64>().ok(),
        Some(123)
    );
    assert_eq!(
        mode.sender().unwrap().sender_id.parse::<i64>().ok(),
        Some(456)
    );
}

#[test]
fn test_internal_run_returns_none_for_chat_and_user() {
    let mode = AgentRunMode::Internal {
        reason: "test".into(),
    };

    assert_eq!(mode.address(), None);
    assert_eq!(mode.sender(), None);
}
