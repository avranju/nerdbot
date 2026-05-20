#![allow(
    dead_code,
    unused,
    unused_imports,
    unused_variables,
    unused_assignments
)]
//! Tests for tool system: traits, registry, and stub tools.

use std::path::PathBuf;

use nerdbot::error::AgentError;
use nerdbot::llm::types::{ToolCall, ToolSpec};
use nerdbot::tools::registry::ToolRegistry;
use nerdbot::tools::traits::{Tool, ToolContext, ToolOutput};

// ── ToolContext ──────────────────────────────────────────────────────────

#[test]
fn test_tool_context_clone() {
    let ctx = ToolContext {
        run_mode: nerdbot::agent::run_mode::AgentRunMode::InteractiveReply {
            chat_id: 123,
            user_id: 456,
        },
        workspace_root: PathBuf::from("/workspace"),
        telegram_token: "bot-token".into(),
        allowed_chat_ids: vec![123, 456],
        allowed_user_ids: vec![],
        pool: None,
        scheduler_notifier: None,
    };
    let cloned = ctx.clone();
    assert_eq!(ctx.workspace_root, cloned.workspace_root);
    assert_eq!(ctx.telegram_token, cloned.telegram_token);
    assert_eq!(ctx.allowed_chat_ids, cloned.allowed_chat_ids);
}

#[test]
fn test_tool_context_clone_run_mode() {
    let ctx = ToolContext {
        run_mode: nerdbot::agent::run_mode::AgentRunMode::ScheduledJob {
            job_id: "job-1".into(),
            default_chat_id: 789,
            notify_on_completion: true,
        },
        workspace_root: PathBuf::from("/data"),
        telegram_token: "token".into(),
        allowed_chat_ids: vec![],
        allowed_user_ids: vec![],
        pool: None,
        scheduler_notifier: None,
    };
    let cloned = ctx.clone();
    assert_eq!(ctx.run_mode, cloned.run_mode);
}

// ── ToolOutput ───────────────────────────────────────────────────────────

#[test]
fn test_tool_output_success() {
    let output = ToolOutput {
        success: true,
        data: serde_json::json!({"count": 5}),
        summary: "Found 5 items".into(),
    };
    assert!(output.success);
    assert_eq!(output.summary, "Found 5 items");
}

#[test]
fn test_tool_output_error() {
    let output = ToolOutput {
        success: false,
        data: serde_json::json!({"error": "not found"}),
        summary: "Item not found".into(),
    };
    assert!(!output.success);
}

#[test]
fn test_tool_output_serialization() {
    let output = ToolOutput {
        success: true,
        data: serde_json::json!({"files": ["a.txt", "b.txt"]}),
        summary: "Listed 2 files".into(),
    };
    let json = serde_json::to_string(&output).unwrap();
    assert!(json.contains("true"));
    assert!(json.contains("a.txt"));
    assert!(json.contains("b.txt"));
}

#[test]
fn test_tool_output_roundtrip() {
    let output = ToolOutput {
        success: false,
        data: serde_json::json!({"code": 404}),
        summary: "Resource not found".into(),
    };
    let json = serde_json::to_string(&output).unwrap();
    let restored: ToolOutput = serde_json::from_str(&json).unwrap();
    assert_eq!(output.success, restored.success);
    assert_eq!(output.data, restored.data);
    assert_eq!(output.summary, restored.summary);
}

// ── ToolRegistry ─────────────────────────────────────────────────────────

#[test]
fn test_registry_new_is_empty() {
    let registry = ToolRegistry::new();
    assert!(registry.is_empty());
    assert_eq!(registry.len(), 0);
}

#[test]
fn test_registry_register_and_len() {
    let mut registry = ToolRegistry::new();
    registry.register(nerdbot::tools::files::ReadFile);
    assert_eq!(registry.len(), 1);
    assert!(!registry.is_empty());
}

#[test]
fn test_registry_register_multiple() {
    let mut registry = ToolRegistry::new();
    registry.register(nerdbot::tools::files::ReadFile);
    registry.register(nerdbot::tools::files::WriteFile);
    registry.register(nerdbot::tools::schedule::ScheduleJob);
    assert_eq!(registry.len(), 3);
}

