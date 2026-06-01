//! Integration tests for the agent loop.
//!
//! Tests exercise the full agent loop with a fake provider and toy tools,
//! verifying:
//! - Tool call → execute → feedback → final text flow
//! - Multiple tool calls in a single response
//! - Max iterations exceeded
//! - Error handling during tool execution
//! - Tool results are correctly fed back to the provider

#![allow(
    dead_code,
    unused,
    unused_imports,
    unused_variables,
    unused_assignments
)]

use std::path::PathBuf;

use genai::chat::{
    ChatMessage, ChatOptions, ChatRequest, ChatRole, MessageContent, StopReason, Tool as GenAiTool,
    ToolCall,
};
use nerdbot::agent::agent_loop::{AgentContext, AgentLoopConfig, run_agent};
use nerdbot::agent::outcome::{AgentOutcome, AgentResult};
use nerdbot::agent::run_mode::AgentRunMode;
use nerdbot::error::AgentError;
use nerdbot::llm::LlmExecutor;
use nerdbot::llm::fake::{FakeProvider, FakeResponse};
use nerdbot::tools::calculator::CalculatorTool;
use nerdbot::tools::echo::EchoTool;
use nerdbot::tools::registry::ToolRegistry;
use nerdbot::tools::traits::{Tool, ToolContext};

/// Build a standard test agent context.
fn test_context() -> AgentContext {
    AgentContext::new(
        AgentRunMode::InteractiveReply {
            chat_id: 123_456_789,
            user_id: 987_654_321,
        },
        "You are a helpful assistant.".to_string(),
        PathBuf::from("/workspace"),
        "fake-token".into(),
        vec![],
        vec![],
    )
}

/// Build a registry with both toy tools registered.
fn toy_registry() -> ToolRegistry {
    let mut registry = ToolRegistry::new();
    registry.register(EchoTool);
    registry.register(CalculatorTool);
    registry
}

// ── Test: Tool Call → Execute → Final Text ─────────────────────────────

#[tokio::test]
async fn test_tool_call_then_final_text() {
    // Provider returns: tool call to "echo", then final text
    let provider = FakeProvider::tool_then_final("Echo received: hello world");

    let ctx = test_context();
    let registry = toy_registry();
    let config = AgentLoopConfig::default();

    let result = run_agent(&ctx, &provider, &registry, &config)
        .await
        .unwrap();

    assert!(matches!(result.outcome, AgentOutcome::FinalText(_)));
    let AgentOutcome::FinalText(text) = result.outcome else {
        panic!("expected FinalText");
    };
    assert_eq!(text, "Echo received: hello world");
    assert_eq!(result.metadata.iterations, 2);
    assert_eq!(provider.call_count(), 2);
}

// ── Test: Final Text Without Tool Calls ────────────────────────────────

#[tokio::test]
async fn test_final_text_no_tools() {
    // Provider returns final text immediately (no tool calls)
    let provider = FakeProvider::new(vec![FakeResponse::final_text("Hello, I am ready.")]);

    let ctx = test_context();
    let registry = toy_registry();
    let config = AgentLoopConfig::default();

    let result = run_agent(&ctx, &provider, &registry, &config)
        .await
        .unwrap();

    assert!(matches!(result.outcome, AgentOutcome::FinalText(_)));
    let AgentOutcome::FinalText(text) = result.outcome else {
        panic!("expected FinalText");
    };
    assert_eq!(text, "Hello, I am ready.");
    assert_eq!(result.metadata.iterations, 1);
    assert_eq!(provider.call_count(), 1);
}

// ── Test: Multiple Tool Calls in One Response ──────────────────────────

#[tokio::test]
async fn test_multiple_tool_calls_in_one_response() {
    // Provider returns two tool calls at once, then final text
    let provider = FakeProvider::multi_tool_then_final("Done with both calculations.");

    let ctx = test_context();
    let registry = toy_registry();
    let config = AgentLoopConfig::default();

    let result = run_agent(&ctx, &provider, &registry, &config)
        .await
        .unwrap();

    assert!(matches!(result.outcome, AgentOutcome::FinalText(_)));
    let AgentOutcome::FinalText(text) = result.outcome else {
        panic!("expected FinalText");
    };
    assert_eq!(text, "Done with both calculations.");
    assert_eq!(result.metadata.iterations, 2);
    assert_eq!(provider.call_count(), 2);
}

