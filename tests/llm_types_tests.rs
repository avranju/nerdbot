#![allow(dead_code, unused, unused_imports, unused_variables, unused_assignments)]
//! Tests for LLM provider-neutral types in `src/llm/types.rs`.

use nerdbot::llm::types::*;
use serde_json::json;

// ── Role ─────────────────────────────────────────────────────────────────

#[test]
fn test_role_serialization() {
    assert_eq!(serde_json::to_string(&Role::System).unwrap(), "\"system\"");
    assert_eq!(serde_json::to_string(&Role::User).unwrap(), "\"user\"");
    assert_eq!(serde_json::to_string(&Role::Assistant).unwrap(), "\"assistant\"");
    assert_eq!(serde_json::to_string(&Role::Tool).unwrap(), "\"tool\"");
}

#[test]
fn test_role_deserialization() {
    assert_eq!(
        serde_json::from_str::<Role>("\"system\"").unwrap(),
        Role::System
    );
    assert_eq!(
        serde_json::from_str::<Role>("\"user\"").unwrap(),
        Role::User
    );
    assert_eq!(
        serde_json::from_str::<Role>("\"assistant\"").unwrap(),
        Role::Assistant
    );
    assert_eq!(
        serde_json::from_str::<Role>("\"tool\"").unwrap(),
        Role::Tool
    );
}

#[test]
fn test_role_eq() {
    assert_eq!(Role::System, Role::System);
    assert_ne!(Role::System, Role::User);
    assert_ne!(Role::Assistant, Role::Tool);
}

// ── MessageContent ───────────────────────────────────────────────────────

#[test]
fn test_message_content_text() {
    let content = MessageContent::Text("hello".into());
    assert_eq!(
        serde_json::to_string(&content).unwrap(),
        "{\"Text\":\"hello\"}"
    );
}

#[test]
fn test_message_content_parts() {
    let content = MessageContent::Parts(vec![ContentPart::Text("a".into())]);
    let s = serde_json::to_string(&content).unwrap();
    assert!(s.contains("Parts"));
    assert!(s.contains("\"a\""));
}

// ── ContentPart ──────────────────────────────────────────────────────────

#[test]
fn test_content_part_text() {
    let part = ContentPart::Text("world".into());
    let s = serde_json::to_string(&part).unwrap();
    assert!(s.contains("Text"));
}

#[test]
fn test_content_part_tool_call() {
    let call = ToolCall {
        id: "call1".into(),
        name: "read_file".into(),
        arguments: json!({"path": "test.txt"}),
    };
    let part = ContentPart::ToolCall(call);
    let s = serde_json::to_string(&part).unwrap();
    assert!(s.contains("ToolCall"));
    assert!(s.contains("read_file"));
}

#[test]
fn test_content_part_tool_result() {
    let result = ToolResult {
        tool_call_id: "call1".into(),
        status: ToolExecutionStatus::Success,
        content: json!({"content": "file contents"}),
    };
    let part = ContentPart::ToolResult(result);
    let s = serde_json::to_string(&part).unwrap();
    assert!(s.contains("ToolResult"));
}

// ── ToolSpec ─────────────────────────────────────────────────────────────

#[test]
fn test_tool_spec_new() {
    let spec = ToolSpec::new(
        "read_file",
        "Read a file from the workspace.",
        json!({"type": "object", "properties": {"path": {"type": "string"}}}),
    );
    assert_eq!(spec.name, "read_file");
    assert_eq!(spec.description, "Read a file from the workspace.");
    assert!(spec.input_schema.is_object());
}

#[test]
fn test_tool_spec_serialization() {
    let spec = ToolSpec::new("test", "A test tool", json!({"type": "object"}));
    let json = serde_json::to_string(&spec).unwrap();
    assert!(json.contains("test"));
    assert!(json.contains("A test tool"));
    assert!(json.contains("type"));
}

#[test]
fn test_tool_spec_deserialization() {
    let input = r#"{
        "name": "list_dir",
        "description": "List directory contents",
        "input_schema": {"type": "object", "properties": {"path": {"type": "string"}}}
    }"#;
    let spec: ToolSpec = serde_json::from_str(input).unwrap();
    assert_eq!(spec.name, "list_dir");
    assert_eq!(spec.description, "List directory contents");
}

// ── ToolCall ─────────────────────────────────────────────────────────────

#[test]
fn test_tool_call_serialization() {
    let call = ToolCall {
        id: "call-abc-123".into(),
        name: "web_search".into(),
        arguments: json!({"query": "Rust programming"}),
    };
    let json = serde_json::to_string(&call).unwrap();
    assert!(json.contains("call-abc-123"));
    assert!(json.contains("web_search"));
    assert!(json.contains("Rust programming"));
}

