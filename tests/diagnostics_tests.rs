//! Integration tests for the opt-in Unix socket diagnostics transport.

use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Command;
use std::sync::Arc;

use nerdbot::agent::personality::Personality;
use nerdbot::config::AppConfig;
use nerdbot::context::budget::ContextBudget;
use nerdbot::context::compaction_service::CompactionService;
use nerdbot::context::compaction_worker::CompactionWorker;
use nerdbot::diagnostics::client::{render_human, render_json};
use nerdbot::diagnostics::protocol::{DiagnosticsRequest, DiagnosticsResponse};
use nerdbot::diagnostics::server::DiagnosticsServer;
use nerdbot::llm::fake::{FakeProvider, FakeResponse};
use nerdbot::storage;
use nerdbot::tools::registry::ToolRegistry;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;

async fn setup_test_db() -> sqlx::SqlitePool {
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("failed to create in-memory DB");
    sqlx::migrate!("./migrations")
        .run(&pool)
        .await
        .expect("failed to run migrations");
    pool
}

fn make_compaction_service(pool: sqlx::SqlitePool) -> Arc<CompactionService> {
    let llm = Arc::new(FakeProvider::new(vec![FakeResponse::final_text(
        "# Summary",
    )]));
    Arc::new(CompactionService::new(
        pool,
        Arc::new(CompactionWorker::new(llm, "fake-model".into(), 0.0)),
        ContextBudget::default(),
        30,
    ))
}

async fn start_diagnostics_server(
    socket_path: std::path::PathBuf,
    pool: sqlx::SqlitePool,
    compaction_service: Arc<CompactionService>,
    config: AppConfig,
    tools: Arc<ToolRegistry>,
) -> Result<DiagnosticsServer, nerdbot::error::AgentError> {
    let personality = Personality::from_config(&config);
    DiagnosticsServer::start(
        socket_path,
        pool,
        compaction_service,
        config,
        personality,
        tools,
    )
    .await
}

async fn send_request(socket_path: &Path, request: DiagnosticsRequest) -> DiagnosticsResponse {
    let mut stream = UnixStream::connect(socket_path)
        .await
        .expect("failed to connect to diagnostics socket");
    let request = serde_json::to_string(&request).unwrap();
    stream.write_all(request.as_bytes()).await.unwrap();
    stream.write_all(b"\n").await.unwrap();

    let mut response = String::new();
    BufReader::new(stream)
        .read_line(&mut response)
        .await
        .unwrap();
    serde_json::from_str(response.trim_end()).unwrap()
}

#[tokio::test]
async fn test_diagnostics_server_ping_permissions_and_cleanup() {
    let temp = tempfile::tempdir().unwrap();
    let socket_path = temp.path().join("diagnostics.sock");
    let pool = setup_test_db().await;
    let mut server = start_diagnostics_server(
        socket_path.clone(),
        pool.clone(),
        make_compaction_service(pool),
        AppConfig::default(),
        Arc::new(ToolRegistry::new()),
    )
    .await
    .unwrap();

    let mode = std::fs::metadata(&socket_path)
        .unwrap()
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(mode, 0o600);
    assert_eq!(
        send_request(&socket_path, DiagnosticsRequest::Ping).await,
        DiagnosticsResponse::Pong
    );

    server.stop().await;
    assert!(!socket_path.exists());
}

#[tokio::test]
async fn test_diagnostics_server_lists_and_shows_session_by_chat_id() {
    let temp = tempfile::tempdir().unwrap();
    let socket_path = temp.path().join("diagnostics.sock");
    let pool = setup_test_db().await;
    let session = storage::sessions::create_session(&pool, 42).await.unwrap();
    storage::messages::create_message(
        &pool,
        &session.id,
        &genai::chat::ChatMessage::user(genai::chat::MessageContent::from_text("hello")),
        Some(7),
    )
    .await
    .unwrap();
    let mut server = start_diagnostics_server(
        socket_path.clone(),
        pool.clone(),
        make_compaction_service(pool),
        AppConfig::default(),
        Arc::new(ToolRegistry::new()),
    )
    .await
    .unwrap();

    let listed = send_request(&socket_path, DiagnosticsRequest::ListSessions).await;
    match listed {
        DiagnosticsResponse::Sessions { sessions } => {
            assert_eq!(sessions.len(), 1);
            assert_eq!(sessions[0].id, session.id);
            assert_eq!(sessions[0].telegram_chat_id, 42);
        }
        other => panic!("unexpected response: {other:?}"),
    }

    let shown = send_request(
        &socket_path,
        DiagnosticsRequest::ShowSession {
            session_id: None,
            chat_id: Some(42),
            include_prompts: false,
        },
    )
    .await;
    match shown {
        DiagnosticsResponse::Session {
            session: shown_session,
            context,
            compaction_state,
            effective_personality,
            summary_prompt,
            tool_spec,
        } => {
            assert_eq!(shown_session.id, session.id);
            assert_eq!(context.estimated_uncompacted_tokens, 7);
            assert_eq!(context.raw_message_count, 1);
            assert!(matches!(
                compaction_state,
                nerdbot::context::compaction_service::CompactionState::Idle
            ));
            assert!(effective_personality.char_count > 0);
            assert_eq!(effective_personality.timezone, "UTC");
            assert!(effective_personality.body.is_none());
            assert!(summary_prompt.char_count > 0);
            assert!(summary_prompt.body.is_none());
            assert_eq!(tool_spec.count, 0);
            assert!(tool_spec.token_estimate > 0);
            assert!(tool_spec.specs.is_none());
        }
        other => panic!("unexpected response: {other:?}"),
    }

    server.stop().await;
}