// ── Test: Max Iterations Exceeded ──────────────────────────────────────

#[tokio::test]
async fn test_max_tool_iterations_exceeded() {
    // Provider always returns tool calls (never final text)
    let always_tool = FakeResponse::tool_call("echo", serde_json::json!({"message": "keep going"}));
    let provider = FakeProvider::new(vec![always_tool; 20]);

    let ctx = test_context();
    let registry = toy_registry();
    let config = AgentLoopConfig {
        max_tool_iterations: 5,
        llm_model: String::new(),
        llm_temperature: 0.0,
        llm_max_output_tokens: 0,
    };

    let result = run_agent(&ctx, &provider, &registry, &config).await;

    assert!(result.is_err());
    assert!(matches!(
        result.unwrap_err(),
        AgentError::MaxToolIterationsExceeded(5)
    ));
    assert_eq!(provider.call_count(), 5);
}

// ── Test: Tool Execution Error Handling ────────────────────────────────

#[tokio::test]
async fn test_tool_error_produced_as_result() {
    // Provider returns a tool call to a nonexistent tool, then final text
    let provider = FakeProvider::new(vec![
        FakeResponse::tool_call("nonexistent_tool", serde_json::json!({"bad": "args"})),
        FakeResponse::final_text("The tool did not exist."),
    ]);

    let ctx = test_context();
    let registry = toy_registry();
    let config = AgentLoopConfig::default();

    let result = run_agent(&ctx, &provider, &registry, &config)
        .await
        .unwrap();

    assert!(matches!(result.outcome, AgentOutcome::FinalText(_)));
    let AgentOutcome::FinalText(text) = result.outcome else {
        panic!("expected FinalText");
    };
    assert_eq!(text, "The tool did not exist.");
    assert_eq!(result.metadata.iterations, 2);
    assert_eq!(provider.call_count(), 2);
}

// ── Test: Echo Tool Actually Works ─────────────────────────────────────

#[tokio::test]
async fn test_echo_tool_execution() {
    let tool = EchoTool;
    assert_eq!(tool.name(), "echo");
    assert!(!tool.description().is_empty());
    assert!(tool.input_schema().is_object());

    let ctx = ToolContext {
        run_mode: AgentRunMode::InteractiveReply {
            chat_id: 1,
            user_id: 1,
        },
        workspace_root: PathBuf::from("/tmp"),
        telegram_token: "test".into(),
        allowed_chat_ids: vec![],
        allowed_user_ids: vec![],
        pool: None,
        scheduler_notifier: None,
    };

    let result = tool
        .execute(serde_json::json!({"message": "hello world"}), ctx)
        .await;

    assert!(result.is_ok());
    let output = result.unwrap();
    assert!(output.success);
    assert_eq!(output.summary, "Echoed: hello world");
    let data = output.data.get("echoed").unwrap().as_str().unwrap();
    assert_eq!(data, "hello world");
}

// ── Test: Calculator Tool — Basic Operations ───────────────────────────

#[tokio::test]
async fn test_calculator_add() {
    let tool = CalculatorTool;
    let ctx = ToolContext {
        run_mode: AgentRunMode::InteractiveReply {
            chat_id: 1,
            user_id: 1,
        },
        workspace_root: PathBuf::from("/tmp"),
        telegram_token: "test".into(),
        allowed_chat_ids: vec![],
        allowed_user_ids: vec![],
        pool: None,
        scheduler_notifier: None,
    };

    let result = tool
        .execute(serde_json::json!({"operation": "add", "a": 2, "b": 3}), ctx)
        .await;

    assert!(result.is_ok());
    let output = result.unwrap();
    assert!(output.success);
    let data = output.data.as_object().unwrap();
    assert_eq!(data.get("result").and_then(|v| v.as_f64()).unwrap(), 5.0);
}

