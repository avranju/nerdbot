#![allow(
    dead_code,
    unused,
    unused_imports,
    unused_variables,
    unused_assignments
)]
//! Tests for tool system: traits, registry, and stub tools.

use std::path::PathBuf;

use genai::chat::ToolCall;
use nerdbot::error::AgentError;
use nerdbot::tools::registry::ToolRegistry;
use nerdbot::tools::traits::{Tool, ToolContext, ToolOutput};

// ── ToolContext ──────────────────────────────────────────────────────────

#[test]
fn test_tool_context_clone() {
    let ctx = ToolContext {
        run_mode: nerdbot::agent::run_mode::AgentRunMode::InteractiveReply {
            address: nerdbot::channel::ConversationAddress::telegram_chat(123),
            sender: nerdbot::channel::SenderIdentity::new((456).to_string(), None),
        },
        workspace_root: PathBuf::from("/workspace"),
        access_policy: nerdbot::channel::ChannelAccessPolicy {
            allowed_conversations: vec![123, 456]
                .into_iter()
                .map(|id| nerdbot::channel::ConversationAddressPattern {
                    channel_id: "telegram".to_string(),
                    conversation_id: id.to_string(),
                    thread_id: None,
                })
                .collect(),
            allowed_senders: Vec::<String>::new(),
        },
        channel_registry: None,
        pool: None,
        scheduler_notifier: None,
    };
    let cloned = ctx.clone();
    assert_eq!(ctx.workspace_root, cloned.workspace_root);
    assert_eq!(ctx.workspace_root, cloned.workspace_root);
    assert_eq!(
        ctx.access_policy.allowed_conversations,
        cloned.access_policy.allowed_conversations
    );
}

#[test]
fn test_tool_context_clone_run_mode() {
    let ctx = ToolContext {
        run_mode: nerdbot::agent::run_mode::AgentRunMode::ScheduledJob {
            job_id: "job-1".into(),
            default_address: nerdbot::channel::ConversationAddress::telegram_chat(789),
            notify_on_completion: true,
        },
        workspace_root: PathBuf::from("/data"),
        access_policy: nerdbot::channel::ChannelAccessPolicy::default(),
        channel_registry: None,
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
    registry.register(nerdbot::tools::files::ReadFile::default_for_test());
    assert_eq!(registry.len(), 1);
    assert!(!registry.is_empty());
}

#[test]
fn test_registry_register_multiple() {
    let mut registry = ToolRegistry::new();
    registry.register(nerdbot::tools::files::ReadFile::default_for_test());
    registry.register(nerdbot::tools::files::WriteFile::default_for_test());
    registry.register(nerdbot::tools::schedule::ScheduleJob);
    assert_eq!(registry.len(), 3);
}

#[test]
fn test_registry_specs() {
    let mut registry = ToolRegistry::new();
    registry.register(nerdbot::tools::files::ReadFile::default_for_test());
    registry.register(nerdbot::tools::web::WebSearch::new("test-key".into(), 5));

    let specs = registry.specs();
    assert_eq!(specs.len(), 2);

    let names: Vec<&str> = specs.iter().map(|s| s.name.as_str()).collect();
    assert!(names.contains(&"read_file"));
    assert!(names.contains(&"web_search"));
}

#[test]
fn test_registry_specs_are_unique() {
    let mut registry = ToolRegistry::new();
    registry.register(nerdbot::tools::files::ReadFile::default_for_test());
    registry.register(nerdbot::tools::files::WriteFile::default_for_test());
    registry.register(nerdbot::tools::files::AppendFile::default_for_test());
    registry.register(nerdbot::tools::files::ListDirectory::default_for_test());

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
            address: nerdbot::channel::ConversationAddress::telegram_chat(1),
            sender: nerdbot::channel::SenderIdentity::new((1).to_string(), None),
        },
        workspace_root: PathBuf::from("/tmp"),
        access_policy: nerdbot::channel::ChannelAccessPolicy::default(),
        channel_registry: None,
        pool: None,
        scheduler_notifier: None,
    };
    let result = registry.execute(&call, ctx).await;
    assert!(result.is_err());
    assert!(matches!(result.unwrap_err(), AgentError::ToolNotFound(_)));
}