#[test]
fn test_registry_specs() {
    let mut registry = ToolRegistry::new();
    registry.register(nerdbot::tools::files::ReadFile);
    registry.register(nerdbot::tools::web::WebSearch);

    let specs = registry.specs();
    assert_eq!(specs.len(), 2);

    let names: Vec<&str> = specs.iter().map(|s| s.name.as_str()).collect();
    assert!(names.contains(&"read_file"));
    assert!(names.contains(&"web_search"));
}

#[test]
fn test_registry_specs_are_unique() {
    let mut registry = ToolRegistry::new();
    registry.register(nerdbot::tools::files::ReadFile);
    registry.register(nerdbot::tools::files::WriteFile);
    registry.register(nerdbot::tools::files::AppendFile);
    registry.register(nerdbot::tools::files::ListDirectory);

    let specs = registry.specs();
    let names: Vec<&str> = specs.iter().map(|s| s.name.as_str()).collect();
    // All names should be unique
    assert_eq!(
        names.len(),
        names.iter().collect::<std::collections::HashSet<_>>().len()
    );
}

#[tokio::test]
async fn test_registry_execute_unknown_tool() {
    let registry = ToolRegistry::new();
    let call = ToolCall {
        id: "1".into(),
        name: "nonexistent_tool".into(),
        arguments: serde_json::json!({}),
    };
    let ctx = ToolContext {
        run_mode: nerdbot::agent::run_mode::AgentRunMode::InteractiveReply {
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
    assert!(result.is_err());
    assert!(matches!(result.unwrap_err(), AgentError::ToolNotFound(_)));
}

#[tokio::test]
async fn test_registry_execute_stub_tool_returns_not_implemented() {
    let mut registry = ToolRegistry::new();
    registry.register(nerdbot::tools::files::ReadFile);

    let call = ToolCall {
        id: "1".into(),
        name: "read_file".into(),
        arguments: serde_json::json!({"path": "test.txt"}),
    };
    let ctx = ToolContext {
        run_mode: nerdbot::agent::run_mode::AgentRunMode::InteractiveReply {
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
    assert!(result.is_err());
    // The stub returns a Generic error with "not yet implemented"
    if let Err(AgentError::Generic(msg)) = result {
        assert!(msg.contains("not yet implemented"));
    } else {
        panic!("expected Generic error, got {:?}", result);
    }
}

#[test]
fn test_registry_tool_spec_validation() {
    let mut registry = ToolRegistry::new();
    registry.register(nerdbot::tools::schedule::ScheduleJob);

    let specs = registry.specs();
    assert_eq!(specs.len(), 1);

    let spec = &specs[0];
    assert_eq!(spec.name, "schedule_job");
    assert_eq!(
        spec.description,
        "Schedule a one-shot or recurring agent task."
    );
    assert!(spec.input_schema.is_object());
}

#[test]
fn test_schedule_job_schema_documents_host_timezone_default() {
    let tool = nerdbot::tools::schedule::ScheduleJob;
    let schema = tool.input_schema();

    let description = schema["properties"]["timezone"]["description"]
        .as_str()
        .expect("timezone description should be present");

    assert!(description.contains("host system timezone"));
    assert!(description.contains("falling back to UTC"));
}

#[test]
fn test_registry_default() {
    let registry: ToolRegistry = Default::default();
    assert!(registry.is_empty());
}

// ── Stub Tools ───────────────────────────────────────────────────────────

#[test]
fn test_read_file_name() {
    let tool = nerdbot::tools::files::ReadFile;
    assert_eq!(tool.name(), "read_file");
}

#[test]
fn test_write_file_name() {
    let tool = nerdbot::tools::files::WriteFile;
    assert_eq!(tool.name(), "write_file");
}

#[test]
fn test_append_file_name() {
    let tool = nerdbot::tools::files::AppendFile;
    assert_eq!(tool.name(), "append_file");
}

#[test]
fn test_list_directory_name() {
    let tool = nerdbot::tools::files::ListDirectory;
    assert_eq!(tool.name(), "list_directory");
}

#[test]
fn test_schedule_job_name() {
    let tool = nerdbot::tools::schedule::ScheduleJob;
    assert_eq!(tool.name(), "schedule_job");
}

#[test]
fn test_list_jobs_name() {
    let tool = nerdbot::tools::schedule::ListJobs;
    assert_eq!(tool.name(), "list_jobs");
}

#[test]
fn test_delete_job_name() {
    let tool = nerdbot::tools::schedule::DeleteJob;
    assert_eq!(tool.name(), "delete_job");
}

#[test]
fn test_run_job_now_name() {
    let tool = nerdbot::tools::schedule::RunJobNow;
    assert_eq!(tool.name(), "run_job_now");
}

#[test]
fn test_send_user_message_name() {
    let tool = nerdbot::tools::telegram::SendTelegramMessage;
    assert_eq!(tool.name(), "send_user_message");
}

#[test]
fn test_web_search_name() {
    let tool = nerdbot::tools::web::WebSearch;
    assert_eq!(tool.name(), "web_search");
}

#[test]
fn test_web_fetch_name() {
    let tool = nerdbot::tools::web::WebFetch;
    assert_eq!(tool.name(), "web_fetch");
}

#[test]
fn test_stub_tool_descriptions_non_empty() {
    let tools: Vec<&dyn Tool> = vec![
        &nerdbot::tools::files::ReadFile,
        &nerdbot::tools::files::WriteFile,
        &nerdbot::tools::files::AppendFile,
        &nerdbot::tools::files::ListDirectory,
        &nerdbot::tools::schedule::ScheduleJob,
        &nerdbot::tools::schedule::ListJobs,
        &nerdbot::tools::schedule::DeleteJob,
        &nerdbot::tools::schedule::RunJobNow,
        &nerdbot::tools::telegram::SendTelegramMessage,
        &nerdbot::tools::web::WebSearch,
        &nerdbot::tools::web::WebFetch,
    ];

    for tool in tools {
        let desc = tool.description();
        assert!(
            !desc.is_empty(),
            "tool {} should have a non-empty description",
            tool.name()
        );
        assert!(
            desc.len() > 5,
            "tool {} description should be descriptive enough",
            tool.name()
        );
    }
}

#[test]
fn test_stub_tool_input_schema_is_object() {
    let tools: Vec<&dyn Tool> = vec![
        &nerdbot::tools::files::ReadFile,
        &nerdbot::tools::files::WriteFile,
        &nerdbot::tools::schedule::ScheduleJob,
        &nerdbot::tools::telegram::SendTelegramMessage,
        &nerdbot::tools::web::WebSearch,
    ];

    for tool in tools {
        let schema = tool.input_schema();
        assert!(
            schema.is_object(),
            "tool {} input schema should be a JSON object",
            tool.name()
        );
    }
}

#[tokio::test]
async fn test_stub_tool_execute_fails_gracefully() {
    let tool = nerdbot::tools::files::ReadFile;
    let ctx = ToolContext {
        run_mode: nerdbot::agent::run_mode::AgentRunMode::InteractiveReply {
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
    let result = Tool::execute(&tool, serde_json::json!({}), ctx).await;
    assert!(result.is_err());
}

#[test]
fn test_registry_mixed_tools() {
    let mut registry = ToolRegistry::new();
    registry.register(nerdbot::tools::files::ReadFile);
    registry.register(nerdbot::tools::schedule::DeleteJob);
    registry.register(nerdbot::tools::web::WebFetch);

    assert_eq!(registry.len(), 3);

    let specs = registry.specs();
    let names: Vec<&str> = specs.iter().map(|s| s.name.as_str()).collect();
    assert!(names.contains(&"read_file"));
    assert!(names.contains(&"delete_job"));
    assert!(names.contains(&"web_fetch"));
}

#[tokio::test]
async fn test_registry_execute_with_invalid_args() {
    let mut registry = ToolRegistry::new();
    registry.register(nerdbot::tools::files::ReadFile);

    // Pass completely invalid arguments to a stub tool
    let call = ToolCall {
        id: "1".into(),
        name: "read_file".into(),
        arguments: serde_json::json!({"this_is_not_a_real_field": true, "nested": {"deep": {"value": 42}}}),
    };
    let ctx = ToolContext {
        run_mode: nerdbot::agent::run_mode::AgentRunMode::InteractiveReply {
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
    // Should fail since it's a stub, but should not panic
    assert!(result.is_err());
}