#[tokio::test]
async fn test_calculator_subtract() {
    let tool = CalculatorTool;
    let ctx = ToolContext {
        run_mode: AgentRunMode::InteractiveReply {
            chat_id: 1,
            user_id: 1,
        },
        workspace_root: PathBuf::from("/tmp"),
        telegram_token: "test".into(),
        allowed_chat_ids: vec![],
        allowed_user_ids: vec![],
        pool: None,
        scheduler_notifier: None,
    };

    let result = tool
        .execute(
            serde_json::json!({"operation": "subtract", "a": 10, "b": 4}),
            ctx,
        )
        .await;

    assert!(result.is_ok());
    let output = result.unwrap();
    let data = output.data.as_object().unwrap();
    assert_eq!(data.get("result").and_then(|v| v.as_f64()).unwrap(), 6.0);
}

#[tokio::test]
async fn test_calculator_multiply() {
    let tool = CalculatorTool;
    let ctx = ToolContext {
        run_mode: AgentRunMode::InteractiveReply {
            chat_id: 1,
            user_id: 1,
        },
        workspace_root: PathBuf::from("/tmp"),
        telegram_token: "test".into(),
        allowed_chat_ids: vec![],
        allowed_user_ids: vec![],
        pool: None,
        scheduler_notifier: None,
    };

    let result = tool
        .execute(
            serde_json::json!({"operation": "multiply", "a": 7, "b": 6}),
            ctx,
        )
        .await;

    assert!(result.is_ok());
    let output = result.unwrap();
    let data = output.data.as_object().unwrap();
    assert_eq!(data.get("result").and_then(|v| v.as_f64()).unwrap(), 42.0);
}

#[tokio::test]
async fn test_calculator_divide() {
    let tool = CalculatorTool;
    let ctx = ToolContext {
        run_mode: AgentRunMode::InteractiveReply {
            chat_id: 1,
            user_id: 1,
        },
        workspace_root: PathBuf::from("/tmp"),
        telegram_token: "test".into(),
        allowed_chat_ids: vec![],
        allowed_user_ids: vec![],
        pool: None,
        scheduler_notifier: None,
    };

    let result = tool
        .execute(
            serde_json::json!({"operation": "divide", "a": 15, "b": 3}),
            ctx,
        )
        .await;

    assert!(result.is_ok());
    let output = result.unwrap();
    let data = output.data.as_object().unwrap();
    assert_eq!(data.get("result").and_then(|v| v.as_f64()).unwrap(), 5.0);
}

#[tokio::test]
async fn test_calculator_division_by_zero() {
    let tool = CalculatorTool;
    let ctx = ToolContext {
        run_mode: AgentRunMode::InteractiveReply {
            chat_id: 1,
            user_id: 1,
        },
        workspace_root: PathBuf::from("/tmp"),
        telegram_token: "test".into(),
        allowed_chat_ids: vec![],
        allowed_user_ids: vec![],
        pool: None,
        scheduler_notifier: None,
    };

    let result = tool
        .execute(
            serde_json::json!({"operation": "divide", "a": 1, "b": 0}),
            ctx,
        )
        .await;

    // Should succeed but return a failure output (not an AgentError)
    assert!(result.is_ok());
    let output = result.unwrap();
    assert!(!output.success);
    assert!(output.summary.contains("division by zero"));
}

#[tokio::test]
async fn test_calculator_invalid_operation() {
    let tool = CalculatorTool;
    let ctx = ToolContext {
        run_mode: AgentRunMode::InteractiveReply {
            chat_id: 1,
            user_id: 1,
        },
        workspace_root: PathBuf::from("/tmp"),
        telegram_token: "test".into(),
        allowed_chat_ids: vec![],
        allowed_user_ids: vec![],
        pool: None,
        scheduler_notifier: None,
    };

    let result = tool
        .execute(
            serde_json::json!({"operation": "modulus", "a": 10, "b": 3}),
            ctx,
        )
        .await;

    assert!(result.is_err());
    assert!(matches!(
        result.unwrap_err(),
        AgentError::InvalidToolArgs(_)
    ));
}

#[tokio::test]
async fn test_calculator_missing_field() {
    let tool = CalculatorTool;
    let ctx = ToolContext {
        run_mode: AgentRunMode::InteractiveReply {
            chat_id: 1,
            user_id: 1,
        },
        workspace_root: PathBuf::from("/tmp"),
        telegram_token: "test".into(),
        allowed_chat_ids: vec![],
        allowed_user_ids: vec![],
        pool: None,
        scheduler_notifier: None,
    };

    // Missing "b" field
    let result = tool
        .execute(serde_json::json!({"operation": "add", "a": 5}), ctx)
        .await;

    assert!(result.is_err());
    assert!(matches!(
        result.unwrap_err(),
        AgentError::InvalidToolArgs(_)
    ));
}