#[tokio::test]
async fn test_diagnostics_server_replaces_stale_socket() {
    let temp = tempfile::tempdir().unwrap();
    let socket_path = temp.path().join("diagnostics.sock");
    let listener = tokio::net::UnixListener::bind(&socket_path).unwrap();
    drop(listener);

    let pool = setup_test_db().await;
    let mut server = start_diagnostics_server(
        socket_path.clone(),
        pool.clone(),
        make_compaction_service(pool),
        AppConfig::default(),
        Arc::new(ToolRegistry::new()),
    )
    .await
    .unwrap();
    assert_eq!(
        send_request(&socket_path, DiagnosticsRequest::Ping).await,
        DiagnosticsResponse::Pong
    );
    server.stop().await;
}

#[tokio::test]
async fn test_diagnostics_server_refuses_active_socket() {
    let temp = tempfile::tempdir().unwrap();
    let socket_path = temp.path().join("diagnostics.sock");
    let pool = setup_test_db().await;
    let service = make_compaction_service(pool.clone());
    let mut server = start_diagnostics_server(
        socket_path.clone(),
        pool.clone(),
        service.clone(),
        AppConfig::default(),
        Arc::new(ToolRegistry::new()),
    )
    .await
    .unwrap();

    let err = start_diagnostics_server(
        socket_path.clone(),
        pool,
        service,
        AppConfig::default(),
        Arc::new(ToolRegistry::new()),
    )
    .await
    .err()
    .expect("second server should fail");
    assert!(err.to_string().contains("already in use"));

    server.stop().await;
}

#[tokio::test]
async fn test_diagnostics_server_refuses_to_replace_regular_file() {
    let temp = tempfile::tempdir().unwrap();
    let socket_path = temp.path().join("diagnostics.sock");
    std::fs::write(&socket_path, "keep me").unwrap();
    let pool = setup_test_db().await;

    let err = start_diagnostics_server(
        socket_path.clone(),
        pool.clone(),
        make_compaction_service(pool),
        AppConfig::default(),
        Arc::new(ToolRegistry::new()),
    )
    .await
    .err()
    .expect("regular file path should fail");
    assert!(err.to_string().contains("Refusing to replace non-socket"));
    assert_eq!(std::fs::read_to_string(socket_path).unwrap(), "keep me");
}

#[tokio::test]
async fn test_diagnostics_server_rejects_ambiguous_session_selector() {
    let temp = tempfile::tempdir().unwrap();
    let socket_path = temp.path().join("diagnostics.sock");
    let pool = setup_test_db().await;
    let mut server = start_diagnostics_server(
        socket_path.clone(),
        pool.clone(),
        make_compaction_service(pool),
        AppConfig::default(),
        Arc::new(ToolRegistry::new()),
    )
    .await
    .unwrap();

    let response = send_request(
        &socket_path,
        DiagnosticsRequest::ShowSession {
            session_id: Some("session-id".to_string()),
            chat_id: Some(42),
            include_prompts: false,
        },
    )
    .await;
    match response {
        DiagnosticsResponse::Error { message } => {
            assert!(message.contains("exactly one of session_id or chat_id"));
        }
        other => panic!("unexpected response: {other:?}"),
    }

    server.stop().await;
}