#[tokio::test]
async fn test_registry_execute_file_operations() {
    let mut registry = ToolRegistry::new();
    registry.register(nerdbot::tools::files::WriteFile::default_for_test());
    registry.register(nerdbot::tools::files::ReadFile::default_for_test());
    registry.register(nerdbot::tools::files::AppendFile::default_for_test());
    registry.register(nerdbot::tools::files::ListDirectory::default_for_test());

    let tmp = tempfile::tempdir().unwrap();
    let workspace = tmp.path().to_path_buf();

    let ctx = ToolContext {
        run_mode: nerdbot::agent::run_mode::AgentRunMode::InteractiveReply {
            address: nerdbot::channel::ConversationAddress::telegram_chat(1),
            sender: nerdbot::channel::SenderIdentity::new((1).to_string(), None),
        },
        workspace_root: workspace.clone(),
        access_policy: nerdbot::channel::ChannelAccessPolicy::default(),
        channel_registry: None,
        pool: None,
        scheduler_notifier: None,
    };

    // Write a file
    let write_call = ToolCall {
        call_id: "1".into(),
        fn_name: "write_file".into(),
        fn_arguments: serde_json::json!({
            "path": "hello.txt",
            "content": "Hello, world!"
        }),
        thought_signatures: None,
    };
    let result = registry.execute(&write_call, ctx.clone()).await.unwrap();
    assert!(result.success);
    assert_eq!(result.data["bytes_written"].as_u64().unwrap(), 13);

    // Read the file back
    let read_call = ToolCall {
        call_id: "2".into(),
        fn_name: "read_file".into(),
        fn_arguments: serde_json::json!({"path": "hello.txt"}),
        thought_signatures: None,
    };
    let result = registry.execute(&read_call, ctx.clone()).await.unwrap();
    assert!(result.success);
    assert_eq!(result.data["content"].as_str().unwrap(), "Hello, world!");
    assert_eq!(result.data["bytes"].as_u64().unwrap(), 13);

    // Append to the file
    let append_call = ToolCall {
        call_id: "3".into(),
        fn_name: "append_file".into(),
        fn_arguments: serde_json::json!({
            "path": "hello.txt",
            "content": "\nAppended line"
        }),
        thought_signatures: None,
    };
    let result = registry.execute(&append_call, ctx.clone()).await.unwrap();
    assert!(result.success);
    assert_eq!(result.data["bytes_appended"].as_u64().unwrap(), 14);

    // Verify appended content
    let read_call2 = ToolCall {
        call_id: "4".into(),
        fn_name: "read_file".into(),
        fn_arguments: serde_json::json!({"path": "hello.txt"}),
        thought_signatures: None,
    };
    let result = registry.execute(&read_call2, ctx.clone()).await.unwrap();
    assert!(result.success);
    assert_eq!(
        result.data["content"].as_str().unwrap(),
        "Hello, world!\nAppended line"
    );

    // List directory
    let list_call = ToolCall {
        call_id: "5".into(),
        fn_name: "list_directory".into(),
        fn_arguments: serde_json::json!({}),
        thought_signatures: None,
    };
    let result = registry.execute(&list_call, ctx.clone()).await.unwrap();
    assert!(result.success);
    assert_eq!(result.data["count"].as_u64().unwrap(), 1);
    assert_eq!(
        result.data["entries"][0]["name"].as_str().unwrap(),
        "hello.txt"
    );
    assert_eq!(result.data["entries"][0]["type"].as_str().unwrap(), "file");

    // Write to a nested path (should auto-create directories)
    let nested_call = ToolCall {
        call_id: "6".into(),
        fn_name: "write_file".into(),
        fn_arguments: serde_json::json!({
            "path": "subdir/nested/deep.txt",
            "content": "deep content"
        }),
        thought_signatures: None,
    };
    let result = registry.execute(&nested_call, ctx.clone()).await.unwrap();
    assert!(result.success);

    // List subdir
    let list_subdir_call = ToolCall {
        call_id: "7".into(),
        fn_name: "list_directory".into(),
        fn_arguments: serde_json::json!({"path": "subdir"}),
        thought_signatures: None,
    };
    let result = registry
        .execute(&list_subdir_call, ctx.clone())
        .await
        .unwrap();
    assert!(result.success);
    assert_eq!(result.data["count"].as_u64().unwrap(), 1);
    assert_eq!(
        result.data["entries"][0]["name"].as_str().unwrap(),
        "nested"
    );
    assert_eq!(
        result.data["entries"][0]["type"].as_str().unwrap(),
        "directory"
    );

    // Sandbox: path traversal should fail
    let traversal_call = ToolCall {
        call_id: "8".into(),
        fn_name: "read_file".into(),
        fn_arguments: serde_json::json!({"path": "../../etc/passwd"}),
        thought_signatures: None,
    };
    let result = registry.execute(&traversal_call, ctx).await;
    assert!(result.is_err());
    assert!(matches!(result.unwrap_err(), AgentError::SandboxViolation));
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
    let tool = nerdbot::tools::files::ReadFile::default_for_test();
    assert_eq!(tool.name(), "read_file");
}

#[test]
fn test_write_file_name() {
    let tool = nerdbot::tools::files::WriteFile::default_for_test();
    assert_eq!(tool.name(), "write_file");
}

#[test]
fn test_append_file_name() {
    let tool = nerdbot::tools::files::AppendFile::default_for_test();
    assert_eq!(tool.name(), "append_file");
}