// ── Test: Fake Provider Behavior ───────────────────────────────────────

#[tokio::test]
async fn test_fake_provider_returns_sequence() {
    let provider = FakeProvider::new(vec![
        FakeResponse::tool_call("echo", serde_json::json!({"message": "first"})),
        FakeResponse::tool_call(
            "calculator",
            serde_json::json!({"operation": "add", "a": 1, "b": 2}),
        ),
        FakeResponse::final_text("All done."),
    ]);

    // First call: tool call
    let resp1 = provider
        .complete("fake-model", ChatRequest::default(), ChatOptions::default())
        .await
        .unwrap();
    assert!(!resp1.tool_calls().is_empty());
    assert_eq!(provider.call_count(), 1);

    // Second call: tool call
    let resp2 = provider
        .complete("fake-model", ChatRequest::default(), ChatOptions::default())
        .await
        .unwrap();
    assert!(!resp2.tool_calls().is_empty());
    assert_eq!(provider.call_count(), 2);

    // Third call: final text
    let resp3 = provider
        .complete("fake-model", ChatRequest::default(), ChatOptions::default())
        .await
        .unwrap();
    assert!(resp3.tool_calls().is_empty());
    assert_eq!(resp3.first_text(), Some("All done."));
    assert_eq!(provider.call_count(), 3);

    // Fourth call: returns last response again (sequence exhausted)
    let resp4 = provider
        .complete("fake-model", ChatRequest::default(), ChatOptions::default())
        .await
        .unwrap();
    assert!(resp4.tool_calls().is_empty());
    assert_eq!(provider.call_count(), 4);
}

#[tokio::test]
async fn test_fake_provider_error_response() {
    let provider = FakeProvider::new(vec![FakeResponse::error("something went wrong")]);

    let result = provider
        .complete("fake-model", ChatRequest::default(), ChatOptions::default())
        .await;
    assert!(result.is_err());
    assert!(matches!(result.unwrap_err(), AgentError::LlmProvider(_)));
}

#[tokio::test]
async fn test_fake_provider_inspects_last_request() {
    let provider = FakeProvider::new(vec![FakeResponse::final_text("test")]);

    let request = ChatRequest::default().with_tools(vec![
        GenAiTool::new("test_tool")
            .with_description("a test tool")
            .with_schema(serde_json::json!({})),
    ]);

    let _ = provider
        .complete("fake-model", request.clone(), ChatOptions::default())
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

    let _ = provider
        .complete("fake-model", ChatRequest::default(), ChatOptions::default())
        .await
        .unwrap();
    assert_eq!(provider.call_count(), 1);

    provider.reset();
    assert_eq!(provider.call_count(), 0);

    let _ = provider
        .complete("fake-model", ChatRequest::default(), ChatOptions::default())
        .await
        .unwrap();
    assert_eq!(provider.call_count(), 1);
    assert!(provider.last_request().is_some());
}

#[tokio::test]
async fn test_fake_provider_total_tool_calls_received() {
    let provider = FakeProvider::new(vec![
        FakeResponse::tool_call("echo", serde_json::json!({"message": "test"})),
        FakeResponse::final_text("done"),
    ]);

    let ctx = test_context();
    let registry = toy_registry();
    let config = AgentLoopConfig::default();

    let _ = run_agent(&ctx, &provider, &registry, &config)
        .await
        .unwrap();

    // After the agent loop, the provider should have seen:
    // - 1 tool call from first response (echo)
    // - 1 tool result from tool execution (in second request)
    // Total: 2 tool-related messages in request
    let total = provider.total_tool_calls_received();
    assert_eq!(total, 2);
}