#[tokio::test]
async fn test_diagnostics_client_sends_requests_and_propagates_server_errors() {
    let temp = tempfile::tempdir().unwrap();
    let socket_path = temp.path().join("diagnostics.sock");
    let pool = setup_test_db().await;
    let mut server = start_diagnostics_server(
        socket_path.clone(),
        pool.clone(),
        make_compaction_service(pool),
        AppConfig::default(),
        Arc::new(ToolRegistry::new()),
    )
    .await
    .unwrap();

    let response =
        nerdbot::diagnostics::client::send_request(&socket_path, &DiagnosticsRequest::Ping)
            .await
            .unwrap();
    assert_eq!(response, DiagnosticsResponse::Pong);

    let err = nerdbot::diagnostics::client::send_request(
        &socket_path,
        &DiagnosticsRequest::ShowSession {
            session_id: None,
            chat_id: None,
            include_prompts: false,
        },
    )
    .await
    .unwrap_err();
    assert!(
        err.to_string()
            .contains("exactly one of session_id or chat_id")
    );

    server.stop().await;
}

#[test]
fn test_diagnostics_client_renders_human_and_json_output() {
    let response = DiagnosticsResponse::Session {
        session: nerdbot::diagnostics::protocol::SessionDiagnostics {
            id: "session-1".to_string(),
            telegram_chat_id: 42,
            created_at: chrono::DateTime::from_timestamp(1_700_000_000, 0).unwrap(),
            updated_at: chrono::DateTime::from_timestamp(1_700_000_010, 0).unwrap(),
        },
        context: Box::new(nerdbot::context::diagnostics::ContextDiagnosticsSnapshot {
            session_id: "session-1".to_string(),
            raw_message_count: 3,
            preserved_raw_message_count: 2,
            estimated_uncompacted_tokens: 250,
            usable_input_budget: 1_000,
            soft_threshold_tokens: 600,
            hard_threshold_tokens: 850,
            remaining_before_compaction_tokens: 350,
            remaining_before_hard_bound_tokens: 600,
            pressure: 0.25,
            values_are_estimated: true,
            latest_summary: None,
        }),
        compaction_state: nerdbot::context::compaction_service::CompactionState::Idle,
        effective_personality: Box::new(nerdbot::diagnostics::protocol::PersonalityDiagnostics {
            char_count: 100,
            token_estimate: 25,
            timezone: "UTC".to_string(),
            body: None,
        }),
        summary_prompt: Box::new(nerdbot::diagnostics::protocol::SummaryPromptDiagnostics {
            char_count: 150,
            token_estimate: 37,
            body: None,
        }),
        tool_spec: Box::new(nerdbot::diagnostics::protocol::ToolSpecDiagnostics {
            count: 3,
            token_estimate: 75,
            specs: None,
        }),
    };

    let human = render_human(&response);
    assert!(human.contains("Session: session-1"));
    assert!(human.contains("Telegram chat: 42"));
    assert!(human.contains("Remaining before run:     350"));
    assert!(human.contains("Pressure:                 25.0%"));
    assert!(human.contains("Personality prompt:       25 tokens (100 chars, timezone=UTC)"));
    assert!(human.contains("Summary prompt:           37 tokens (150 chars)"));
    assert!(human.contains("Registered tools:         3 tools, 75 tokens"));

    let json = render_json(&response).unwrap();
    assert!(json.contains("\"type\": \"session\""));
    assert!(json.contains("\"estimated_uncompacted_tokens\": 250"));
}

#[tokio::test]
async fn test_diagnostics_cli_ping_json() {
    let temp = tempfile::tempdir().unwrap();
    let socket_path = temp.path().join("diagnostics.sock");
    let pool = setup_test_db().await;
    let mut server = start_diagnostics_server(
        socket_path.clone(),
        pool.clone(),
        make_compaction_service(pool),
        AppConfig::default(),
        Arc::new(ToolRegistry::new()),
    )
    .await
    .unwrap();

    let binary = env!("CARGO_BIN_EXE_nerdbot");
    let output = tokio::task::spawn_blocking(move || {
        Command::new(binary)
            .arg("--diagnostics-socket")
            .arg(socket_path)
            .arg("diagnostics")
            .arg("ping")
            .arg("--json")
            .output()
            .unwrap()
    })
    .await
    .unwrap();

    assert!(output.status.success());
    assert_eq!(
        String::from_utf8(output.stdout).unwrap().trim(),
        r#"{
  "type": "pong"
}"#
    );

    server.stop().await;
}

