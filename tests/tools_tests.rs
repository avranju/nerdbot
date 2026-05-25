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
use genai::chat::ToolCall;
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
        call_id: "1".into(),
        fn_name: "nonexistent_tool".into(),
        fn_arguments: serde_json::json!({}),
        thought_signatures: None,
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
        call_id: "1".into(),
        fn_name: "read_file".into(),
        fn_arguments: serde_json::json!({"path": "test.txt"}),
        thought_signatures: None,
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
    assert_eq!(spec.name.as_str(), "schedule_job");
    assert_eq!(
        spec.description.as_deref(),
        Some("Schedule a one-shot or recurring agent task.")
    );
    assert!(spec.schema.as_ref().unwrap().is_object());
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
        call_id: "1".into(),
        fn_name: "read_file".into(),
        fn_arguments: serde_json::json!({"this_is_not_a_real_field": true, "nested": {"deep": {"value": 42}}}),
        thought_signatures: None,
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

// ── send_user_message Tool Tests ───────────────────────────────────────

#[test]
fn test_send_user_message_schema_has_required_text() {
    let tool = nerdbot::tools::telegram::SendTelegramMessage;
    let schema = tool.input_schema();
    let required: Vec<&str> = schema["required"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert!(required.contains(&"text"));
}

#[test]
fn test_send_user_message_schema_has_optional_fields() {
    let tool = nerdbot::tools::telegram::SendTelegramMessage;
    let schema = tool.input_schema();
    let props = &schema["properties"];
    assert!(props.get("chat_id").is_some());
    assert!(props.get("formatting").is_some());
    assert!(props.get("disable_notification").is_some());
}

#[tokio::test]
async fn test_send_user_message_requires_text() {
    let tool = nerdbot::tools::telegram::SendTelegramMessage;
    let ctx = ToolContext {
        run_mode: nerdbot::agent::run_mode::AgentRunMode::InteractiveReply {
            chat_id: 123,
            user_id: 456,
        },
        workspace_root: PathBuf::from("/tmp"),
        telegram_token: "bot-token".into(),
        allowed_chat_ids: vec![123],
        allowed_user_ids: vec![],
        pool: None,
        scheduler_notifier: None,
    };

    // Missing text should fail
    let result = Tool::execute(&tool, serde_json::json!({}), ctx).await;
    assert!(result.is_err());
    if let Err(AgentError::Generic(msg)) = result {
        assert!(msg.contains("text is required"));
    } else {
        panic!("expected Generic error, got {:?}", result);
    }
}

#[tokio::test]
async fn test_send_user_message_uses_context_chat_id_when_missing() {
    let tool = nerdbot::tools::telegram::SendTelegramMessage;
    let ctx = ToolContext {
        run_mode: nerdbot::agent::run_mode::AgentRunMode::InteractiveReply {
            chat_id: 999,
            user_id: 100,
        },
        workspace_root: PathBuf::from("/tmp"),
        telegram_token: "".into(), // Empty token = mock mode
        allowed_chat_ids: vec![999],
        allowed_user_ids: vec![],
        pool: None,
        scheduler_notifier: None,
    };

    // With empty token, should succeed in mock mode and use chat_id from context
    let result = Tool::execute(
        &tool,
        serde_json::json!({"text": "hello"}),
        ctx,
    )
    .await
    .unwrap();
    assert!(result.success);
    assert!(result.data["sent"].is_boolean());
    assert_eq!(result.data["chat_id"].as_i64(), Some(999));
}

#[tokio::test]
async fn test_send_user_message_uses_explicit_chat_id() {
    let tool = nerdbot::tools::telegram::SendTelegramMessage;
    let ctx = ToolContext {
        run_mode: nerdbot::agent::run_mode::AgentRunMode::InteractiveReply {
            chat_id: 111,
            user_id: 100,
        },
        workspace_root: PathBuf::from("/tmp"),
        telegram_token: "".into(), // Mock mode
        allowed_chat_ids: vec![222], // Different from both context and explicit
        allowed_user_ids: vec![],
        pool: None,
        scheduler_notifier: None,
    };

    // Explicit chat_id should be used
    let result = Tool::execute(
        &tool,
        serde_json::json!({"text": "hello", "chat_id": 222}),
        ctx,
    )
    .await
    .unwrap();
    assert!(result.success);
    assert_eq!(result.data["chat_id"].as_i64(), Some(222));
}

#[tokio::test]
async fn test_send_user_message_enforces_chat_allowlist() {
    let tool = nerdbot::tools::telegram::SendTelegramMessage;
    let ctx = ToolContext {
        run_mode: nerdbot::agent::run_mode::AgentRunMode::InteractiveReply {
            chat_id: 111,
            user_id: 100,
        },
        workspace_root: PathBuf::from("/tmp"),
        telegram_token: "".into(),
        allowed_chat_ids: vec![999], // Only 999 allowed
        allowed_user_ids: vec![],
        pool: None,
        scheduler_notifier: None,
    };

    // Target chat 222 is not in allowlist
    let result = Tool::execute(
        &tool,
        serde_json::json!({"text": "hello", "chat_id": 222}),
        ctx,
    )
    .await;
    assert!(result.is_err());
    assert!(matches!(result.unwrap_err(), AgentError::PermissionDenied));
}

#[tokio::test]
async fn test_send_user_message_enforces_user_allowlist() {
    let tool = nerdbot::tools::telegram::SendTelegramMessage;
    let ctx = ToolContext {
        run_mode: nerdbot::agent::run_mode::AgentRunMode::InteractiveReply {
            chat_id: 111,
            user_id: 100,
        },
        workspace_root: PathBuf::from("/tmp"),
        telegram_token: "".into(),
        allowed_chat_ids: vec![111],
        allowed_user_ids: vec![999], // Only user 999 allowed
        pool: None,
        scheduler_notifier: None,
    };

    // User 100 is not in allowlist
    let result = Tool::execute(
        &tool,
        serde_json::json!({"text": "hello", "chat_id": 111}),
        ctx,
    )
    .await;
    assert!(result.is_err());
    assert!(matches!(result.unwrap_err(), AgentError::PermissionDenied));
}

#[tokio::test]
async fn test_send_user_message_formatting_markdown() {
    let tool = nerdbot::tools::telegram::SendTelegramMessage;
    let ctx = ToolContext {
        run_mode: nerdbot::agent::run_mode::AgentRunMode::InteractiveReply {
            chat_id: 123,
            user_id: 456,
        },
        workspace_root: PathBuf::from("/tmp"),
        telegram_token: "".into(), // Empty = mock mode
        allowed_chat_ids: vec![123],
        allowed_user_ids: vec![],
        pool: None,
        scheduler_notifier: None,
    };

    let result = Tool::execute(
        &tool,
        serde_json::json!({"text": "hello", "formatting": "markdown"}),
        ctx,
    )
    .await
    .unwrap();
    assert!(result.success);
    assert_eq!(result.data["formatting"].as_str(), Some("markdown"));
}

#[tokio::test]
async fn test_send_user_message_disable_notification() {
    let tool = nerdbot::tools::telegram::SendTelegramMessage;
    let ctx = ToolContext {
        run_mode: nerdbot::agent::run_mode::AgentRunMode::InteractiveReply {
            chat_id: 123,
            user_id: 456,
        },
        workspace_root: PathBuf::from("/tmp"),
        telegram_token: "".into(), // Empty = mock mode
        allowed_chat_ids: vec![123],
        allowed_user_ids: vec![],
        pool: None,
        scheduler_notifier: None,
    };

    let result = Tool::execute(
        &tool,
        serde_json::json!({"text": "hello", "disable_notification": true}),
        ctx,
    )
    .await
    .unwrap();
    assert!(result.success);
    assert_eq!(result.data["disable_notification"].as_bool(), Some(true));
}

#[tokio::test]
async fn test_send_user_message_no_chat_id_in_internal_run() {
    let tool = nerdbot::tools::telegram::SendTelegramMessage;
    let ctx = ToolContext {
        run_mode: nerdbot::agent::run_mode::AgentRunMode::Internal {
            reason: "test".into(),
        },
        workspace_root: PathBuf::from("/tmp"),
        telegram_token: "".into(),
        allowed_chat_ids: vec![],
        allowed_user_ids: vec![],
        pool: None,
        scheduler_notifier: None,
    };

    // No chat_id in context and none provided explicitly
    let result = Tool::execute(&tool, serde_json::json!({"text": "hello"}), ctx).await;
    assert!(result.is_err());
    if let Err(AgentError::Generic(msg)) = result {
        assert!(msg.contains("chat_id is required"));
    } else {
        panic!("expected Generic error, got {:?}", result);
    }
}

// ── ShellExecute Tool Tests ─────────────────────────────────────────────

#[test]
fn test_shell_execute_name() {
    let tool = nerdbot::tools::shell::ShellExecute::new(
        nerdbot::tools::shell::ShellConfig::default(),
    );
    assert_eq!(tool.name(), "shell_execute");
}

#[test]
fn test_shell_execute_description_non_empty() {
    let tool = nerdbot::tools::shell::ShellExecute::new(
        nerdbot::tools::shell::ShellConfig::default(),
    );
    let desc = tool.description();
    assert!(!desc.is_empty());
    assert!(desc.len() > 50);
    assert!(desc.to_lowercase().contains("sandbox"));
}

#[test]
fn test_shell_execute_schema_has_required_command() {
    let tool = nerdbot::tools::shell::ShellExecute::new(
        nerdbot::tools::shell::ShellConfig::default(),
    );
    let schema = tool.input_schema();
    let required: Vec<&str> = schema["required"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert!(required.contains(&"command"));
}

#[test]
fn test_shell_execute_schema_has_optional_fields() {
    let tool = nerdbot::tools::shell::ShellExecute::new(
        nerdbot::tools::shell::ShellConfig::default(),
    );
    let schema = tool.input_schema();
    let props = &schema["properties"];
    assert!(props.get("command").is_some());
    assert!(props.get("working_directory").is_some());
    assert!(props.get("timeout_seconds").is_some());
    assert!(props.get("max_output_bytes").is_some());
}

#[tokio::test]
async fn test_shell_execute_requires_command() {
    let tool = nerdbot::tools::shell::ShellExecute::new(
        nerdbot::tools::shell::ShellConfig::default(),
    );
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

    // Missing command should fail
    let result = Tool::execute(&tool, serde_json::json!({}), ctx).await;
    assert!(result.is_err());
    if let Err(AgentError::InvalidToolArgs(msg)) = result {
        assert!(msg.contains("command"));
    } else {
        panic!("expected InvalidToolArgs error, got {:?}", result);
    }
}

#[tokio::test]
async fn test_shell_execute_denied_command() {
    let tool = nerdbot::tools::shell::ShellExecute::new(
        nerdbot::tools::shell::ShellConfig::default(),
    );
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

    // 'rm' is in the default denylist
    let result = Tool::execute(
        &tool,
        serde_json::json!({ "command": "rm -rf /" }),
        ctx,
    )
    .await;
    assert!(result.is_err());
    if let Err(AgentError::ToolExecution(msg)) = result {
        assert!(msg.contains("denied"));
    } else {
        panic!("expected ToolExecution error, got {:?}", result);
    }
}

#[tokio::test]
async fn test_shell_execute_allows_safe_command() {
    let tool = nerdbot::tools::shell::ShellExecute::new(
        nerdbot::tools::shell::ShellConfig::default(),
    );
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

    // 'echo' is not denied and allowlist is empty (allow all)
    let result = Tool::execute(
        &tool,
        serde_json::json!({ "command": "echo hello world" }),
        ctx,
    )
    .await;
    assert!(result.is_ok());
    let output = result.unwrap();
    assert!(output.success);
    assert!(output.summary.contains("successfully"));
    assert!(output.data["output"].as_str().unwrap().contains("hello world"));
}

#[tokio::test]
async fn test_shell_execute_allows_specific_command_with_allowlist() {
    let config = nerdbot::tools::shell::ShellConfig {
        allowed_commands: vec!["echo".into(), "ls".into(), "cat".into()],
        denied_commands: vec!["rm".into()],
        max_output_bytes: 1_048_576,
        timeout_secs: 30,
    };
    let tool = nerdbot::tools::shell::ShellExecute::new(config);
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

    // 'echo' is in allowlist
    let result = Tool::execute(
        &tool,
        serde_json::json!({ "command": "echo allowed" }),
        ctx.clone(),
    )
    .await;
    assert!(result.is_ok());

    // 'python3' is not in allowlist
    let result = Tool::execute(
        &tool,
        serde_json::json!({ "command": "python3 --version" }),
        ctx,
    )
    .await;
    assert!(result.is_err());
    if let Err(AgentError::ToolExecution(msg)) = result {
        assert!(msg.contains("allowlist"));
    } else {
        panic!("expected ToolExecution error, got {:?}", result);
    }
}

#[tokio::test]
async fn test_shell_execute_respects_denylist_over_allowlist() {
    let config = nerdbot::tools::shell::ShellConfig {
        allowed_commands: vec!["rm".into()], // rm is in allowlist
        denied_commands: vec!["rm".into()], // but also in denylist
        max_output_bytes: 1_048_576,
        timeout_secs: 30,
    };
    let tool = nerdbot::tools::shell::ShellExecute::new(config);
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

    // Denylist should take precedence
    let result = Tool::execute(
        &tool,
        serde_json::json!({ "command": "rm -rf /" }),
        ctx,
    )
    .await;
    assert!(result.is_err());
    if let Err(AgentError::ToolExecution(msg)) = result {
        assert!(msg.contains("denied"));
    } else {
        panic!("expected ToolExecution error, got {:?}", result);
    }
}

#[tokio::test]
async fn test_shell_execute_output_truncation() {
    let config = nerdbot::tools::shell::ShellConfig {
        allowed_commands: vec![],
        denied_commands: vec![],
        max_output_bytes: 1024,
        timeout_secs: 30,
    };
    let tool = nerdbot::tools::shell::ShellExecute::new(config);
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

    // Generate output larger than 1024 bytes
    let result = Tool::execute(
        &tool,
        serde_json::json!({ "command": "yes A | head -2000" }),
        ctx,
    )
    .await;
    assert!(result.is_ok());
    let output = result.unwrap();
    assert!(!output.success); // Should be non-success due to truncation
    assert!(output.data["truncated"].as_bool().unwrap());
    assert!(output.data["output"].as_str().unwrap().contains("truncated"));
}

#[tokio::test]
async fn test_shell_execute_custom_timeout() {
    let tool = nerdbot::tools::shell::ShellExecute::new(
        nerdbot::tools::shell::ShellConfig::default(),
    );
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

    // Custom timeout of 5 seconds
    let result = Tool::execute(
        &tool,
        serde_json::json!({
            "command": "echo hello",
            "timeout_seconds": 5
        }),
        ctx,
    )
    .await;
    assert!(result.is_ok());
    let output = result.unwrap();
    assert_eq!(output.data["timeout_seconds"].as_u64().unwrap(), 5);
}

#[tokio::test]
async fn test_shell_execute_working_directory() {
    let tmp = tempfile::tempdir().unwrap();
    // Create a subdirectory
    let sub = tmp.path().join("subdir");
    std::fs::create_dir(&sub).unwrap();

    let tool = nerdbot::tools::shell::ShellExecute::new(
        nerdbot::tools::shell::ShellConfig::default(),
    );
    let ctx = ToolContext {
        run_mode: nerdbot::agent::run_mode::AgentRunMode::InteractiveReply {
            chat_id: 1,
            user_id: 1,
        },
        workspace_root: tmp.path().to_path_buf(),
        telegram_token: "test".into(),
        allowed_chat_ids: vec![],
        allowed_user_ids: vec![],
        pool: None,
        scheduler_notifier: None,
    };

    let result = Tool::execute(
        &tool,
        serde_json::json!({
            "command": "pwd",
            "working_directory": "subdir"
        }),
        ctx,
    )
    .await;
    assert!(result.is_ok());
    let output = result.unwrap();
    assert!(output.data["working_directory"].as_str().unwrap().contains("subdir"));
}

#[tokio::test]
async fn test_shell_execute_empty_command() {
    let tool = nerdbot::tools::shell::ShellExecute::new(
        nerdbot::tools::shell::ShellConfig::default(),
    );
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

    // Empty command string
    let result = Tool::execute(
        &tool,
        serde_json::json!({ "command": "" }),
        ctx,
    )
    .await;
    assert!(result.is_err());
    // Empty string is caught by argument parsing (InvalidToolArgs) or by command validation (ToolExecution)
    match result.unwrap_err() {
        AgentError::InvalidToolArgs(msg) | AgentError::ToolExecution(msg) => {
            assert!(msg.contains("empty"));
        }
        other => {
            panic!("expected InvalidToolArgs or ToolExecution error, got {:?}", other);
        }
    }
}

#[test]
fn test_shell_config_default_denylist() {
    let config = nerdbot::tools::shell::ShellConfig::default();
    assert!(config.denied_commands.contains(&"rm".into()));
    assert!(config.denied_commands.contains(&"chmod".into()));
    assert!(config.denied_commands.contains(&"curl".into()));
    assert!(config.denied_commands.contains(&"wget".into()));
    assert!(config.allowed_commands.is_empty());
}

#[test]
fn test_shell_execute_schema_type_constraints() {
    let tool = nerdbot::tools::shell::ShellExecute::new(
        nerdbot::tools::shell::ShellConfig::default(),
    );
    let schema = tool.input_schema();

    // command should be string type
    assert_eq!(
        schema["properties"]["command"]["type"].as_str().unwrap(),
        "string"
    );

    // timeout_seconds should have min/max
    assert_eq!(
        schema["properties"]["timeout_seconds"]["minimum"].as_u64().unwrap(),
        1
    );
    assert_eq!(
        schema["properties"]["timeout_seconds"]["maximum"].as_u64().unwrap(),
        300
    );
}

#[tokio::test]
async fn test_shell_execute_shell_special_chars() {
    let tool = nerdbot::tools::shell::ShellExecute::new(
        nerdbot::tools::shell::ShellConfig::default(),
    );
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

    // Shell pipes and redirects should work
    let result = Tool::execute(
        &tool,
        serde_json::json!({
            "command": "echo hello | grep hello"
        }),
        ctx,
    )
    .await;
    assert!(result.is_ok());
    let output = result.unwrap();
    assert!(output.success);
}

#[tokio::test]
async fn test_shell_execute_multiple_denylisted_commands() {
    let config = nerdbot::tools::shell::ShellConfig {
        allowed_commands: vec![],
        denied_commands: vec![
            "rm".into(),
            "chmod".into(),
            "chown".into(),
            "mkfs".into(),
            "dd".into(),
            "wget".into(),
            "curl".into(),
        ],
        max_output_bytes: 1_048_576,
        timeout_secs: 30,
    };
    let tool = nerdbot::tools::shell::ShellExecute::new(config);
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

    // All denied commands should be blocked
    for cmd in &["rm", "chmod", "chown", "mkfs", "dd", "wget", "curl"] {
        let result = Tool::execute(
            &tool,
            serde_json::json!({ "command": format!("{cmd} --version") }),
            ctx.clone(),
        )
        .await;
        assert!(
            result.is_err(),
            "command '{cmd}' should be denied"
        );
    }
}

#[tokio::test]
async fn test_shell_execute_custom_max_output_bytes() {
    let config = nerdbot::tools::shell::ShellConfig {
        allowed_commands: vec![],
        denied_commands: vec![],
        max_output_bytes: 1024,
        timeout_secs: 30,
    };
    let tool = nerdbot::tools::shell::ShellExecute::new(config);
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

    // Generate > 1024 bytes; min clamp is 1024 so truncation triggers
    let result = Tool::execute(
        &tool,
        serde_json::json!({
            "command": "yes A | head -2000",
            "max_output_bytes": 1024
        }),
        ctx,
    )
    .await;
    assert!(result.is_ok());
    let output = result.unwrap();
    assert!(output.data["truncated"].as_bool().unwrap());
}

#[tokio::test]
async fn test_shell_execute_non_zero_exit_code() {
    let tool = nerdbot::tools::shell::ShellExecute::new(
        nerdbot::tools::shell::ShellConfig::default(),
    );
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

    // 'false' always exits with code 1
    let result = Tool::execute(
        &tool,
        serde_json::json!({ "command": "false" }),
        ctx,
    )
    .await;
    assert!(result.is_ok());
    let output = result.unwrap();
    assert!(!output.success);
    assert_eq!(output.data["exit_code"].as_i64(), Some(1));
    assert!(output.summary.contains("failed"));
}

#[tokio::test]
async fn test_shell_execute_timeout_enforcement() {
    let tool = nerdbot::tools::shell::ShellExecute::new(
        nerdbot::tools::shell::ShellConfig::default(),
    );
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

    // sleep 5 with 1 second timeout should be killed
    let start = std::time::Instant::now();
    let result = Tool::execute(
        &tool,
        serde_json::json!({
            "command": "sleep 5",
            "timeout_seconds": 1
        }),
        ctx,
    )
    .await;
    let elapsed = start.elapsed();
    assert!(result.is_ok());
    let output = result.unwrap();
    assert!(!output.success);
    assert_eq!(output.data["exit_code"].as_i64(), Some(-1));
    assert!(elapsed.as_secs() < 3, "timeout should fire quickly, took {:?}", elapsed);
}