#[tokio::test]
async fn test_agent_loop_permission_denied() {
    // Provider returns a tool call, which triggers the permission check
    let provider = FakeProvider::new(vec![
        FakeResponse::tool_call("echo", serde_json::json!({"message": "test"})),
        FakeResponse::final_text("should not be reached"),
    ]);

    let mut ctx = test_context();
    // Set allowed_chat_ids to a non-matching value
    ctx.allowed_chat_ids = vec![999_999_999];

    let registry = toy_registry();
    let config = AgentLoopConfig::default();

    let result = run_agent(&ctx, &provider, &registry, &config).await;
    assert!(result.is_err());
    assert!(matches!(result.unwrap_err(), AgentError::PermissionDenied));
}

#[tokio::test]
async fn test_agent_loop_tool_returns_failure_output() {
    // Calculator returns success: false for division by zero.
    // The agent loop should handle this and continue (not error out).
    let provider = FakeProvider::new(vec![
        FakeResponse::tool_call(
            "calculator",
            serde_json::json!({"operation": "divide", "a": 1, "b": 0}),
        ),
        FakeResponse::final_text("Division by zero was attempted."),
    ]);

    let ctx = test_context();
    let registry = toy_registry();
    let config = AgentLoopConfig::default();

    let result = run_agent(&ctx, &provider, &registry, &config)
        .await
        .unwrap();
    assert!(matches!(result.outcome, AgentOutcome::FinalText(_)));
    let AgentOutcome::FinalText(text) = result.outcome else {
        panic!("expected FinalText");
    };
    assert_eq!(text, "Division by zero was attempted.");
    assert_eq!(provider.call_count(), 2);
}

// ── Test: Agent Context Carries Messages ───────────────────────────────

#[tokio::test]
async fn test_agent_context_with_initial_messages() {
    let mut ctx = test_context();
    ctx.messages
        .push(ChatMessage::user(MessageContent::from_text(
            "Initial message",
        )));

    let provider = FakeProvider::new(vec![FakeResponse::final_text("Got it.")]);
    let registry = toy_registry();
    let config = AgentLoopConfig::default();

    let result = run_agent(&ctx, &provider, &registry, &config)
        .await
        .unwrap();

    assert!(matches!(result.outcome, AgentOutcome::FinalText(_)));
    // Verify the provider received the initial user message
    let last_req = provider.last_request().unwrap();
    let user_msgs: Vec<_> = last_req
        .messages
        .iter()
        .filter(|m| matches!(m.role, ChatRole::User))
        .collect();
    assert!(!user_msgs.is_empty());
}

// ── Test: Tool Results Feeded Back to Provider ─────────────────────────

#[tokio::test]
async fn test_tool_results_visible_to_provider() {
    // The provider's second call should see the tool results in the message history.
    // We verify this by checking the last_request for ToolRole messages.
    let provider = FakeProvider::new(vec![
        FakeResponse::tool_call("echo", serde_json::json!({"message": "test echo"})),
        FakeResponse::final_text("I see the echo result."),
    ]);

    let ctx = test_context();
    let registry = toy_registry();
    let config = AgentLoopConfig::default();

    let result = run_agent(&ctx, &provider, &registry, &config)
        .await
        .unwrap();

    assert!(matches!(result.outcome, AgentOutcome::FinalText(_)));
    let AgentOutcome::FinalText(text) = result.outcome else {
        panic!("expected FinalText");
    };
    assert_eq!(text, "I see the echo result.");

    // The second call should have received messages with the tool results
    let second_req = provider.last_request().unwrap();
    let tool_messages: Vec<_> = second_req
        .messages
        .iter()
        .filter(|m| matches!(m.role, ChatRole::Tool))
        .collect();
    assert!(
        !tool_messages.is_empty(),
        "Provider should have received Tool messages with tool results"
    );
}

// ── Test: Registry with Only One Toy Tool ──────────────────────────────

#[test]
fn test_registry_with_echo_only() {
    let mut registry = ToolRegistry::new();
    registry.register(EchoTool);

    assert_eq!(registry.len(), 1);
    assert!(!registry.is_empty());

    let specs = registry.specs();
    assert_eq!(specs.len(), 1);
    assert_eq!(specs[0].name.as_str(), "echo");
}