#[tokio::test]
async fn test_diagnostics_server_exposes_personality_summary_and_tools_when_requested() {
    let temp = tempfile::tempdir().unwrap();
    let socket_path = temp.path().join("diagnostics.sock");
    let pool = setup_test_db().await;
    let session = storage::sessions::create_session(&pool, 100).await.unwrap();

    let mut config = AppConfig::default();
    config.agent.default_timezone = "Europe/London".to_string();
    let personality_file = temp.path().join("personality.md");
    std::fs::write(&personality_file, "You are a friendly diagnostics helper.").unwrap();
    config.agent.personality_file = personality_file;

    let mut registry = ToolRegistry::new();
    registry.register(nerdbot::tools::echo::EchoTool);
    let registry = Arc::new(registry);

    let mut server = start_diagnostics_server(
        socket_path.clone(),
        pool.clone(),
        make_compaction_service(pool),
        config,
        registry,
    )
    .await
    .unwrap();

    let response = send_request(
        &socket_path,
        DiagnosticsRequest::ShowSession {
            session_id: Some(session.id.clone()),
            chat_id: None,
            include_prompts: true,
        },
    )
    .await;

    match response {
        DiagnosticsResponse::Session {
            effective_personality,
            summary_prompt,
            tool_spec,
            ..
        } => {
            let body = effective_personality.body.unwrap();
            assert!(body.contains("friendly diagnostics helper"));
            assert!(body.contains("Europe/London"));
            assert_eq!(effective_personality.timezone, "Europe/London");
            assert_eq!(effective_personality.char_count, body.len());
            assert_eq!(
                effective_personality.token_estimate,
                (body.len() / 4).max(1)
            );

            let summary_body = summary_prompt.body.unwrap();
            assert!(summary_body.contains("You are a conversation summarizer"));
            assert_eq!(summary_prompt.char_count, summary_body.len());

            assert_eq!(tool_spec.count, 1);
            let specs = tool_spec.specs.unwrap();
            assert_eq!(specs.len(), 1);
            assert_eq!(specs[0]["name"], "echo");
        }
        other => panic!("unexpected response: {other:?}"),
    }

    server.stop().await;
}

#[tokio::test]
async fn test_diagnostics_prompt_redaction_by_default() {
    let temp = tempfile::tempdir().unwrap();
    let socket_path = temp.path().join("diagnostics.sock");
    let pool = setup_test_db().await;
    let session = storage::sessions::create_session(&pool, 100).await.unwrap();

    let mut config = AppConfig::default();
    config.agent.default_timezone = "UTC".to_string();

    let mut server = start_diagnostics_server(
        socket_path.clone(),
        pool.clone(),
        make_compaction_service(pool),
        config,
        Arc::new(ToolRegistry::new()),
    )
    .await
    .unwrap();

    let response = send_request(
        &socket_path,
        DiagnosticsRequest::ShowSession {
            session_id: Some(session.id.clone()),
            chat_id: None,
            include_prompts: false,
        },
    )
    .await;

    match response {
        DiagnosticsResponse::Session {
            effective_personality,
            summary_prompt,
            tool_spec,
            ..
        } => {
            assert!(effective_personality.body.is_none());
            assert!(summary_prompt.body.is_none());
            assert!(tool_spec.specs.is_none());
        }
        other => panic!("unexpected response: {other:?}"),
    }

    server.stop().await;
}

#[test]
fn test_diagnostics_protocol_serialization() {
    let request = DiagnosticsRequest::ShowSession {
        session_id: Some("session-uuid".to_string()),
        chat_id: None,
        include_prompts: true,
    };
    let request_json = serde_json::to_string(&request).unwrap();
    let request_parsed: DiagnosticsRequest = serde_json::from_str(&request_json).unwrap();
    assert_eq!(request, request_parsed);

    let response = DiagnosticsResponse::Pong;
    let response_json = serde_json::to_string(&response).unwrap();
    let response_parsed: DiagnosticsResponse = serde_json::from_str(&response_json).unwrap();
    assert_eq!(response, response_parsed);
}

#[tokio::test]
async fn test_diagnostics_docker_bind_mount_socket_lifecycle() {
    let temp = tempfile::tempdir().unwrap();
    let mount_dir = temp.path().join("run");
    std::fs::create_dir(&mount_dir).unwrap();
    let socket_path = mount_dir.join("nerdbot.sock");

    let pool = setup_test_db().await;

    let mut server = start_diagnostics_server(
        socket_path.clone(),
        pool.clone(),
        make_compaction_service(pool),
        AppConfig::default(),
        Arc::new(ToolRegistry::new()),
    )
    .await
    .unwrap();

    assert!(socket_path.exists());

    let metadata = std::fs::metadata(&socket_path).unwrap();
    let mode = metadata.permissions().mode() & 0o777;
    assert_eq!(mode, 0o600);

    server.stop().await;

    assert!(!socket_path.exists());
    assert!(mount_dir.exists());
}