#[test]
fn test_tool_call_roundtrip() {
    let call = ToolCall {
        id: "call-xyz".into(),
        name: "schedule_job".into(),
        arguments: json!({"prompt": "daily report", "schedule_type": "cron"}),
    };
    let json = serde_json::to_string(&call).unwrap();
    let restored: ToolCall = serde_json::from_str(&json).unwrap();
    assert_eq!(call.id, restored.id);
    assert_eq!(call.name, restored.name);
    assert_eq!(call.arguments, restored.arguments);
}

// ── ToolResult ───────────────────────────────────────────────────────────

#[test]
fn test_tool_result_success() {
    let result = ToolResult {
        tool_call_id: "call1".into(),
        status: ToolExecutionStatus::Success,
        content: json!({"files": ["a.txt", "b.txt"]}),
    };
    let s = serde_json::to_string(&result).unwrap();
    assert!(s.contains("success"));
}

#[test]
fn test_tool_result_error() {
    let result = ToolResult {
        tool_call_id: "call2".into(),
        status: ToolExecutionStatus::Error {
            error: "file not found".into(),
        },
        content: json!({}),
    };
    let json = serde_json::to_string(&result).unwrap();
    assert!(json.contains("error"));
    assert!(json.contains("file not found"));
}

#[test]
fn test_tool_result_roundtrip() {
    let result = ToolResult {
        tool_call_id: "call-42".into(),
        status: ToolExecutionStatus::Error {
            error: "timeout".into(),
        },
        content: json!({"retry_after": 5}),
    };
    let json = serde_json::to_string(&result).unwrap();
    let restored: ToolResult = serde_json::from_str(&json).unwrap();
    assert_eq!(result.tool_call_id, restored.tool_call_id);
    assert_eq!(result.status, restored.status);
    assert_eq!(result.content, restored.content);
}

// ── ToolExecutionStatus ──────────────────────────────────────────────────

#[test]
fn test_tool_execution_status_success() {
    assert!(ToolExecutionStatus::Success.is_success());
}

#[test]
fn test_tool_execution_status_error() {
    let status = ToolExecutionStatus::Error {
        error: "something broke".into(),
    };
    assert!(!status.is_success());
    assert!(matches!(&status, ToolExecutionStatus::Error { error } if error == "something broke"));
}

#[test]
fn test_tool_execution_status_serialization_success() {
    let s = serde_json::to_string(&ToolExecutionStatus::Success).unwrap();
    assert_eq!(s, "{\"status\":\"success\"}");
}

#[test]
fn test_tool_execution_status_serialization_error() {
    let status = ToolExecutionStatus::Error {
        error: "disk full".into(),
    };
    let s = serde_json::to_string(&status).unwrap();
    assert!(s.contains("error"));
    assert!(s.contains("disk full"));
}

#[test]
fn test_tool_execution_status_deserialization() {
    let s: ToolExecutionStatus = serde_json::from_str("{\"status\":\"success\"}").unwrap();
    assert!(s.is_success());
}

// ── Message constructors ─────────────────────────────────────────────────

#[test]
fn test_message_new() {
    let msg = Message::new(Role::User, MessageContent::Text("hi".into()));
    assert_eq!(msg.role, Role::User);
    assert!(matches!(msg.content, MessageContent::Text(ref t) if t == "hi"));
    assert!(msg.metadata.is_none());
}

#[test]
fn test_message_system() {
    let msg = Message::system("You are a helpful assistant.");
    assert_eq!(msg.role, Role::System);
    match msg.content {
        MessageContent::Text(text) => assert_eq!(text, "You are a helpful assistant."),
        _ => panic!("expected Text"),
    }
}

#[test]
fn test_message_user() {
    let msg = Message::user("What is Rust?");
    assert_eq!(msg.role, Role::User);
    match msg.content {
        MessageContent::Text(text) => assert_eq!(text, "What is Rust?"),
        _ => panic!("expected Text"),
    }
}

#[test]
fn test_message_assistant() {
    let msg = Message::assistant("Rust is a systems programming language.");
    assert_eq!(msg.role, Role::Assistant);
    match msg.content {
        MessageContent::Text(text) => assert_eq!(text, "Rust is a systems programming language."),
        _ => panic!("expected Text"),
    }
}

#[test]
fn test_message_assistant_tool_calls() {
    let calls = vec![
        ToolCall { id: "1".into(), name: "read_file".into(), arguments: json!({"path": "a"}) },
        ToolCall { id: "2".into(), name: "web_search".into(), arguments: json!({"query": "b"}) },
    ];
    let msg = Message::assistant_tool_calls(calls.clone());
    assert_eq!(msg.role, Role::Assistant);
    match msg.content {
        MessageContent::Parts(parts) => {
            assert_eq!(parts.len(), 2);
            assert!(matches!(parts[0], ContentPart::ToolCall(_)));
            assert!(matches!(parts[1], ContentPart::ToolCall(_)));
        }
        _ => panic!("expected Parts"),
    }
}