#[test]
fn test_registry_with_calculator_only() {
    let mut registry = ToolRegistry::new();
    registry.register(CalculatorTool);

    assert_eq!(registry.len(), 1);

    let specs = registry.specs();
    assert_eq!(specs[0].name.as_str(), "calculator");
    let props = specs[0]
        .schema
        .as_ref()
        .unwrap()
        .get("properties")
        .and_then(|p| p.as_object());
    assert!(props.is_some());
    let props = props.unwrap();
    assert!(props.contains_key("operation"));
}

// ── Test: Registry Execute Echo Tool ───────────────────────────────────

#[tokio::test]
async fn test_registry_execute_echo() {
    let mut registry = ToolRegistry::new();
    registry.register(EchoTool);

    let call = ToolCall {
        call_id: "tc_1".into(),
        fn_name: "echo".into(),
        fn_arguments: serde_json::json!({"message": "test message"}),
        thought_signatures: None,
    };

    let ctx = ToolContext {
        run_mode: AgentRunMode::InteractiveReply {
            chat_id: 1,
            user_id: 1,
        },
        workspace_root: PathBuf::from("/tmp"),
        telegram_token: "test".into(),
        allowed_chat_ids: vec![],
        allowed_user_ids: vec![],
        pool: None,
        scheduler_notifier: None,
    };

    let result = registry.execute(&call, ctx).await;
    assert!(result.is_ok());
    let output = result.unwrap();
    assert!(output.success);
}

// ── Test: Registry Execute Calculator Tool ─────────────────────────────

#[tokio::test]
async fn test_registry_execute_calculator() {
    let mut registry = ToolRegistry::new();
    registry.register(CalculatorTool);

    let call = ToolCall {
        call_id: "tc_2".into(),
        fn_name: "calculator".into(),
        fn_arguments: serde_json::json!({"operation": "multiply", "a": 3, "b": 7}),
        thought_signatures: None,
    };

    let ctx = ToolContext {
        run_mode: AgentRunMode::InteractiveReply {
            chat_id: 1,
            user_id: 1,
        },
        workspace_root: PathBuf::from("/tmp"),
        telegram_token: "test".into(),
        allowed_chat_ids: vec![],
        allowed_user_ids: vec![],
        pool: None,
        scheduler_notifier: None,
    };

    let result = registry.execute(&call, ctx).await;
    assert!(result.is_ok());
    let output = result.unwrap();
    assert!(output.success);
    let data = output.data.as_object().unwrap();
    assert_eq!(data.get("result").and_then(|v| v.as_f64()).unwrap(), 21.0);
}

// ── Test: FakeResponse Builders ──────────────────────────────────────────

#[test]
fn test_echo_tool_spec_generation() {
    let tool = EchoTool;
    let spec = GenAiTool::new(tool.name())
        .with_description(tool.description())
        .with_schema(tool.input_schema());

    assert_eq!(spec.name.as_str(), "echo");
    assert_eq!(spec.description.as_deref(), Some(tool.description()));
    assert!(spec.schema.as_ref().unwrap().is_object());
    assert!(
        spec.schema
            .as_ref()
            .unwrap()
            .get("required")
            .and_then(|r| r.as_array())
            .map(|a| a.iter().any(|v| v.as_str() == Some("message")))
            .unwrap_or(false)
    );
}

#[test]
fn test_calculator_tool_spec_generation() {
    let tool = CalculatorTool;
    let spec = GenAiTool::new(tool.name())
        .with_description(tool.description())
        .with_schema(tool.input_schema());

    assert_eq!(spec.name.as_str(), "calculator");
    assert_eq!(spec.description.as_deref(), Some(tool.description()));
    let schema = spec.schema.as_ref().unwrap().as_object().unwrap();
    assert!(schema.contains_key("properties"));
    assert!(schema.contains_key("required"));

    let props = schema.get("properties").unwrap().as_object().unwrap();
    assert!(props.contains_key("operation"));
    assert!(props.contains_key("a"));
    assert!(props.contains_key("b"));

    let operation = props.get("operation").unwrap().as_object().unwrap();
    let enums = operation.get("enum").unwrap().as_array().unwrap();
    assert_eq!(enums.len(), 4);
}

// ── Test: Agent Loop with Personality ──────────────────────────────────

