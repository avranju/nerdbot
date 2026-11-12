//! Integration coverage for the agent loop using test-only tool fixtures.

mod support;

use std::path::PathBuf;

use genai::chat::{
    ChatMessage, ChatOptions, ChatRequest, ChatRole, MessageContent, StopReason, Tool as GenAiTool,
};
use nerdbot::agent::agent_loop::{AgentContext, AgentLoopConfig, run_agent};
use nerdbot::agent::outcome::AgentOutcome;
use nerdbot::agent::run_mode::AgentRunMode;
use nerdbot::error::AgentError;
use nerdbot::llm::LlmExecutor;
use nerdbot::llm::fake::{FakeProvider, FakeResponse};
use nerdbot::tools::registry::ToolRegistry;
use nerdbot::tools::traits::{Tool, ToolContext, ToolOutput};
use serde_json::json;
use support::EchoTool;

fn test_context() -> AgentContext {
    AgentContext::new(
        "telegram",
        AgentRunMode::InteractiveReply {
            address: nerdbot::channel::ConversationAddress::telegram_chat(123_456_789),
            sender: nerdbot::channel::SenderIdentity::new("987654321", None),
        },
        "You are a helpful assistant.".to_string(),
        PathBuf::from("/workspace"),
        vec![],
        vec![],
    )
}

/// Both tools are fixtures, not built-ins available to the runtime.
fn toy_registry() -> ToolRegistry {
    let mut registry = ToolRegistry::new();
    registry.register(EchoTool).unwrap();
    registry.register(FailureTool).unwrap();
    registry
}

struct FailureTool;

#[async_trait::async_trait]
impl Tool for FailureTool {
    fn name(&self) -> &str {
        "test_failure"
    }

    fn description(&self) -> &str {
        "Test fixture returning an unsuccessful tool output."
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({"type": "object"})
    }

    async fn execute(
        &self,
        _args: serde_json::Value,
        _ctx: ToolContext,
    ) -> Result<ToolOutput, AgentError> {
        Ok(ToolOutput {
            success: false,
            data: json!({"error": "test failure"}),
            summary: "Test failure".into(),
        })
    }
}

struct TestSendUserMessageTool;

#[async_trait::async_trait]
impl Tool for TestSendUserMessageTool {
    fn name(&self) -> &str {
        "send_user_message"
    }

    fn description(&self) -> &str {
        "Test send_user_message tool"
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({"type": "object", "properties": {"text": {"type": "string"}}, "required": ["text"]})
    }

    async fn execute(
        &self,
        args: serde_json::Value,
        _ctx: ToolContext,
    ) -> Result<ToolOutput, AgentError> {
        Ok(ToolOutput {
            success: true,
            data: json!({"sent": true, "text": args["text"]}),
            summary: "test message sent".into(),
        })
    }
}

#[tokio::test]
async fn test_tool_call_then_final_text() {
    let provider = FakeProvider::new(vec![
        FakeResponse::tool_call("echo", json!({"message": "hello world"})),
        FakeResponse::final_text("Tool received: hello world"),
    ]);
    let result = run_agent(
        &test_context(),
        &provider,
        &toy_registry(),
        &AgentLoopConfig::default(),
    )
    .await
    .unwrap();
    assert_eq!(
        result.outcome,
        AgentOutcome::FinalText("Tool received: hello world".into())
    );
    assert_eq!(result.metadata.iterations, 2);
    assert_eq!(provider.call_count(), 2);
}

#[tokio::test]
async fn test_final_text_no_tools() {
    let provider = FakeProvider::new(vec![FakeResponse::final_text("Hello, I am ready.")]);
    let result = run_agent(
        &test_context(),
        &provider,
        &toy_registry(),
        &AgentLoopConfig::default(),
    )
    .await
    .unwrap();
    assert_eq!(
        result.outcome,
        AgentOutcome::FinalText("Hello, I am ready.".into())
    );
    assert_eq!(result.metadata.iterations, 1);
    assert_eq!(provider.call_count(), 1);
}