#[test]
fn test_list_directory_name() {
    let tool = nerdbot::tools::files::ListDirectory::default_for_test();
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
fn test_list_jobs_schema_supports_include_disabled() {
    let tool = nerdbot::tools::schedule::ListJobs;
    let schema = tool.input_schema();
    assert_eq!(
        schema["properties"]["include_disabled"]["type"].as_str(),
        Some("boolean")
    );
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
    let tool = nerdbot::tools::messaging::SendUserMessage;
    assert_eq!(tool.name(), "send_user_message");
}

#[test]
fn test_web_search_name() {
    let tool = nerdbot::tools::web::WebSearch::new("test-key".into(), 5);
    assert_eq!(tool.name(), "web_search");
}

#[test]
fn test_web_fetch_name() {
    let tool = nerdbot::tools::web::WebFetch::new("test-key".into(), 8000);
    assert_eq!(tool.name(), "web_fetch");
}

#[test]
fn test_stub_tool_descriptions_non_empty() {
    let web_search = nerdbot::tools::web::WebSearch::new("test-key".into(), 5);
    let web_fetch = nerdbot::tools::web::WebFetch::new("test-key".into(), 8000);
    let read_file = nerdbot::tools::files::ReadFile::default_for_test();
    let write_file = nerdbot::tools::files::WriteFile::default_for_test();
    let append_file = nerdbot::tools::files::AppendFile::default_for_test();
    let list_directory = nerdbot::tools::files::ListDirectory::default_for_test();
    let tools: Vec<&dyn Tool> = vec![
        &read_file,
        &write_file,
        &append_file,
        &list_directory,
        &nerdbot::tools::schedule::ScheduleJob,
        &nerdbot::tools::schedule::ListJobs,
        &nerdbot::tools::schedule::DeleteJob,
        &nerdbot::tools::schedule::RunJobNow,
        &nerdbot::tools::messaging::SendUserMessage,
        &web_search,
        &web_fetch,
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
    let web_search = nerdbot::tools::web::WebSearch::new("test-key".into(), 5);
    let read_file = nerdbot::tools::files::ReadFile::default_for_test();
    let write_file = nerdbot::tools::files::WriteFile::default_for_test();
    let tools: Vec<&dyn Tool> = vec![
        &read_file,
        &write_file,
        &nerdbot::tools::schedule::ScheduleJob,
        &nerdbot::tools::messaging::SendUserMessage,
        &web_search,
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
async fn test_read_file_invalid_args_fails() {
    let tool = nerdbot::tools::files::ReadFile::default_for_test();
    let ctx = ToolContext {
        run_mode: nerdbot::agent::run_mode::AgentRunMode::InteractiveReply {
            address: nerdbot::channel::ConversationAddress::telegram_chat(1),
            sender: nerdbot::channel::SenderIdentity::new((1).to_string(), None),
        },
        workspace_root: PathBuf::from("/tmp"),
        access_policy: nerdbot::channel::ChannelAccessPolicy::default(),
        channel_registry: None,
        pool: None,
        scheduler_notifier: None,
    };
    // Missing required 'path' field should fail gracefully
    let result = Tool::execute(&tool, serde_json::json!({}), ctx).await;
    assert!(result.is_err());
}

// ── File I/O Error Path Tests ─────────────────────────────────────────

#[tokio::test]
async fn test_read_file_not_found() {
    let tool = nerdbot::tools::files::ReadFile::default_for_test();
    let ctx = ToolContext {
        run_mode: nerdbot::agent::run_mode::AgentRunMode::InteractiveReply {
            address: nerdbot::channel::ConversationAddress::telegram_chat(1),
            sender: nerdbot::channel::SenderIdentity::new((1).to_string(), None),
        },
        workspace_root: PathBuf::from("/tmp"),
        access_policy: nerdbot::channel::ChannelAccessPolicy::default(),
        channel_registry: None,
        pool: None,
        scheduler_notifier: None,
    };
    let result = Tool::execute(&tool, serde_json::json!({"path": "nonexistent.txt"}), ctx).await;
    assert!(result.is_err());
    assert!(matches!(result.unwrap_err(), AgentError::FileIo(_)));
}

#[tokio::test]
async fn test_write_file_missing_content() {
    let tool = nerdbot::tools::files::WriteFile::default_for_test();
    let ctx = ToolContext {
        run_mode: nerdbot::agent::run_mode::AgentRunMode::InteractiveReply {
            address: nerdbot::channel::ConversationAddress::telegram_chat(1),
            sender: nerdbot::channel::SenderIdentity::new((1).to_string(), None),
        },
        workspace_root: PathBuf::from("/tmp"),
        access_policy: nerdbot::channel::ChannelAccessPolicy::default(),
        channel_registry: None,
        pool: None,
        scheduler_notifier: None,
    };
    let result = Tool::execute(&tool, serde_json::json!({"path": "test.txt"}), ctx).await;
    assert!(result.is_err());
    if let Err(AgentError::InvalidToolArgs(msg)) = result {
        assert!(msg.contains("content"));
    } else {
        panic!("expected InvalidToolArgs, got {:?}", result);
    }
}

#[tokio::test]
async fn test_append_file_missing_content() {
    let tool = nerdbot::tools::files::AppendFile::default_for_test();
    let ctx = ToolContext {
        run_mode: nerdbot::agent::run_mode::AgentRunMode::InteractiveReply {
            address: nerdbot::channel::ConversationAddress::telegram_chat(1),
            sender: nerdbot::channel::SenderIdentity::new((1).to_string(), None),
        },
        workspace_root: PathBuf::from("/tmp"),
        access_policy: nerdbot::channel::ChannelAccessPolicy::default(),
        channel_registry: None,
        pool: None,
        scheduler_notifier: None,
    };
    let result = Tool::execute(&tool, serde_json::json!({"path": "test.txt"}), ctx).await;
    assert!(result.is_err());
    if let Err(AgentError::InvalidToolArgs(msg)) = result {
        assert!(msg.contains("content"));
    } else {
        panic!("expected InvalidToolArgs, got {:?}", result);
    }
}

#[tokio::test]
async fn test_write_file_size_exceeded() {
    let config = nerdbot::tools::files::FileConfig {
        max_read_bytes: 262_144,
        max_write_bytes: 10,
    };
    let tool = nerdbot::tools::files::WriteFile::new(config);
    let ctx = ToolContext {
        run_mode: nerdbot::agent::run_mode::AgentRunMode::InteractiveReply {
            address: nerdbot::channel::ConversationAddress::telegram_chat(1),
            sender: nerdbot::channel::SenderIdentity::new((1).to_string(), None),
        },
        workspace_root: PathBuf::from("/tmp"),
        access_policy: nerdbot::channel::ChannelAccessPolicy::default(),
        channel_registry: None,
        pool: None,
        scheduler_notifier: None,
    };
    let result = Tool::execute(
        &tool,
        serde_json::json!({"path": "test.txt", "content": "this is way too long"}),
        ctx,
    )
    .await;
    assert!(result.is_err());
    assert!(matches!(result.unwrap_err(), AgentError::FileIo(_)));
}

#[tokio::test]
async fn test_append_file_size_exceeded() {
    let config = nerdbot::tools::files::FileConfig {
        max_read_bytes: 262_144,
        max_write_bytes: 10,
    };
    let tool = nerdbot::tools::files::AppendFile::new(config);
    let ctx = ToolContext {
        run_mode: nerdbot::agent::run_mode::AgentRunMode::InteractiveReply {
            address: nerdbot::channel::ConversationAddress::telegram_chat(1),
            sender: nerdbot::channel::SenderIdentity::new((1).to_string(), None),
        },
        workspace_root: PathBuf::from("/tmp"),
        access_policy: nerdbot::channel::ChannelAccessPolicy::default(),
        channel_registry: None,
        pool: None,
        scheduler_notifier: None,
    };
    let result = Tool::execute(
        &tool,
        serde_json::json!({"path": "test.txt", "content": "this is way too long"}),
        ctx,
    )
    .await;
    assert!(result.is_err());
    assert!(matches!(result.unwrap_err(), AgentError::FileIo(_)));
}

#[tokio::test]
async fn test_append_file_to_existing_exceeds_limit() {
    let config = nerdbot::tools::files::FileConfig {
        max_read_bytes: 262_144,
        max_write_bytes: 50,
    };
    let tool = nerdbot::tools::files::AppendFile::new(config.clone());
    let tmp = tempfile::tempdir().unwrap();
    let ctx = ToolContext {
        run_mode: nerdbot::agent::run_mode::AgentRunMode::InteractiveReply {
            address: nerdbot::channel::ConversationAddress::telegram_chat(1),
            sender: nerdbot::channel::SenderIdentity::new((1).to_string(), None),
        },
        workspace_root: tmp.path().to_path_buf(),
        access_policy: nerdbot::channel::ChannelAccessPolicy::default(),
        channel_registry: None,
        pool: None,
        scheduler_notifier: None,
    };
    // First create a 40-byte file
    let write_tool = nerdbot::tools::files::WriteFile::new(config.clone());
    let write_result = Tool::execute(
        &write_tool,
        serde_json::json!({
            "path": "big.txt",
            "content": "0123456789012345678901234567890123456789"
        }),
        ctx.clone(),
    )
    .await
    .unwrap();
    assert!(write_result.success);

    // Try to append 20 bytes (40 + 20 = 60 > 50 limit)
    let result = Tool::execute(
        &tool,
        serde_json::json!({"path": "big.txt", "content": "01234567890123456789"}),
        ctx,
    )
    .await;
    assert!(result.is_err());
    assert!(matches!(result.unwrap_err(), AgentError::FileIo(_)));
}

#[test]
fn test_registry_mixed_tools() {
    let mut registry = ToolRegistry::new();
    registry.register(nerdbot::tools::files::ReadFile::default_for_test());
    registry.register(nerdbot::tools::schedule::DeleteJob);
    registry.register(nerdbot::tools::web::WebFetch::new("test-key".into(), 8000));

    assert_eq!(registry.len(), 3);

    let specs = registry.specs();
    let names: Vec<&str> = specs.iter().map(|s| s.name.as_str()).collect();
    assert!(names.contains(&"read_file"));
    assert!(names.contains(&"delete_job"));
    assert!(names.contains(&"web_fetch"));
}

#[tokio::test]
async fn test_registry_execute_with_missing_args() {
    let mut registry = ToolRegistry::new();
    registry.register(nerdbot::tools::files::ReadFile::default_for_test());

    // Missing required 'path' field should produce InvalidToolArgs
    let call = ToolCall {
        call_id: "1".into(),
        fn_name: "read_file".into(),
        fn_arguments: serde_json::json!({"other_field": "value"}),
        thought_signatures: None,
    };
    let ctx = ToolContext {
        run_mode: nerdbot::agent::run_mode::AgentRunMode::InteractiveReply {
            address: nerdbot::channel::ConversationAddress::telegram_chat(1),
            sender: nerdbot::channel::SenderIdentity::new((1).to_string(), None),
        },
        workspace_root: PathBuf::from("/tmp"),
        access_policy: nerdbot::channel::ChannelAccessPolicy::default(),
        channel_registry: None,
        pool: None,
        scheduler_notifier: None,
    };
    let result = registry.execute(&call, ctx).await;
    assert!(result.is_err());
    if let Err(AgentError::InvalidToolArgs(msg)) = result {
        assert!(msg.contains("path"));
    } else {
        panic!("expected InvalidToolArgs error, got {:?}", result);
    }
}

// ── send_user_message Tool Tests ───────────────────────────────────────

#[test]
fn test_send_user_message_schema_has_required_text() {
    let tool = nerdbot::tools::messaging::SendUserMessage;
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
    let tool = nerdbot::tools::messaging::SendUserMessage;
    let schema = tool.input_schema();
    let props = &schema["properties"];
    assert!(props.get("conversation").is_some());
    assert!(props.get("formatting").is_some());
    assert!(props.get("disable_notification").is_some());
}

#[tokio::test]
async fn test_send_user_message_requires_text() {
    let tool = nerdbot::tools::messaging::SendUserMessage;
    let ctx = ToolContext {
        run_mode: nerdbot::agent::run_mode::AgentRunMode::InteractiveReply {
            address: nerdbot::channel::ConversationAddress::telegram_chat(123),
            sender: nerdbot::channel::SenderIdentity::new((456).to_string(), None),
        },
        workspace_root: PathBuf::from("/tmp"),
        access_policy: nerdbot::channel::ChannelAccessPolicy {
            allowed_conversations: vec![123]
                .into_iter()
                .map(|id| nerdbot::channel::ConversationAddressPattern {
                    channel_id: "telegram".to_string(),
                    conversation_id: id.to_string(),
                    thread_id: None,
                })
                .collect(),
            allowed_senders: Vec::<String>::new(),
        },
        channel_registry: None,
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
async fn test_send_user_message_uses_context_conversation_when_missing() {
    let tool = nerdbot::tools::messaging::SendUserMessage;
    let ctx = ToolContext {
        run_mode: nerdbot::agent::run_mode::AgentRunMode::InteractiveReply {
            address: nerdbot::channel::ConversationAddress::telegram_chat(999),
            sender: nerdbot::channel::SenderIdentity::new((100).to_string(), None),
        },
        workspace_root: PathBuf::from("/tmp"),
        // Empty token = mock mode
        access_policy: nerdbot::channel::ChannelAccessPolicy {
            allowed_conversations: vec![999]
                .into_iter()
                .map(|id| nerdbot::channel::ConversationAddressPattern {
                    channel_id: "telegram".to_string(),
                    conversation_id: id.to_string(),
                    thread_id: None,
                })
                .collect(),
            allowed_senders: Vec::<String>::new(),
        },
        channel_registry: None,
        pool: None,
        scheduler_notifier: None,
    };

    // With no channel registry, should succeed in mock mode and use conversation from context
    let result = Tool::execute(&tool, serde_json::json!({"text": "hello"}), ctx)
        .await
        .unwrap();
    assert!(result.success);
    assert!(result.data["sent"].is_boolean());
    assert_eq!(
        result.data["conversation"]["channel_id"].as_str(),
        Some("telegram")
    );
    assert_eq!(
        result.data["conversation"]["conversation_id"].as_str(),
        Some("999")
    );
}

#[tokio::test]
async fn test_send_user_message_uses_explicit_conversation() {
    let tool = nerdbot::tools::messaging::SendUserMessage;
    let ctx = ToolContext {
        run_mode: nerdbot::agent::run_mode::AgentRunMode::InteractiveReply {
            address: nerdbot::channel::ConversationAddress::telegram_chat(111),
            sender: nerdbot::channel::SenderIdentity::new((100).to_string(), None),
        },
        workspace_root: PathBuf::from("/tmp"),
        // Mock mode
        access_policy: nerdbot::channel::ChannelAccessPolicy {
            allowed_conversations: vec![222]
                .into_iter()
                .map(|id| nerdbot::channel::ConversationAddressPattern {
                    channel_id: "telegram".to_string(),
                    conversation_id: id.to_string(),
                    thread_id: None,
                })
                .collect(), // Different from both context and explicit
            allowed_senders: Vec::<String>::new(),
        },
        channel_registry: None,
        pool: None,
        scheduler_notifier: None,
    };

    // Explicit conversation should be used
    let result = Tool::execute(
        &tool,
        serde_json::json!({
            "text": "hello",
            "conversation": {
                "channel_id": "telegram",
                "conversation_id": "222"
            }
        }),
        ctx,
    )
    .await
    .unwrap();
    assert!(result.success);
    assert_eq!(
        result.data["conversation"]["conversation_id"].as_str(),
        Some("222")
    );
}

#[tokio::test]
async fn test_send_user_message_enforces_chat_allowlist() {
    let tool = nerdbot::tools::messaging::SendUserMessage;
    let ctx = ToolContext {
        run_mode: nerdbot::agent::run_mode::AgentRunMode::InteractiveReply {
            address: nerdbot::channel::ConversationAddress::telegram_chat(111),
            sender: nerdbot::channel::SenderIdentity::new((100).to_string(), None),
        },
        workspace_root: PathBuf::from("/tmp"),
        access_policy: nerdbot::channel::ChannelAccessPolicy {
            allowed_conversations: vec![999]
                .into_iter()
                .map(|id| nerdbot::channel::ConversationAddressPattern {
                    channel_id: "telegram".to_string(),
                    conversation_id: id.to_string(),
                    thread_id: None,
                })
                .collect(), // Only 999 allowed
            allowed_senders: Vec::<String>::new(),
        },
        channel_registry: None,
        pool: None,
        scheduler_notifier: None,
    };

    // Target conversation 222 is not in allowlist
    let result = Tool::execute(
        &tool,
        serde_json::json!({
            "text": "hello",
            "conversation": {
                "channel_id": "telegram",
                "conversation_id": "222"
            }
        }),
        ctx,
    )
    .await;
    assert!(result.is_err());
    assert!(matches!(result.unwrap_err(), AgentError::PermissionDenied));
}

#[tokio::test]
async fn test_send_user_message_enforces_user_allowlist() {
    let tool = nerdbot::tools::messaging::SendUserMessage;
    let ctx = ToolContext {
        run_mode: nerdbot::agent::run_mode::AgentRunMode::InteractiveReply {
            address: nerdbot::channel::ConversationAddress::telegram_chat(111),
            sender: nerdbot::channel::SenderIdentity::new((100).to_string(), None),
        },
        workspace_root: PathBuf::from("/tmp"),
        access_policy: nerdbot::channel::ChannelAccessPolicy {
            allowed_conversations: vec![111]
                .into_iter()
                .map(|id| nerdbot::channel::ConversationAddressPattern {
                    channel_id: "telegram".to_string(),
                    conversation_id: id.to_string(),
                    thread_id: None,
                })
                .collect(),
            allowed_senders: vec![999].into_iter().map(|id| id.to_string()).collect(),
        },
        channel_registry: None, // Only user 999 allowed
        pool: None,
        scheduler_notifier: None,
    };

    // User 100 is not in allowlist
    let result = Tool::execute(
        &tool,
        serde_json::json!({
            "text": "hello",
            "conversation": {
                "channel_id": "telegram",
                "conversation_id": "111"
            }
        }),
        ctx,
    )
    .await;
    assert!(result.is_err());
    assert!(matches!(result.unwrap_err(), AgentError::PermissionDenied));
}

#[tokio::test]
async fn test_send_user_message_formatting_markdown() {
    let tool = nerdbot::tools::messaging::SendUserMessage;
    let ctx = ToolContext {
        run_mode: nerdbot::agent::run_mode::AgentRunMode::InteractiveReply {
            address: nerdbot::channel::ConversationAddress::telegram_chat(123),
            sender: nerdbot::channel::SenderIdentity::new((456).to_string(), None),
        },
        workspace_root: PathBuf::from("/tmp"),
        // Empty = mock mode
        access_policy: nerdbot::channel::ChannelAccessPolicy {
            allowed_conversations: vec![123]
                .into_iter()
                .map(|id| nerdbot::channel::ConversationAddressPattern {
                    channel_id: "telegram".to_string(),
                    conversation_id: id.to_string(),
                    thread_id: None,
                })
                .collect(),
            allowed_senders: Vec::<String>::new(),
        },
        channel_registry: None,
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
    let tool = nerdbot::tools::messaging::SendUserMessage;
    let ctx = ToolContext {
        run_mode: nerdbot::agent::run_mode::AgentRunMode::InteractiveReply {
            address: nerdbot::channel::ConversationAddress::telegram_chat(123),
            sender: nerdbot::channel::SenderIdentity::new((456).to_string(), None),
        },
        workspace_root: PathBuf::from("/tmp"),
        // Empty = mock mode
        access_policy: nerdbot::channel::ChannelAccessPolicy {
            allowed_conversations: vec![123]
                .into_iter()
                .map(|id| nerdbot::channel::ConversationAddressPattern {
                    channel_id: "telegram".to_string(),
                    conversation_id: id.to_string(),
                    thread_id: None,
                })
                .collect(),
            allowed_senders: Vec::<String>::new(),
        },
        channel_registry: None,
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
async fn test_send_user_message_no_conversation_in_internal_run() {
    let tool = nerdbot::tools::messaging::SendUserMessage;
    let ctx = ToolContext {
        run_mode: nerdbot::agent::run_mode::AgentRunMode::Internal {
            reason: "test".into(),
        },
        workspace_root: PathBuf::from("/tmp"),
        access_policy: nerdbot::channel::ChannelAccessPolicy::default(),
        channel_registry: None,
        pool: None,
        scheduler_notifier: None,
    };

    // No conversation in context and none provided explicitly
    let result = Tool::execute(&tool, serde_json::json!({"text": "hello"}), ctx).await;
    assert!(result.is_err());
    if let Err(AgentError::Generic(msg)) = result {
        assert!(msg.contains("conversation is required"));
    } else {
        panic!("expected Generic error, got {:?}", result);
    }
}

// ── ShellExecute Tool Tests ─────────────────────────────────────────────

#[test]
fn test_shell_execute_name() {
    let tool =
        nerdbot::tools::shell::ShellExecute::new(nerdbot::tools::shell::ShellConfig::default());
    assert_eq!(tool.name(), "shell_execute");
}

#[test]
fn test_shell_execute_description_non_empty() {
    let tool =
        nerdbot::tools::shell::ShellExecute::new(nerdbot::tools::shell::ShellConfig::default());
    let desc = tool.description();
    assert!(!desc.is_empty());
    assert!(desc.len() > 50);
    assert!(desc.to_lowercase().contains("sandbox"));
}

#[test]
fn test_shell_execute_schema_has_required_command() {
    let tool =
        nerdbot::tools::shell::ShellExecute::new(nerdbot::tools::shell::ShellConfig::default());
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
    let tool =
        nerdbot::tools::shell::ShellExecute::new(nerdbot::tools::shell::ShellConfig::default());
    let schema = tool.input_schema();
    let props = &schema["properties"];
    assert!(props.get("command").is_some());
    assert!(props.get("working_directory").is_some());
    assert!(props.get("timeout_seconds").is_some());
    assert!(props.get("max_output_bytes").is_some());
}

#[tokio::test]
async fn test_shell_execute_requires_command() {
    let tool =
        nerdbot::tools::shell::ShellExecute::new(nerdbot::tools::shell::ShellConfig::default());
    let ctx = ToolContext {
        run_mode: nerdbot::agent::run_mode::AgentRunMode::InteractiveReply {
            address: nerdbot::channel::ConversationAddress::telegram_chat(1),
            sender: nerdbot::channel::SenderIdentity::new((1).to_string(), None),
        },
        workspace_root: PathBuf::from("/tmp"),
        access_policy: nerdbot::channel::ChannelAccessPolicy::default(),
        channel_registry: None,
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
    let tool =
        nerdbot::tools::shell::ShellExecute::new(nerdbot::tools::shell::ShellConfig::default());
    let ctx = ToolContext {
        run_mode: nerdbot::agent::run_mode::AgentRunMode::InteractiveReply {
            address: nerdbot::channel::ConversationAddress::telegram_chat(1),
            sender: nerdbot::channel::SenderIdentity::new((1).to_string(), None),
        },
        workspace_root: PathBuf::from("/tmp"),
        access_policy: nerdbot::channel::ChannelAccessPolicy::default(),
        channel_registry: None,
        pool: None,
        scheduler_notifier: None,
    };

    // 'rm' is in the default denylist
    let result = Tool::execute(&tool, serde_json::json!({ "command": "rm -rf /" }), ctx).await;
    assert!(result.is_err());
    if let Err(AgentError::ToolExecution(msg)) = result {
        assert!(msg.contains("denied"));
    } else {
        panic!("expected ToolExecution error, got {:?}", result);
    }
}

#[tokio::test]
async fn test_shell_execute_allows_safe_command() {
    let tool =
        nerdbot::tools::shell::ShellExecute::new(nerdbot::tools::shell::ShellConfig::default());
    let ctx = ToolContext {
        run_mode: nerdbot::agent::run_mode::AgentRunMode::InteractiveReply {
            address: nerdbot::channel::ConversationAddress::telegram_chat(1),
            sender: nerdbot::channel::SenderIdentity::new((1).to_string(), None),
        },
        workspace_root: PathBuf::from("/tmp"),
        access_policy: nerdbot::channel::ChannelAccessPolicy::default(),
        channel_registry: None,
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
    assert!(
        output.data["output"]
            .as_str()
            .unwrap()
            .contains("hello world")
    );
}

#[tokio::test]
async fn test_shell_execute_allows_specific_command_with_allowlist() {
    let config = nerdbot::tools::shell::ShellConfig {
        allowed_commands: vec!["echo".into(), "ls".into(), "cat".into()],
        denied_commands: vec!["rm".into()],
        max_output_bytes: 1_048_576,
        timeout_secs: 30,
        sandbox_mode: "none".into(),
        network_access: "disabled".into(),
    };
    let tool = nerdbot::tools::shell::ShellExecute::new(config);
    let ctx = ToolContext {
        run_mode: nerdbot::agent::run_mode::AgentRunMode::InteractiveReply {
            address: nerdbot::channel::ConversationAddress::telegram_chat(1),
            sender: nerdbot::channel::SenderIdentity::new((1).to_string(), None),
        },
        workspace_root: PathBuf::from("/tmp"),
        access_policy: nerdbot::channel::ChannelAccessPolicy::default(),
        channel_registry: None,
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
        denied_commands: vec!["rm".into()],  // but also in denylist
        max_output_bytes: 1_048_576,
        timeout_secs: 30,
        sandbox_mode: "none".into(),
        network_access: "disabled".into(),
    };
    let tool = nerdbot::tools::shell::ShellExecute::new(config);
    let ctx = ToolContext {
        run_mode: nerdbot::agent::run_mode::AgentRunMode::InteractiveReply {
            address: nerdbot::channel::ConversationAddress::telegram_chat(1),
            sender: nerdbot::channel::SenderIdentity::new((1).to_string(), None),
        },
        workspace_root: PathBuf::from("/tmp"),
        access_policy: nerdbot::channel::ChannelAccessPolicy::default(),
        channel_registry: None,
        pool: None,
        scheduler_notifier: None,
    };

    // Denylist should take precedence
    let result = Tool::execute(&tool, serde_json::json!({ "command": "rm -rf /" }), ctx).await;
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
        sandbox_mode: "none".into(),
        network_access: "disabled".into(),
    };
    let tool = nerdbot::tools::shell::ShellExecute::new(config);
    let ctx = ToolContext {
        run_mode: nerdbot::agent::run_mode::AgentRunMode::InteractiveReply {
            address: nerdbot::channel::ConversationAddress::telegram_chat(1),
            sender: nerdbot::channel::SenderIdentity::new((1).to_string(), None),
        },
        workspace_root: PathBuf::from("/tmp"),
        access_policy: nerdbot::channel::ChannelAccessPolicy::default(),
        channel_registry: None,
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
    assert!(
        output.data["output"]
            .as_str()
            .unwrap()
            .contains("truncated")
    );
}

#[tokio::test]
async fn test_shell_execute_custom_timeout() {
    let tool =
        nerdbot::tools::shell::ShellExecute::new(nerdbot::tools::shell::ShellConfig::default());
    let ctx = ToolContext {
        run_mode: nerdbot::agent::run_mode::AgentRunMode::InteractiveReply {
            address: nerdbot::channel::ConversationAddress::telegram_chat(1),
            sender: nerdbot::channel::SenderIdentity::new((1).to_string(), None),
        },
        workspace_root: PathBuf::from("/tmp"),
        access_policy: nerdbot::channel::ChannelAccessPolicy::default(),
        channel_registry: None,
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

    let tool =
        nerdbot::tools::shell::ShellExecute::new(nerdbot::tools::shell::ShellConfig::default());
    let ctx = ToolContext {
        run_mode: nerdbot::agent::run_mode::AgentRunMode::InteractiveReply {
            address: nerdbot::channel::ConversationAddress::telegram_chat(1),
            sender: nerdbot::channel::SenderIdentity::new((1).to_string(), None),
        },
        workspace_root: tmp.path().to_path_buf(),
        access_policy: nerdbot::channel::ChannelAccessPolicy::default(),
        channel_registry: None,
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
    assert!(
        output.data["working_directory"]
            .as_str()
            .unwrap()
            .contains("subdir")
    );
}

#[tokio::test]
async fn test_shell_execute_empty_command() {
    let tool =
        nerdbot::tools::shell::ShellExecute::new(nerdbot::tools::shell::ShellConfig::default());
    let ctx = ToolContext {
        run_mode: nerdbot::agent::run_mode::AgentRunMode::InteractiveReply {
            address: nerdbot::channel::ConversationAddress::telegram_chat(1),
            sender: nerdbot::channel::SenderIdentity::new((1).to_string(), None),
        },
        workspace_root: PathBuf::from("/tmp"),
        access_policy: nerdbot::channel::ChannelAccessPolicy::default(),
        channel_registry: None,
        pool: None,
        scheduler_notifier: None,
    };

    // Empty command string
    let result = Tool::execute(&tool, serde_json::json!({ "command": "" }), ctx).await;
    assert!(result.is_err());
    // Empty string is caught by argument parsing (InvalidToolArgs) or by command validation (ToolExecution)
    match result.unwrap_err() {
        AgentError::InvalidToolArgs(msg) | AgentError::ToolExecution(msg) => {
            assert!(msg.contains("empty"));
        }
        other => {
            panic!(
                "expected InvalidToolArgs or ToolExecution error, got {:?}",
                other
            );
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
    let tool =
        nerdbot::tools::shell::ShellExecute::new(nerdbot::tools::shell::ShellConfig::default());
    let schema = tool.input_schema();

    // command should be string type
    assert_eq!(
        schema["properties"]["command"]["type"].as_str().unwrap(),
        "string"
    );

    // timeout_seconds should have min/max
    assert_eq!(
        schema["properties"]["timeout_seconds"]["minimum"]
            .as_u64()
            .unwrap(),
        1
    );
    assert_eq!(
        schema["properties"]["timeout_seconds"]["maximum"]
            .as_u64()
            .unwrap(),
        300
    );
}

#[tokio::test]
async fn test_shell_execute_shell_special_chars() {
    let tool =
        nerdbot::tools::shell::ShellExecute::new(nerdbot::tools::shell::ShellConfig::default());
    let ctx = ToolContext {
        run_mode: nerdbot::agent::run_mode::AgentRunMode::InteractiveReply {
            address: nerdbot::channel::ConversationAddress::telegram_chat(1),
            sender: nerdbot::channel::SenderIdentity::new((1).to_string(), None),
        },
        workspace_root: PathBuf::from("/tmp"),
        access_policy: nerdbot::channel::ChannelAccessPolicy::default(),
        channel_registry: None,
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
        sandbox_mode: "none".into(),
        network_access: "disabled".into(),
    };
    let tool = nerdbot::tools::shell::ShellExecute::new(config);
    let ctx = ToolContext {
        run_mode: nerdbot::agent::run_mode::AgentRunMode::InteractiveReply {
            address: nerdbot::channel::ConversationAddress::telegram_chat(1),
            sender: nerdbot::channel::SenderIdentity::new((1).to_string(), None),
        },
        workspace_root: PathBuf::from("/tmp"),
        access_policy: nerdbot::channel::ChannelAccessPolicy::default(),
        channel_registry: None,
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
        assert!(result.is_err(), "command '{cmd}' should be denied");
    }
}

#[tokio::test]
async fn test_shell_execute_custom_max_output_bytes() {
    let config = nerdbot::tools::shell::ShellConfig {
        allowed_commands: vec![],
        denied_commands: vec![],
        max_output_bytes: 1024,
        timeout_secs: 30,
        sandbox_mode: "none".into(),
        network_access: "disabled".into(),
    };
    let tool = nerdbot::tools::shell::ShellExecute::new(config);
    let ctx = ToolContext {
        run_mode: nerdbot::agent::run_mode::AgentRunMode::InteractiveReply {
            address: nerdbot::channel::ConversationAddress::telegram_chat(1),
            sender: nerdbot::channel::SenderIdentity::new((1).to_string(), None),
        },
        workspace_root: PathBuf::from("/tmp"),
        access_policy: nerdbot::channel::ChannelAccessPolicy::default(),
        channel_registry: None,
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
    let tool =
        nerdbot::tools::shell::ShellExecute::new(nerdbot::tools::shell::ShellConfig::default());
    let ctx = ToolContext {
        run_mode: nerdbot::agent::run_mode::AgentRunMode::InteractiveReply {
            address: nerdbot::channel::ConversationAddress::telegram_chat(1),
            sender: nerdbot::channel::SenderIdentity::new((1).to_string(), None),
        },
        workspace_root: PathBuf::from("/tmp"),
        access_policy: nerdbot::channel::ChannelAccessPolicy::default(),
        channel_registry: None,
        pool: None,
        scheduler_notifier: None,
    };

    // 'false' always exits with code 1
    let result = Tool::execute(&tool, serde_json::json!({ "command": "false" }), ctx).await;
    assert!(result.is_ok());
    let output = result.unwrap();
    assert!(!output.success);
    assert_eq!(output.data["exit_code"].as_i64(), Some(1));
    assert!(output.summary.contains("failed"));
}

#[tokio::test]
async fn test_shell_execute_timeout_enforcement() {
    let tool =
        nerdbot::tools::shell::ShellExecute::new(nerdbot::tools::shell::ShellConfig::default());
    let ctx = ToolContext {
        run_mode: nerdbot::agent::run_mode::AgentRunMode::InteractiveReply {
            address: nerdbot::channel::ConversationAddress::telegram_chat(1),
            sender: nerdbot::channel::SenderIdentity::new((1).to_string(), None),
        },
        workspace_root: PathBuf::from("/tmp"),
        access_policy: nerdbot::channel::ChannelAccessPolicy::default(),
        channel_registry: None,
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
    assert!(
        elapsed.as_secs() < 3,
        "timeout should fire quickly, took {:?}",
        elapsed
    );
}

// ── Bubblewrap Sandbox Mode Tests ─────────────────────────────────────

#[tokio::test]
async fn test_shell_execute_bwrap_missing_command() {
    let config = nerdbot::tools::shell::ShellConfig {
        allowed_commands: vec![],
        denied_commands: vec![],
        max_output_bytes: 1_048_576,
        timeout_secs: 30,
        sandbox_mode: "bwrap".into(),
        network_access: "disabled".into(),
    };
    let tool = nerdbot::tools::shell::ShellExecute::new(config);
    let ctx = ToolContext {
        run_mode: nerdbot::agent::run_mode::AgentRunMode::InteractiveReply {
            address: nerdbot::channel::ConversationAddress::telegram_chat(1),
            sender: nerdbot::channel::SenderIdentity::new((1).to_string(), None),
        },
        workspace_root: PathBuf::from("/tmp"),
        access_policy: nerdbot::channel::ChannelAccessPolicy::default(),
        channel_registry: None,
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
async fn test_shell_execute_bwrap_strict_missing_command() {
    let config = nerdbot::tools::shell::ShellConfig {
        allowed_commands: vec![],
        denied_commands: vec![],
        max_output_bytes: 1_048_576,
        timeout_secs: 30,
        sandbox_mode: "bwrap-strict".into(),
        network_access: "disabled".into(),
    };
    let tool = nerdbot::tools::shell::ShellExecute::new(config);
    let ctx = ToolContext {
        run_mode: nerdbot::agent::run_mode::AgentRunMode::InteractiveReply {
            address: nerdbot::channel::ConversationAddress::telegram_chat(1),
            sender: nerdbot::channel::SenderIdentity::new((1).to_string(), None),
        },
        workspace_root: PathBuf::from("/tmp"),
        access_policy: nerdbot::channel::ChannelAccessPolicy::default(),
        channel_registry: None,
        pool: None,
        scheduler_notifier: None,
    };

    let result = Tool::execute(&tool, serde_json::json!({}), ctx).await;
    assert!(result.is_err());
    if let Err(AgentError::InvalidToolArgs(msg)) = result {
        assert!(msg.contains("command"));
    } else {
        panic!("expected InvalidToolArgs error, got {:?}", result);
    }
}

#[test]
fn test_shell_execute_bwrap_policy_for_workspace() {
    let policy =
        nerdbot::tools::shell::BwrapPolicy::for_workspace(std::path::Path::new("/workspace"));
    // Should have workspace as read-write
    assert!(policy.writable_roots.iter().any(|p| p == "/workspace"));
}

#[test]
fn test_bwrap_available_is_callable() {
    // Just verify the function is callable; the result depends on the environment
    let available = nerdbot::tools::shell::bwrap_available();
    // The function should not panic
    let _ = available;
}

#[tokio::test]
async fn test_shell_execute_invalid_sandbox_mode() {
    let config = nerdbot::tools::shell::ShellConfig {
        allowed_commands: vec![],
        denied_commands: vec![],
        max_output_bytes: 1_048_576,
        timeout_secs: 30,
        sandbox_mode: "invalid-mode".into(),
        network_access: "disabled".into(),
    };
    let tool = nerdbot::tools::shell::ShellExecute::new(config);
    let ctx = ToolContext {
        run_mode: nerdbot::agent::run_mode::AgentRunMode::InteractiveReply {
            address: nerdbot::channel::ConversationAddress::telegram_chat(1),
            sender: nerdbot::channel::SenderIdentity::new((1).to_string(), None),
        },
        workspace_root: PathBuf::from("/tmp"),
        access_policy: nerdbot::channel::ChannelAccessPolicy::default(),
        channel_registry: None,
        pool: None,
        scheduler_notifier: None,
    };

    let result = Tool::execute(&tool, serde_json::json!({ "command": "echo hello" }), ctx).await;
    assert!(result.is_err());
    assert!(matches!(result.unwrap_err(), AgentError::Config(_)));
}

#[tokio::test]
async fn test_shell_execute_sandbox_mode_none_in_output() {
    let config = nerdbot::tools::shell::ShellConfig {
        allowed_commands: vec![],
        denied_commands: vec![],
        max_output_bytes: 1_048_576,
        timeout_secs: 30,
        sandbox_mode: "none".into(),
        network_access: "disabled".into(),
    };
    let tool = nerdbot::tools::shell::ShellExecute::new(config);
    let ctx = ToolContext {
        run_mode: nerdbot::agent::run_mode::AgentRunMode::InteractiveReply {
            address: nerdbot::channel::ConversationAddress::telegram_chat(1),
            sender: nerdbot::channel::SenderIdentity::new((1).to_string(), None),
        },
        workspace_root: PathBuf::from("/tmp"),
        access_policy: nerdbot::channel::ChannelAccessPolicy::default(),
        channel_registry: None,
        pool: None,
        scheduler_notifier: None,
    };

    let result = Tool::execute(&tool, serde_json::json!({ "command": "echo hello" }), ctx)
        .await
        .unwrap();
    assert!(result.success);
    assert_eq!(result.data["sandbox_mode"].as_str(), Some("none"));
}

#[tokio::test]
async fn test_shell_execute_sandbox_mode_denied_in_bwrap() {
    let config = nerdbot::tools::shell::ShellConfig {
        allowed_commands: vec![],
        denied_commands: vec!["rm".into()],
        max_output_bytes: 1_048_576,
        timeout_secs: 30,
        sandbox_mode: "bwrap".into(),
        network_access: "disabled".into(),
    };
    let tool = nerdbot::tools::shell::ShellExecute::new(config);
    let ctx = ToolContext {
        run_mode: nerdbot::agent::run_mode::AgentRunMode::InteractiveReply {
            address: nerdbot::channel::ConversationAddress::telegram_chat(1),
            sender: nerdbot::channel::SenderIdentity::new((1).to_string(), None),
        },
        workspace_root: PathBuf::from("/tmp"),
        access_policy: nerdbot::channel::ChannelAccessPolicy::default(),
        channel_registry: None,
        pool: None,
        scheduler_notifier: None,
    };

    // 'rm' is denied regardless of sandbox mode
    let result = Tool::execute(&tool, serde_json::json!({ "command": "rm -rf /" }), ctx).await;
    assert!(result.is_err());
    if let Err(AgentError::ToolExecution(msg)) = result {
        assert!(msg.contains("denied"));
    } else {
        panic!("expected ToolExecution error, got {:?}", result);
    }
}