#[tokio::test]
async fn test_agent_loop_includes_personality() {
    let provider = FakeProvider::new(vec![FakeResponse::final_text("Personality loaded.")]);

    let mut ctx = test_context();
    ctx.personality = "You speak in rhymes.".to_string();

    let registry = toy_registry();
    let config = AgentLoopConfig::default();

    let result = run_agent(&ctx, &provider, &registry, &config)
        .await
        .unwrap();

    assert!(matches!(result.outcome, AgentOutcome::FinalText(_)));
    // The provider should have received a System message with the personality
    let last_req = provider.last_request().unwrap();
    let system_msgs: Vec<_> = last_req
        .messages
        .iter()
        .filter(|m| matches!(m.role, ChatRole::System))
        .collect();
    assert!(
        !system_msgs.is_empty(),
        "Provider should have received a System message"
    );
}

#[tokio::test]
async fn test_agent_loop_includes_personality_alongside_existing_system_message() {
    let provider = FakeProvider::new(vec![FakeResponse::final_text("Personality loaded.")]);

    let mut ctx = test_context();
    ctx.personality = "Runtime timezone: Asia/Kolkata.".to_string();
    ctx.messages.insert(
        0,
        ChatMessage::system(MessageContent::from_text("Conversation summary.")),
    );

    let registry = toy_registry();
    let config = AgentLoopConfig::default();

    run_agent(&ctx, &provider, &registry, &config)
        .await
        .unwrap();

    let last_req = provider.last_request().unwrap();
    let system_msgs: Vec<_> = last_req
        .messages
        .iter()
        .filter(|m| matches!(m.role, ChatRole::System))
        .collect();
    assert_eq!(system_msgs.len(), 2);
    assert_eq!(
        system_msgs[0].content.joined_texts().as_deref(),
        Some("Runtime timezone: Asia/Kolkata.")
    );
    assert_eq!(
        system_msgs[1].content.joined_texts().as_deref(),
        Some("Conversation summary.")
    );
}

// ── Test: Agent Loop Token Tracking ────────────────────────────────────

#[tokio::test]
async fn test_agent_loop_tracks_tokens() {
    let provider = FakeProvider::new(vec![FakeResponse {
        assistant_text: Some("test".into()),
        tool_calls: Vec::new(),
        stop_reason: Some(StopReason::Completed("stop".to_string())),
        token_usage: Some((100, 50)),
    }]);

    let ctx = test_context();
    let registry = toy_registry();
    let config = AgentLoopConfig::default();

    let result = run_agent(&ctx, &provider, &registry, &config)
        .await
        .unwrap();

    let tokens = result.metadata.token_usage;
    assert_eq!(tokens.input_tokens, 100);
    assert_eq!(tokens.output_tokens, 50);
}

// ── Test: Silent Completion (Phase 6) ──────────────────────────────────

#[tokio::test]
async fn test_silent_completion_when_no_text_and_no_tools() {
    // Provider returns no tool calls and no assistant text
    let provider = FakeProvider::new(vec![FakeResponse {
        assistant_text: None,
        tool_calls: Vec::new(),
        stop_reason: Some(StopReason::Completed("stop".to_string())),
        token_usage: None,
    }]);

    let ctx = test_context();
    let registry = toy_registry();
    let config = AgentLoopConfig::default();

    let result = run_agent(&ctx, &provider, &registry, &config)
        .await
        .unwrap();

    // Should return Silent, not FinalText("")
    assert!(matches!(result.outcome, AgentOutcome::Silent));
    assert_eq!(result.metadata.iterations, 1);
    assert_eq!(provider.call_count(), 1);
}

#[tokio::test]
async fn test_silent_completion_with_empty_text() {
    // Provider returns empty assistant text (edge case)
    let provider = FakeProvider::new(vec![FakeResponse {
        assistant_text: Some("".into()),
        tool_calls: Vec::new(),
        stop_reason: Some(StopReason::Completed("stop".to_string())),
        token_usage: None,
    }]);

    let ctx = test_context();
    let registry = toy_registry();
    let config = AgentLoopConfig::default();

    let result = run_agent(&ctx, &provider, &registry, &config)
        .await
        .unwrap();

    // Empty text should also be treated as Silent
    assert!(matches!(result.outcome, AgentOutcome::Silent));
    assert_eq!(result.metadata.iterations, 1);
}