#[tokio::test]
async fn test_multiple_tool_calls_in_one_response() {
    let mut first = FakeResponse::tool_call("echo", json!({"message": "first"}));
    let mut second = first.tool_calls[0].clone();
    second.call_id = "fc_2".into();
    second.fn_arguments = json!({"message": "second"});
    first.tool_calls.push(second);
    let provider = FakeProvider::new(vec![
        first,
        FakeResponse::final_text("Done with both calls."),
    ]);
    let result = run_agent(
        &test_context(),
        &provider,
        &toy_registry(),
        &AgentLoopConfig::default(),
    )
    .await
    .unwrap();
    assert_eq!(
        result.outcome,
        AgentOutcome::FinalText("Done with both calls.".into())
    );
    assert_eq!(result.metadata.iterations, 2);
    assert_eq!(provider.total_tool_calls_received(), 4);
}

#[tokio::test]
async fn test_max_tool_iterations_exceeded() {
    let provider = FakeProvider::new(vec![FakeResponse::tool_call("echo", json!({})); 20]);
    let config = AgentLoopConfig {
        max_tool_iterations: 5,
        ..Default::default()
    };
    let result = run_agent(&test_context(), &provider, &toy_registry(), &config).await;
    assert!(matches!(
        result,
        Err(AgentError::MaxToolIterationsExceeded(5))
    ));
    assert_eq!(provider.call_count(), 5);
}

#[tokio::test]
async fn test_tool_error_produced_as_result() {
    let provider = FakeProvider::new(vec![
        FakeResponse::tool_call("nonexistent_tool", json!({})),
        FakeResponse::final_text("The tool did not exist."),
    ]);
    let result = run_agent(
        &test_context(),
        &provider,
        &toy_registry(),
        &AgentLoopConfig::default(),
    )
    .await
    .unwrap();
    assert_eq!(
        result.outcome,
        AgentOutcome::FinalText("The tool did not exist.".into())
    );
    assert_eq!(result.metadata.iterations, 2);
    assert_eq!(provider.call_count(), 2);
}

#[tokio::test]
async fn test_send_user_message_sets_run_metadata() {
    let provider = FakeProvider::new(vec![
        FakeResponse::tool_call(
            "send_user_message",
            json!({"text": "scheduled notification"}),
        ),
        FakeResponse::final_text("Done."),
    ]);
    let mut registry = toy_registry();
    registry.register(TestSendUserMessageTool).unwrap();
    let result = run_agent(
        &test_context(),
        &provider,
        &registry,
        &AgentLoopConfig::default(),
    )
    .await
    .unwrap();
    assert!(matches!(result.outcome, AgentOutcome::FinalText(_)));
    assert!(result.metadata.sent_user_message);
}

#[tokio::test]
async fn test_fake_provider_returns_sequence() {
    let provider = FakeProvider::new(vec![
        FakeResponse::tool_call("echo", json!({"message": "first"})),
        FakeResponse::tool_call("echo", json!({"message": "second"})),
        FakeResponse::final_text("All done."),
    ]);
    for expected_count in 1..=4 {
        let response = provider
            .complete("fake-model", ChatRequest::default(), ChatOptions::default())
            .await
            .unwrap();
        assert_eq!(provider.call_count(), expected_count);
        if expected_count < 3 {
            assert!(!response.tool_calls().is_empty());
        } else {
            assert!(response.tool_calls().is_empty());
            assert_eq!(response.first_text(), Some("All done."));
        }
    }
}

#[tokio::test]
async fn test_fake_provider_error_response() {
    let provider = FakeProvider::new(vec![FakeResponse::error("something went wrong")]);
    let result = provider
        .complete("fake-model", ChatRequest::default(), ChatOptions::default())
        .await;
    assert!(matches!(result, Err(AgentError::LlmProvider(_))));
}