#[test]
fn test_message_tool_result() {
    let call = ToolCall {
        id: "call-1".into(),
        name: "read_file".into(),
        arguments: json!({"path": "x.txt"}),
    };
    let msg = Message::tool_result(
        &call,
        ToolExecutionStatus::Success,
        json!({"content": "hello world"}),
    );
    assert_eq!(msg.role, Role::Tool);
    match msg.content {
        MessageContent::Parts(parts) => {
            assert_eq!(parts.len(), 1);
            match &parts[0] {
                ContentPart::ToolResult(tr) => {
                    assert_eq!(tr.tool_call_id, "call-1");
                    assert!(tr.status.is_success());
                    assert_eq!(tr.content, json!({"content": "hello world"}));
                }
                _ => panic!("expected ToolResult"),
            }
        }
        _ => panic!("expected Parts"),
    }
}

#[test]
fn test_message_metadata_passthrough() {
    let meta = json!({"timestamp": 1234});
    let msg = Message {
        role: Role::User,
        content: MessageContent::Text("test".into()),
        metadata: Some(meta.clone()),
    };
    assert_eq!(msg.metadata.unwrap(), meta);
}

// ── ModelRequest ─────────────────────────────────────────────────────────

#[test]
fn test_model_request_default() {
    let req = ModelRequest::default();
    assert!(req.messages.is_empty());
    assert!(req.tools.is_empty());
    assert!(req.model.is_empty());
    assert!(req.temperature.is_none());
    assert!(req.max_output_tokens.is_none());
    assert!(req.metadata.is_none());
}

#[test]
fn test_model_request_with_messages() {
    let msg = Message::user("test");
    let req = ModelRequest::default().with_messages(vec![msg.clone()]);
    assert_eq!(req.messages.len(), 1);
    assert_eq!(req.messages[0], msg);
}

#[test]
fn test_model_request_with_tools() {
    let spec = ToolSpec::new("test", "a test", json!({}));
    let req = ModelRequest::default().with_tools(vec![spec.clone()]);
    assert_eq!(req.tools.len(), 1);
    assert_eq!(req.tools[0], spec);
}

#[test]
fn test_model_request_with_tool_results() {
    let call = ToolCall {
        id: "c1".into(),
        name: "test".into(),
        arguments: json!({}),
    };
    let result = ToolResult {
        tool_call_id: "c1".into(),
        status: ToolExecutionStatus::Success,
        content: json!({}),
    };
    let req = ModelRequest::default().with_tool_results(vec![result]);
    // Should have one message (the assistant message with tool results)
    assert_eq!(req.messages.len(), 1);
    match &req.messages[0].content {
        MessageContent::Parts(parts) => {
            assert!(matches!(parts[0], ContentPart::ToolResult(_)));
        }
        _ => panic!("expected Parts"),
    }
}

// ── ModelResponse ────────────────────────────────────────────────────────

#[test]
fn test_model_response_has_tool_calls() {
    let response = ModelResponse {
        assistant_text: None,
        tool_calls: vec![ToolCall { id: "1".into(), name: "x".into(), arguments: json!({}) }],
        finish_reason: FinishReason::ToolUse,
        provider_metadata: None,
    };
    assert!(response.has_tool_calls());
}

#[test]
fn test_model_response_no_tool_calls() {
    let response = ModelResponse {
        assistant_text: Some("done".into()),
        tool_calls: vec![],
        finish_reason: FinishReason::Completed,
        provider_metadata: None,
    };
    assert!(!response.has_tool_calls());
}

#[test]
fn test_model_response_is_final() {
    let response = ModelResponse {
        assistant_text: Some("final answer".into()),
        tool_calls: vec![],
        finish_reason: FinishReason::Completed,
        provider_metadata: None,
    };
    assert!(response.is_final());
}

#[test]
fn test_model_response_is_not_final_with_tools() {
    let response = ModelResponse {
        assistant_text: Some("I'll check...".into()),
        tool_calls: vec![ToolCall { id: "1".into(), name: "x".into(), arguments: json!({}) }],
        finish_reason: FinishReason::ToolUse,
        provider_metadata: None,
    };
    assert!(!response.is_final());
}

// ── FinishReason ─────────────────────────────────────────────────────────

#[test]
fn test_finish_reason_serialization() {
    assert_eq!(
        serde_json::to_string(&FinishReason::Completed).unwrap(),
        "\"completed\""
    );
    assert_eq!(
        serde_json::to_string(&FinishReason::ToolUse).unwrap(),
        "\"tooluse\""
    );
    assert_eq!(
        serde_json::to_string(&FinishReason::Length).unwrap(),
        "\"length\""
    );
    assert_eq!(
        serde_json::to_string(&FinishReason::Safety).unwrap(),
        "\"safety\""
    );
    assert_eq!(
        serde_json::to_string(&FinishReason::Error).unwrap(),
        "\"error\""
    );
}