#[tokio::test]
async fn test_fake_provider_inspects_last_request() {
    let provider = FakeProvider::new(vec![FakeResponse::final_text("test")]);
    let request = ChatRequest::default().with_tools(vec![
        GenAiTool::new("test_tool")
            .with_description("a test tool")
            .with_schema(json!({})),
    ]);
    provider
        .complete("fake-model", request, ChatOptions::default())
        .await
        .unwrap();
    let inspected = provider.last_request().unwrap();
    let tools = inspected.tools.as_ref().unwrap();
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0].name.as_str(), "test_tool");
}

#[tokio::test]
async fn test_fake_provider_reset() {
    let provider = FakeProvider::new(vec![
        FakeResponse::final_text("hello"),
        FakeResponse::final_text("world"),
    ]);
    provider
        .complete("fake-model", ChatRequest::default(), ChatOptions::default())
        .await
        .unwrap();
    assert_eq!(provider.call_count(), 1);
    provider.reset();
    assert_eq!(provider.call_count(), 0);
    assert!(provider.last_request().is_none());
    provider
        .complete("fake-model", ChatRequest::default(), ChatOptions::default())
        .await
        .unwrap();
    assert_eq!(provider.call_count(), 1);
    assert!(provider.last_request().is_some());
}

#[tokio::test]
async fn test_fake_provider_total_tool_calls_received() {
    let provider = FakeProvider::new(vec![
        FakeResponse::tool_call("echo", json!({"message": "test"})),
        FakeResponse::final_text("done"),
    ]);
    run_agent(
        &test_context(),
        &provider,
        &toy_registry(),
        &AgentLoopConfig::default(),
    )
    .await
    .unwrap();
    // The second request includes both the assistant call and tool result.
    assert_eq!(provider.total_tool_calls_received(), 2);
}

#[tokio::test]
async fn test_agent_loop_permission_denied() {
    let provider = FakeProvider::new(vec![
        FakeResponse::tool_call("echo", json!({})),
        FakeResponse::final_text("should not be reached"),
    ]);
    let mut ctx = test_context();
    ctx.access_policy.allowed_conversations = vec![nerdbot::channel::ConversationAddressPattern {
        channel_id: "telegram".into(),
        conversation_id: "999999999".into(),
        thread_id: None,
    }];
    let result = run_agent(
        &ctx,
        &provider,
        &toy_registry(),
        &AgentLoopConfig::default(),
    )
    .await;
    assert!(matches!(result, Err(AgentError::PermissionDenied)));
}

#[tokio::test]
async fn test_agent_loop_tool_returns_failure_output() {
    let provider = FakeProvider::new(vec![
        FakeResponse::tool_call("test_failure", json!({})),
        FakeResponse::final_text("The tool reported failure."),
    ]);
    let result = run_agent(
        &test_context(),
        &provider,
        &toy_registry(),
        &AgentLoopConfig::default(),
    )
    .await
    .unwrap();
    assert_eq!(
        result.outcome,
        AgentOutcome::FinalText("The tool reported failure.".into())
    );
    assert_eq!(provider.call_count(), 2);
    let request = provider.last_request().unwrap();
    let output = request
        .messages
        .iter()
        .flat_map(|message| message.content.parts())
        .find_map(|part| match part {
            genai::chat::ContentPart::ToolResponse(response) => Some(response),
            _ => None,
        })
        .expect("tool failure must be fed back to the provider");
    let data: serde_json::Value = serde_json::from_str(&output.content).unwrap();
    assert_eq!(data["success"], false);
    assert_eq!(data["data"]["error"], "test failure");
}

#[tokio::test]
async fn test_agent_context_with_initial_messages() {
    let mut ctx = test_context();
    ctx.messages
        .push(ChatMessage::user(MessageContent::from_text(
            "Initial message",
        )));
    let provider = FakeProvider::new(vec![FakeResponse::final_text("Got it.")]);
    let result = run_agent(
        &ctx,
        &provider,
        &toy_registry(),
        &AgentLoopConfig::default(),
    )
    .await
    .unwrap();
    assert!(matches!(result.outcome, AgentOutcome::FinalText(_)));
    assert!(
        provider
            .last_request()
            .unwrap()
            .messages
            .iter()
            .any(|message| message.role == ChatRole::User)
    );
}

#[tokio::test]
async fn test_tool_results_visible_to_provider() {
    let provider = FakeProvider::new(vec![
        FakeResponse::tool_call("echo", json!({"message": "test result"})),
        FakeResponse::final_text("I see the tool result."),
    ]);
    let result = run_agent(
        &test_context(),
        &provider,
        &toy_registry(),
        &AgentLoopConfig::default(),
    )
    .await
    .unwrap();
    assert_eq!(
        result.outcome,
        AgentOutcome::FinalText("I see the tool result.".into())
    );
    let request = provider.last_request().unwrap();
    assert!(
        request
            .messages
            .iter()
            .any(|message| message.role == ChatRole::Tool)
    );
}

#[tokio::test]
async fn test_agent_loop_includes_personality() {
    let provider = FakeProvider::new(vec![FakeResponse::final_text("Personality loaded.")]);
    let mut ctx = test_context();
    ctx.personality = "You speak in rhymes.".into();
    let result = run_agent(
        &ctx,
        &provider,
        &toy_registry(),
        &AgentLoopConfig::default(),
    )
    .await
    .unwrap();
    assert!(matches!(result.outcome, AgentOutcome::FinalText(_)));
    let request = provider.last_request().unwrap();
    assert!(
        request
            .messages
            .iter()
            .any(|message| message.role == ChatRole::System
                && message.content.joined_texts().as_deref() == Some("You speak in rhymes."))
    );
}

#[tokio::test]
async fn test_agent_loop_includes_personality_alongside_existing_system_message() {
    let provider = FakeProvider::new(vec![FakeResponse::final_text("Personality loaded.")]);
    let mut ctx = test_context();
    ctx.personality = "Runtime timezone: Asia/Kolkata.".into();
    ctx.messages.insert(
        0,
        ChatMessage::system(MessageContent::from_text("Conversation summary.")),
    );
    run_agent(
        &ctx,
        &provider,
        &toy_registry(),
        &AgentLoopConfig::default(),
    )
    .await
    .unwrap();
    let request = provider.last_request().unwrap();
    let system: Vec<_> = request
        .messages
        .iter()
        .filter(|message| message.role == ChatRole::System)
        .collect();
    assert_eq!(system.len(), 2);
    assert_eq!(
        system[0].content.joined_texts().as_deref(),
        Some("Runtime timezone: Asia/Kolkata.")
    );
    assert_eq!(
        system[1].content.joined_texts().as_deref(),
        Some("Conversation summary.")
    );
}

#[tokio::test]
async fn test_agent_loop_tracks_tokens() {
    let provider = FakeProvider::new(vec![FakeResponse {
        assistant_text: Some("test".into()),
        tool_calls: vec![],
        stop_reason: Some(StopReason::Completed("stop".into())),
        token_usage: Some((100, 50)),
    }]);
    let result = run_agent(
        &test_context(),
        &provider,
        &toy_registry(),
        &AgentLoopConfig::default(),
    )
    .await
    .unwrap();
    assert_eq!(result.metadata.token_usage.input_tokens, 100);
    assert_eq!(result.metadata.token_usage.output_tokens, 50);
}

#[tokio::test]
async fn test_silent_completion_when_no_text_and_no_tools() {
    assert_silent(None).await;
}

#[tokio::test]
async fn test_silent_completion_with_empty_text() {
    assert_silent(Some(String::new())).await;
}

async fn assert_silent(assistant_text: Option<String>) {
    let provider = FakeProvider::new(vec![FakeResponse {
        assistant_text,
        tool_calls: vec![],
        stop_reason: Some(StopReason::Completed("stop".into())),
        token_usage: None,
    }]);
    let result = run_agent(
        &test_context(),
        &provider,
        &toy_registry(),
        &AgentLoopConfig::default(),
    )
    .await
    .unwrap();
    assert_eq!(result.outcome, AgentOutcome::Silent);
    assert_eq!(result.metadata.iterations, 1);
    assert_eq!(provider.call_count(), 1);
}