#[test]
fn test_finish_reason_unknown() {
    let reason = FinishReason::Unknown("custom_reason".into());
    let s = serde_json::to_string(&reason).unwrap();
    assert!(s.contains("custom_reason"));
}

// ── TokenEstimate ────────────────────────────────────────────────────────

#[test]
fn test_token_estimate_new() {
    let est = TokenEstimate::new(100, 50);
    assert_eq!(est.input_tokens, 100);
    assert_eq!(est.output_tokens, 50);
    assert_eq!(est.total_tokens, 150);
}

#[test]
fn test_token_estimate_serialization() {
    let est = TokenEstimate::new(200, 100);
    let s = serde_json::to_string(&est).unwrap();
    assert!(s.contains("input_tokens"));
    assert!(s.contains("output_tokens"));
    assert!(s.contains("total_tokens"));
}

// ── MessageContent serialization round-trips ─────────────────────────────

#[test]
fn test_message_content_roundtrip_text() {
    let content = MessageContent::Text("hello world".into());
    let json = serde_json::to_string(&content).unwrap();
    let restored: MessageContent = serde_json::from_str(&json).unwrap();
    assert_eq!(content, restored);
}

#[test]
fn test_message_content_roundtrip_parts() {
    let content = MessageContent::Parts(vec![
        ContentPart::Text("part1".into()),
        ContentPart::Text("part2".into()),
    ]);
    let json = serde_json::to_string(&content).unwrap();
    let restored: MessageContent = serde_json::from_str(&json).unwrap();
    assert_eq!(content, restored);
}

// ── MessageContentParts round-trip with tool calls ───────────────────────

#[test]
fn test_message_content_parts_with_tool_calls_roundtrip() {
    let call = ToolCall {
        id: "tc-1".into(),
        name: "web_search".into(),
        arguments: json!({"query": "test"}),
    };
    let content = MessageContent::Parts(vec![ContentPart::ToolCall(call)]);
    let json = serde_json::to_string(&content).unwrap();
    let restored: MessageContent = serde_json::from_str(&json).unwrap();
    assert_eq!(content, restored);
}

// ── Role Clone / Debug ───────────────────────────────────────────────────

#[test]
fn test_role_clone() {
    let role = Role::Assistant;
    let cloned = role.clone();
    assert_eq!(role, cloned);
}

#[test]
fn test_role_debug() {
    let debug_str = format!("{:?}", Role::System);
    assert!(debug_str.contains("System"));
}

// ── ToolCall Clone / Debug ───────────────────────────────────────────────

#[test]
fn test_tool_call_clone() {
    let call = ToolCall {
        id: "1".into(),
        name: "test".into(),
        arguments: json!({"a": 1}),
    };
    let cloned = call.clone();
    assert_eq!(call.id, cloned.id);
    assert_eq!(call.name, cloned.name);
    assert_eq!(call.arguments, cloned.arguments);
}

#[test]
fn test_tool_call_debug() {
    let debug_str = format!("{:?}", ToolCall {
        id: "1".into(),
        name: "test".into(),
        arguments: json!({}),
    });
    assert!(debug_str.contains("ToolCall"));
}

// ── ModelRequest Clone / Debug ───────────────────────────────────────────

#[test]
fn test_model_request_clone() {
    let req = ModelRequest::default()
        .with_messages(vec![Message::user("test")])
        .with_tools(vec![ToolSpec::new("t", "d", json!({}))]);
    let cloned = req.clone();
    assert_eq!(req.messages.len(), cloned.messages.len());
    assert_eq!(req.tools.len(), cloned.tools.len());
}

// ── MessageContent Clone ─────────────────────────────────────────────────

#[test]
fn test_message_content_clone() {
    let content = MessageContent::Text("test".into());
    let cloned = content.clone();
    assert_eq!(content, cloned);
}

#[test]
fn test_content_part_clone() {
    let part = ContentPart::Text("test".into());
    let cloned = part.clone();
    assert_eq!(part, cloned);
}

#[test]
fn test_tool_result_clone() {
    let result = ToolResult {
        tool_call_id: "1".into(),
        status: ToolExecutionStatus::Success,
        content: json!({}),
    };
    let cloned = result.clone();
    assert_eq!(result.tool_call_id, cloned.tool_call_id);
}

#[test]
fn test_model_response_clone() {
    let response = ModelResponse {
        assistant_text: Some("hi".into()),
        tool_calls: vec![],
        finish_reason: FinishReason::Completed,
        provider_metadata: None,
    };
    let cloned = response.clone();
    assert_eq!(response.assistant_text, cloned.assistant_text);
}
