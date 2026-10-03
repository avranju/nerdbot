//! Regression coverage for ordered persistence and provider-safe history replay.

use std::collections::HashSet;
use std::sync::Arc;

use genai::chat::{
    ChatMessage, ChatOptions, ChatRequest, ChatResponse, ChatRole, ContentPart, MessageContent,
    ToolCall, ToolResponse,
};
use nerdbot::agent::personality::Personality;
use nerdbot::channel::handler::{ChannelMessageHandler, ChannelMessageHandlerInput};
use nerdbot::channel::{ChannelRegistry, ConversationAddress, InboundMessage, SenderIdentity};
use nerdbot::config::AppConfig;
use nerdbot::context::budget::ContextBudget;
use nerdbot::context::compaction_service::CompactionService;
use nerdbot::context::compaction_worker::CompactionWorker;
use nerdbot::context::history::{compaction_prefix_len, normalize_history};
use nerdbot::context::manager::ContextManager;
use nerdbot::error::AgentError;
use nerdbot::llm::LlmExecutor;
use nerdbot::llm::fake::{FakeProvider, FakeResponse};
use nerdbot::storage::{self, messages};
use nerdbot::tools::echo::EchoTool;
use nerdbot::tools::registry::ToolRegistry;
use serde_json::json;
use sqlx::SqlitePool;

async fn pool() -> SqlitePool {
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(
            sqlx::sqlite::SqliteConnectOptions::new()
                .in_memory(true)
                .foreign_keys(true),
        )
        .await
        .unwrap();
    sqlx::migrate!("./migrations").run(&pool).await.unwrap();
    pool
}

fn call(id: &str, name: &str) -> ToolCall {
    ToolCall {
        call_id: id.into(),
        fn_name: name.into(),
        fn_arguments: json!({"text": "echo context"}),
        thought_signatures: None,
    }
}

fn calls(ids: &[&str]) -> ChatMessage {
    ChatMessage::from(ids.iter().map(|id| call(id, "echo")).collect::<Vec<_>>())
}

fn outputs(ids: &[&str]) -> ChatMessage {
    ChatMessage::from(
        ids.iter()
            .map(|id| ToolResponse::new(*id, format!("useful output {id}")))
            .collect::<Vec<_>>(),
    )
}

/// Independent strict-protocol check, also used on the exact fake ChatRequest.
fn assert_protocol(messages: &[ChatMessage]) {
    let mut pending = HashSet::new();
    let mut seen = HashSet::new();
    for message in messages {
        let calls = message.content.tool_calls();
        if !calls.is_empty() {
            assert_eq!(message.role, ChatRole::Assistant);
            assert!(pending.is_empty());
            for call in calls {
                assert!(!call.call_id.is_empty());
                assert!(seen.insert(call.call_id.clone()));
                assert!(pending.insert(call.call_id.clone()));
            }
        } else if message.role != ChatRole::Tool {
            assert!(
                pending.is_empty(),
                "ordinary message interrupted pending tool calls"
            );
        }
        for part in message.content.parts() {
            if let ContentPart::ToolResponse(response) = part {
                assert_eq!(message.role, ChatRole::Tool);
                assert!(
                    pending.remove(&response.call_id),
                    "output without preceding call: {}",
                    response.call_id
                );
            }
        }
    }
    assert!(pending.is_empty(), "unanswered calls");
}

fn text(messages: &[ChatMessage]) -> String {
    messages
        .iter()
        .filter_map(|m| m.content.joined_texts())
        .collect::<Vec<_>>()
        .join("\n")
}

async fn history(pool: &SqlitePool, session: &str) -> Vec<ChatMessage> {
    messages::list_messages(pool, session, None)
        .await
        .unwrap()
        .into_iter()
        .rev()
        .map(|m| m.to_message().unwrap())
        .collect()
}

fn handler(
    pool: SqlitePool,
    provider: Arc<dyn LlmExecutor>,
    iterations: u32,
) -> ChannelMessageHandler {
    let mut config = AppConfig::default();
    config.agent.max_tool_iterations = iterations;
    handler_with_config(pool, provider, config)
}

fn handler_with_config(
    pool: SqlitePool,
    provider: Arc<dyn LlmExecutor>,
    config: AppConfig,
) -> ChannelMessageHandler {
    let mut registry = ToolRegistry::new();
    registry.register(EchoTool);
    let worker = CompactionWorker::new(provider.clone(), "fake".into(), 0.0);
    let service = Arc::new(CompactionService::new(
        pool.clone(),
        Arc::new(worker),
        ContextBudget::default(),
        30,
    ));
    ChannelMessageHandler::new(ChannelMessageHandlerInput {
        pool,
        llm: provider,
        registry: Arc::new(registry),
        channel_registry: Arc::new(ChannelRegistry::new(vec![])),
        personality: Personality::from_config(&config),
        config,
        scheduler_notifier: None,
        compaction_service: service,
    })
}

async fn turn(handler: &ChannelMessageHandler, prompt: &str) -> Result<Option<String>, AgentError> {
    handler
        .handle_rich_message(
            &ConversationAddress::telegram_chat(1),
            &SenderIdentity::new("1", None),
            &InboundMessage {
                text: prompt.into(),
                attachments: vec![],
                attachment_parts: vec![],
            },
        )
        .await
}

async fn session(pool: &SqlitePool) -> String {
    storage::sessions::get_session_for_chat(pool, 1)
        .await
        .unwrap()
        .unwrap()
        .id
}

/// Inject replay metadata that FakeResponse intentionally does not model.
struct WithMetadata(Arc<FakeProvider>);
#[async_trait::async_trait]
impl LlmExecutor for WithMetadata {
    async fn complete(
        &self,
        model: &str,
        request: ChatRequest,
        options: ChatOptions,
    ) -> Result<ChatResponse, AgentError> {
        let mut response = self.0.complete(model, request, options).await?;
        if !response.tool_calls().is_empty() {
            response
                .content
                .push(ContentPart::ThoughtSignature("provider signature".into()));
            response
                .content
                .push(ContentPart::Custom(genai::chat::CustomPart {
                    model_iden: None,
                    data: json!({"provider": "replay metadata"}),
                }));
            response.reasoning_content = Some("provider reasoning".into());
        }
        Ok(response)
    }
}

#[tokio::test]
async fn interactive_history_replays_complete_batches_in_order_on_next_turn() {
    let pool = pool().await;
    let mut first = FakeResponse::tool_calls(vec![call("a", "echo"), call("b", "missing_tool")]);
    first.assistant_text = Some("I will check both.".into());
    let provider = Arc::new(FakeProvider::new(vec![
        first,
        FakeResponse::tool_calls(vec![call("c", "echo")]),
        FakeResponse::final_text("done"),
        FakeResponse::final_text("next reply"),
    ]));
    let handler = handler(pool.clone(), Arc::new(WithMetadata(provider.clone())), 5);
    assert_eq!(
        turn(&handler, "first prompt").await.unwrap().as_deref(),
        Some("done")
    );
    let session = session(&pool).await;
    let stored = history(&pool, &session).await;
    assert_eq!(
        stored.iter().map(|m| m.role.clone()).collect::<Vec<_>>(),
        vec![
            ChatRole::User,
            ChatRole::Assistant,
            ChatRole::Tool,
            ChatRole::Assistant,
            ChatRole::Tool,
            ChatRole::Assistant
        ]
    );
    assert_eq!(
        stored[0].content.joined_texts().as_deref(),
        Some("first prompt")
    );
    assert!(text(&stored).contains("I will check both."));
    assert_protocol(&stored);
    assert!(stored[2].content.parts().iter().any(|part| matches!(part, ContentPart::ToolResponse(r) if r.content.contains("\"success\":false"))));
    assert!(
        stored[1]
            .content
            .parts()
            .iter()
            .any(|p| matches!(p, ContentPart::ThoughtSignature(s) if s == "provider signature"))
    );
    assert!(
        stored[1]
            .content
            .parts()
            .iter()
            .any(|p| matches!(p, ContentPart::ReasoningContent(s) if s == "provider reasoning"))
    );
    assert!(
        stored[1]
            .content
            .parts()
            .iter()
            .any(|p| matches!(p, ContentPart::Custom(_)))
    );
    turn(&handler, "second prompt").await.unwrap();
    let request = provider.last_request().unwrap();
    assert_protocol(&request.messages);
    assert_eq!(text(&request.messages).matches("first prompt").count(), 1);
    assert_eq!(text(&request.messages).matches("second prompt").count(), 1);
    assert_eq!(
        request
            .messages
            .iter()
            .flat_map(|m| m.content.tool_calls())
            .count(),
        3
    );
}

#[test]
fn malformed_batches_become_useful_text_and_valid_batches_survive() {
    let cases = vec![
        vec![outputs(&["orphan"])],
        vec![calls(&["missing"])],
        vec![calls(&["a", "b"]), outputs(&["a"])],
        vec![calls(&["a"]), outputs(&["a", "a"])],
        vec![calls(&["a", "a"]), outputs(&["a"])],
        vec![calls(&["a"]), outputs(&["unmatched"])],
        vec![calls(&[""]), outputs(&[""])],
        vec![outputs(&["a"]), calls(&["a"])],
        vec![
            calls(&["a"]),
            ChatMessage::user("interrupt"),
            outputs(&["a"]),
        ],
        vec![ChatMessage::user(MessageContent::from_parts(vec![
            ContentPart::Text("ordinary mixed text".into()),
            ContentPart::ToolCall(call("a", "echo")),
        ]))],
        vec![
            calls(&["a"]),
            ChatMessage::tool(MessageContent::from_parts(vec![
                ContentPart::Text("mixed tool text".into()),
                ContentPart::ToolResponse(ToolResponse::new("a", "tool value")),
            ])),
        ],
    ];
    for mut input in cases {
        input.insert(0, ChatMessage::user("ordinary question"));
        input.push(ChatMessage::assistant("ordinary reply"));
        let safe = normalize_history(&input);
        assert_protocol(&safe);
        assert!(text(&safe).contains("ordinary question"));
        assert!(text(&safe).contains("ordinary reply"));
        assert!(text(&safe).contains("Historical tool"));
        assert_eq!(safe.iter().flat_map(|m| m.content.tool_calls()).count(), 0);
        // Normalization is stable when called at more than one replay boundary.
        assert_eq!(
            serde_json::to_value(&safe).unwrap(),
            serde_json::to_value(normalize_history(&safe)).unwrap()
        );
    }
    let valid = vec![
        calls(&["a", "b"]),
        outputs(&["b"]),
        outputs(&["a"]),
        ChatMessage::assistant("done"),
    ];
    assert_protocol(&normalize_history(&valid));
    assert_eq!(
        serde_json::to_value(&valid).unwrap(),
        serde_json::to_value(normalize_history(&valid)).unwrap()
    );
    let repeated = vec![
        calls(&["a"]),
        outputs(&["a"]),
        calls(&["a"]),
        outputs(&["a"]),
    ];
    let safe = normalize_history(&repeated);
    assert_protocol(&safe);
    assert_eq!(safe.iter().flat_map(|m| m.content.tool_calls()).count(), 2);
    assert_eq!(
        serde_json::to_value(&safe).unwrap(),
        serde_json::to_value(normalize_history(&safe)).unwrap()
    );
    assert_eq!(safe.iter().flat_map(|message| message.content.parts()).filter(|part| {
        matches!(part, ContentPart::ToolResponse(response) if response.content == "useful output a")
    }).count(), 2);
}

#[tokio::test]
async fn legacy_and_current_orphans_are_safe_in_actual_request_without_db_changes() {
    let pool = pool().await;
    let session = storage::sessions::create_session(&pool, 1).await.unwrap();
    let old = messages::create_message(&pool, &session.id, &ChatMessage::tool("placeholder"), None)
        .await
        .unwrap();
    sqlx::query("UPDATE messages SET content = '', structured_content_json = ? WHERE id = ?")
        .bind(json!({"Parts": [{"Text": "legacy mixed text"}, {"ToolResult": {"tool_call_id": "ygUmV7Hn2t8xNCxU6BRxwQtFQ1vr1Uim", "status": "success", "content": "legacy useful output"}}]}).to_string())
        .bind(&old.id).execute(&pool).await.unwrap();
    messages::create_message(&pool, &session.id, &outputs(&["current orphan"]), None)
        .await
        .unwrap();
    messages::create_message(
        &pool,
        &session.id,
        &ChatMessage::user("old ordinary text"),
        None,
    )
    .await
    .unwrap();
    let provider = Arc::new(FakeProvider::new(vec![FakeResponse::final_text("reply")]));
    turn(&handler(pool.clone(), provider.clone(), 3), "new prompt")
        .await
        .unwrap();
    let request = provider.last_request().unwrap();
    assert_protocol(&request.messages);
    let input = text(&request.messages);
    for expected in [
        "legacy mixed text",
        "legacy useful output",
        "useful output current orphan",
        "old ordinary text",
        "Historical tool output",
    ] {
        assert!(input.contains(expected), "missing {expected}");
    }
    assert_eq!(history(&pool, &session.id).await[0].role, ChatRole::Tool);
}

fn budget(tokens: usize) -> ContextBudget {
    ContextBudget {
        context_window_tokens: tokens,
        reserved_output_tokens: 0,
        reserved_tool_loop_tokens: 0,
        ..ContextBudget::default()
    }
}

#[tokio::test]
async fn bounding_skips_whole_oversized_batches_and_reserves_current_and_system_costs() {
    let pool = pool().await;
    let session = storage::sessions::create_session(&pool, 1).await.unwrap();
    messages::create_message(&pool, &session.id, &ChatMessage::user("old text"), None)
        .await
        .unwrap();
    messages::create_tool_exchange(
        &pool,
        &session.id,
        &calls(&["a", "b"]),
        &ChatMessage::from(vec![
            ToolResponse::new("a", "x".repeat(4000)),
            ToolResponse::new("b", "short"),
        ]),
    )
    .await
    .unwrap();
    messages::create_message(
        &pool,
        &session.id,
        &ChatMessage::assistant("recent reply"),
        None,
    )
    .await
    .unwrap();
    for preserve in [0, 1, 2, 3, 30] {
        let manager = ContextManager::new_with_preserve(pool.clone(), budget(100), preserve);
        let retained = manager
            .assemble_messages(
                &session.id,
                "system text",
                ChatMessage::user("current"),
                "UTC",
            )
            .await
            .unwrap();
        assert_protocol(&retained);
        assert!(manager.estimate_tokens(&retained) <= 100);
        assert!(text(&retained).contains("recent reply"));
        assert_eq!(
            retained.iter().flat_map(|m| m.content.tool_calls()).count(),
            0
        );
        assert!(!text(&retained).contains("short"));
        let manager = ContextManager::new_with_preserve(pool.clone(), budget(2000), preserve);
        let retained = manager
            .assemble_messages(&session.id, "", ChatMessage::user("current"), "UTC")
            .await
            .unwrap();
        assert_protocol(&retained);
        assert_eq!(
            retained.iter().flat_map(|m| m.content.tool_calls()).count(),
            2
        );
        assert!(manager.estimate_tokens(&retained) <= 2000);
    }
    let manager = ContextManager::new(pool, budget(1));
    assert!(matches!(
        manager
            .assemble_messages(&session.id, "", ChatMessage::user("too big"), "UTC")
            .await,
        Err(AgentError::Context(_))
    ));
}

#[tokio::test]
async fn compaction_rounds_preserve_window_and_replays_old_split_boundary_safely() {
    let pool = pool().await;
    let session = storage::sessions::create_session(&pool, 1).await.unwrap();
    let old =
        messages::create_message(&pool, &session.id, &ChatMessage::user("old question"), None)
            .await
            .unwrap();
    let assistant = messages::create_message(&pool, &session.id, &calls(&["a", "b"]), None)
        .await
        .unwrap();
    messages::create_message(&pool, &session.id, &outputs(&["a"]), None)
        .await
        .unwrap();
    let last_output = messages::create_message(&pool, &session.id, &outputs(&["b"]), None)
        .await
        .unwrap();
    let all = history(&pool, &session.id).await;
    assert_eq!(compaction_prefix_len(&all, 1), 1);
    let provider = Arc::new(FakeProvider::new(vec![FakeResponse::final_text("summary")]));
    let worker = CompactionWorker::new_with_preserve(provider.clone(), "fake".into(), 0.0, 1);
    worker
        .compact(&pool, &session.id, &budget(2000))
        .await
        .unwrap();
    let summary = storage::summaries::get_latest_summary(&pool, &session.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(summary.covers_through_message_id, old.id);
    assert!(!text(&provider.last_request().unwrap().messages).contains("useful output a"));
    let manager = ContextManager::new(pool.clone(), budget(2000));
    let input = manager
        .assemble_messages(&session.id, "", ChatMessage::user("next"), "UTC")
        .await
        .unwrap();
    assert_protocol(&input);
    assert_eq!(input.iter().flat_map(|m| m.content.tool_calls()).count(), 2);
    let worker = CompactionWorker::new_with_preserve(provider.clone(), "fake".into(), 0.0, 0);
    worker
        .compact(&pool, &session.id, &budget(2000))
        .await
        .unwrap();
    assert_eq!(
        storage::summaries::get_latest_summary(&pool, &session.id)
            .await
            .unwrap()
            .unwrap()
            .covers_through_message_id,
        last_output.id
    );
    assert!(text(&provider.last_request().unwrap().messages).contains("useful output a"));
    // Simulate a pre-fix summary ending at the assistant call row.
    sqlx::query("DELETE FROM context_summaries WHERE chat_session_id = ?")
        .bind(&session.id)
        .execute(&pool)
        .await
        .unwrap();
    storage::summaries::create_summary(
        &pool,
        &nerdbot::context::summaries::ContextSummary::new(
            session.id.clone(),
            "old split summary".into(),
            assistant.id,
        ),
    )
    .await
    .unwrap();
    let input = manager
        .assemble_messages(&session.id, "", ChatMessage::user("next"), "UTC")
        .await
        .unwrap();
    assert_protocol(&input);
    assert!(text(&input).contains("Historical tool output"));
    assert!(text(&input).contains("useful output b"));
}

#[tokio::test]
async fn silent_provider_error_and_iteration_limit_leave_one_prompt_and_safe_history() {
    for (responses, limit, expected_rows) in [
        (
            vec![
                FakeResponse::tool_calls(vec![call("a", "echo")]),
                FakeResponse::final_text(""),
            ],
            3,
            4,
        ),
        (vec![FakeResponse::error("failure")], 3, 2),
        (
            vec![
                FakeResponse::tool_calls(vec![call("a", "echo")]),
                FakeResponse::error("failure"),
            ],
            3,
            4,
        ),
        (
            vec![FakeResponse::tool_calls(vec![call("a", "echo")])],
            1,
            4,
        ),
    ] {
        let pool = pool().await;
        let provider = Arc::new(FakeProvider::new(responses));
        turn(&handler(pool.clone(), provider, limit), "one prompt")
            .await
            .unwrap();
        let all = history(&pool, &session(&pool).await).await;
        assert_eq!(all.len(), expected_rows);
        assert_eq!(text(&all).matches("one prompt").count(), 1);
        assert_protocol(&all);
    }
}

#[tokio::test]
async fn storage_failures_do_not_write_partial_exchanges_or_duplicate_prompts() {
    let pool = pool().await;
    let provider = Arc::new(FakeProvider::new(vec![
        FakeResponse::tool_calls(vec![call("a", "echo")]),
        FakeResponse::final_text("done"),
    ]));
    let handler = handler(pool.clone(), provider.clone(), 3);
    // Fail second exchange insert. The assistant call must roll back as well.
    sqlx::query("CREATE TRIGGER reject_tool BEFORE INSERT ON messages WHEN NEW.role = '\"tool\"' BEGIN SELECT RAISE(ABORT, 'test tool persistence failure'); END").execute(&pool).await.unwrap();
    let reply = turn(&handler, "one prompt").await.unwrap().unwrap();
    assert!(reply.contains("test tool persistence failure"));
    assert_eq!(provider.call_count(), 1);
    let all = history(&pool, &session(&pool).await).await;
    assert_eq!(all.len(), 2);
    assert_protocol(&all);
    sqlx::query("DROP TRIGGER reject_tool")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("CREATE TRIGGER reject_user BEFORE INSERT ON messages WHEN NEW.role = '\"user\"' BEGIN SELECT RAISE(ABORT, 'test user persistence failure'); END").execute(&pool).await.unwrap();
    assert!(matches!(
        turn(&handler, "failed prompt").await,
        Err(AgentError::Storage(_))
    ));
    assert_eq!(provider.call_count(), 1);
    assert_eq!(history(&pool, &session(&pool).await).await.len(), 2);
    sqlx::query("DROP TRIGGER reject_user")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("CREATE TRIGGER reject_assistant BEFORE INSERT ON messages WHEN NEW.role = '\"assistant\"' BEGIN SELECT RAISE(ABORT, 'test final persistence failure'); END").execute(&pool).await.unwrap();
    assert!(matches!(
        turn(&handler, "final failed prompt").await,
        Err(AgentError::Storage(_))
    ));
    let all = history(&pool, &session(&pool).await).await;
    assert_eq!(all.len(), 3);
    assert_eq!(text(&all).matches("final failed prompt").count(), 1);
    assert_protocol(&all);
}

#[tokio::test]
async fn reset_commands_keep_old_history_and_new_session_is_empty() {
    for reset in ["/reset_context", "/new_topic"] {
        let pool = pool().await;
        let provider = Arc::new(FakeProvider::new(vec![
            FakeResponse::final_text("old reply"),
            FakeResponse::final_text("new reply"),
        ]));
        let handler = handler(pool.clone(), provider.clone(), 3);
        turn(&handler, "old prompt").await.unwrap();
        let old = session(&pool).await;
        turn(&handler, reset).await.unwrap();
        let new = session(&pool).await;
        assert_ne!(old, new);
        assert!(history(&pool, &new).await.is_empty());
        assert_eq!(history(&pool, &old).await.len(), 4);
        turn(&handler, "new prompt").await.unwrap();
        let request = provider.last_request().unwrap();
        assert!(!text(&request.messages).contains("old prompt"));
        assert_eq!(text(&request.messages).matches("new prompt").count(), 1);
    }
}

#[tokio::test]
async fn scheduled_snapshots_normalize_legacy_and_partial_batches_and_persist_order() {
    use nerdbot::scheduler::models::{JobContextPolicy, ScheduleType};
    for snapshot in [
        serde_json::to_string(&vec![ChatMessage::user("snapshot ordinary"), outputs(&["orphan"]), calls(&["missing"])]).unwrap(),
        json!([{"role":"user", "content":{"Text":"snapshot ordinary"}}, {"role":"tool", "content":{"Parts":[{"ToolResult":{"tool_call_id":"legacy", "status":"success", "content":"snapshot useful"}}]}}]).to_string(),
        serde_json::to_string(&vec![ChatMessage::user("snapshot ordinary"), calls(&["complete"]), outputs(&["complete"])]).unwrap(),
    ] {
        let pool = pool().await;
        let job = storage::jobs::create_job_full(&pool, storage::jobs::CreateJobInput {
            owner_address: ConversationAddress::telegram_chat(1), name: "test".into(), prompt: "scheduled prompt".into(),
            schedule_type: ScheduleType::OneShot, cron_expression: None, run_at: None, timezone: None,
            notify_on_completion: false, context_policy: JobContextPolicy::IncludeCreationSnapshot,
            creation_context_snapshot: Some(snapshot), next_run_at: None,
        }).await.unwrap();
        let provider = Arc::new(FakeProvider::new(vec![FakeResponse::tool_calls(vec![call("new", "echo")]), FakeResponse::final_text("done")]));
        let mut registry = ToolRegistry::new(); registry.register(EchoTool);
        nerdbot::scheduler::runner::run_scheduled_job(nerdbot::scheduler::runner::RunScheduledJobInput {
            pool: pool.clone(), llm: provider.clone(), registry: Arc::new(registry), channel_registry: Arc::new(ChannelRegistry::new(vec![])),
            loop_config: nerdbot::agent::agent_loop::AgentLoopConfig::default(), personality: "".into(),
            workspace_root: "/tmp".into(), access_policy: Default::default(), timezone: "UTC".into(),
        }, &job.id).await.unwrap();
        let input = provider.last_request().unwrap();
        assert_protocol(&input.messages);
        assert!(text(&input.messages).contains("snapshot ordinary"));
        assert_eq!(text(&input.messages).matches("scheduled prompt").count(), 1);
        let all = history(&pool, &session(&pool).await).await;
        assert_protocol(&all);
        assert_eq!(all.iter().map(|m| m.role.clone()).collect::<Vec<_>>(), vec![ChatRole::User, ChatRole::Assistant, ChatRole::Tool, ChatRole::Assistant]);
    }
}

#[tokio::test]
async fn rich_current_message_keeps_binary_once_and_persists_only_safe_markers() {
    let pool = pool().await;
    let provider = Arc::new(FakeProvider::new(vec![FakeResponse::final_text("done")]));
    let handler = handler(pool.clone(), provider.clone(), 3);
    let payload = "BASE64_PAYLOAD_SHOULD_NOT_BE_PERSISTED";
    let inbound = InboundMessage {
        text: "analyze attachment".into(),
        attachments: vec![nerdbot::channel::AttachmentInfo {
            display_name: "test.png".into(),
            mime_type: "image/png".into(),
            size_bytes: 100,
            downloaded: true,
            persistence_marker: "[Image: test.png]".into(),
            extracted_text: None,
        }],
        attachment_parts: vec![ContentPart::from_binary_base64(
            "image/png",
            payload,
            Some("test.png".into()),
        )],
    };
    handler
        .handle_rich_message(
            &ConversationAddress::telegram_chat(1),
            &SenderIdentity::new("1", None),
            &inbound,
        )
        .await
        .unwrap();
    let request = provider.last_request().unwrap();
    assert_eq!(
        request
            .messages
            .iter()
            .flat_map(|m| m.content.parts())
            .filter(|p| matches!(p, ContentPart::Binary(_)))
            .count(),
        1
    );
    assert_eq!(
        text(&request.messages)
            .matches("analyze attachment")
            .count(),
        1
    );
    let persisted = history(&pool, &session(&pool).await).await;
    assert!(!serde_json::to_string(&persisted).unwrap().contains(payload));
    assert!(text(&persisted).contains("[Image: test.png]"));
}

#[test]
fn recent_snapshot_cutoff_keeps_whole_exchange() {
    use nerdbot::context::history::recent_history;
    let history = vec![
        ChatMessage::user("older"),
        calls(&["a", "b"]),
        outputs(&["b"]),
        outputs(&["a"]),
    ];
    for count in [1, 2, 3] {
        let snapshot = recent_history(&history, count);
        assert_protocol(&snapshot);
        assert_eq!(snapshot.len(), 3);
    }
    assert!(recent_history(&history, 0).is_empty());
}

#[derive(Default)]
struct RecordingChannel(std::sync::Mutex<Vec<String>>);
#[async_trait::async_trait]
impl nerdbot::channel::ChannelService for RecordingChannel {
    fn channel_id(&self) -> &str {
        "telegram"
    }
    async fn send_message(
        &self,
        _: &ConversationAddress,
        message: nerdbot::channel::OutboundMessage,
    ) -> Result<(), AgentError> {
        self.0.lock().unwrap().push(message.text);
        Ok(())
    }
}

#[tokio::test]
async fn scheduled_notification_dedup_uses_current_run_even_with_old_outputs() {
    use nerdbot::scheduler::models::{JobContextPolicy, ScheduleType};
    for (send_this_run, fail_send) in [(false, false), (true, false), (true, true)] {
        let pool = pool().await;
        let session = storage::sessions::create_session(&pool, 1).await.unwrap();
        messages::create_message(
            &pool,
            &session.id,
            &ChatMessage::from(ToolResponse::new(
                "historical",
                json!({"tool_name": "send_user_message", "sent": true}).to_string(),
            )),
            None,
        )
        .await
        .unwrap();
        let job = storage::jobs::create_job_full(
            &pool,
            storage::jobs::CreateJobInput {
                owner_address: ConversationAddress::telegram_chat(1),
                name: "test".into(),
                prompt: "scheduled prompt".into(),
                schedule_type: ScheduleType::OneShot,
                cron_expression: None,
                run_at: None,
                timezone: None,
                notify_on_completion: true,
                context_policy: JobContextPolicy::Isolated,
                creation_context_snapshot: None,
                next_run_at: None,
            },
        )
        .await
        .unwrap();
        let mut responses = Vec::new();
        if send_this_run {
            let mut call = call("new notification", "send_user_message");
            call.fn_arguments = json!({"text": "tool notification"});
            if fail_send {
                call.fn_arguments["conversation"] =
                    json!({"channel_id":"missing", "conversation_id":"1"});
            }
            responses.push(FakeResponse::tool_calls(vec![call]));
        }
        responses.push(FakeResponse::final_text("done"));
        let provider = Arc::new(FakeProvider::new(responses));
        let channel = Arc::new(RecordingChannel::default());
        let mut registry = ToolRegistry::new();
        registry.register(nerdbot::tools::messaging::SendUserMessage);
        nerdbot::scheduler::runner::run_scheduled_job(
            nerdbot::scheduler::runner::RunScheduledJobInput {
                pool: pool.clone(),
                llm: provider.clone(),
                registry: Arc::new(registry),
                channel_registry: Arc::new(ChannelRegistry::new(vec![channel.clone()])),
                loop_config: nerdbot::agent::agent_loop::AgentLoopConfig::default(),
                personality: "".into(),
                workspace_root: "/tmp".into(),
                access_policy: Default::default(),
                timezone: "UTC".into(),
            },
            &job.id,
        )
        .await
        .unwrap();
        let sent = channel.0.lock().unwrap().clone();
        assert_eq!(sent.len(), 1);
        if send_this_run && !fail_send {
            assert_eq!(sent[0], "tool notification");
        } else {
            assert!(sent[0].contains("executed successfully"));
        }
        assert_protocol(&provider.last_request().unwrap().messages);
        assert_protocol(&normalize_history(&history(&pool, &session.id).await));
    }
}

#[tokio::test]
async fn compaction_budget_limits_complete_prefix_and_never_covers_omitted_rows() {
    let pool = pool().await;
    let session = storage::sessions::create_session(&pool, 1).await.unwrap();
    let first = messages::create_message(
        &pool,
        &session.id,
        &ChatMessage::user("first useful question"),
        None,
    )
    .await
    .unwrap();
    messages::create_tool_exchange(
        &pool,
        &session.id,
        &calls(&["a", "b"]),
        &ChatMessage::from(vec![
            ToolResponse::new("a", "huge useful output".repeat(1000)),
            ToolResponse::new("b", "other output"),
        ]),
    )
    .await
    .unwrap();
    messages::create_message(
        &pool,
        &session.id,
        &ChatMessage::assistant("omitted newest reply"),
        None,
    )
    .await
    .unwrap();
    let provider = Arc::new(FakeProvider::new(vec![FakeResponse::final_text(
        "short summary",
    )]));
    let worker = CompactionWorker::new_with_preserve(provider.clone(), "fake".into(), 0.0, 0);
    let all = messages::list_messages(&pool, &session.id, None)
        .await
        .unwrap();
    let chronological = all.iter().rev().collect::<Vec<_>>();
    let first_prompt = worker.build_compaction_prompt(&None, &chronological[..1]);
    let limit = first_prompt.len() / 4;
    worker
        .compact(&pool, &session.id, &budget(limit))
        .await
        .unwrap();
    let input = provider.last_request().unwrap();
    assert_protocol(&input.messages);
    let manager = ContextManager::new(pool.clone(), budget(limit));
    assert!(manager.estimate_tokens(&input.messages) <= limit);
    assert!(text(&input.messages).contains("first useful question"));
    assert!(!text(&input.messages).contains("huge useful output"));
    assert!(!text(&input.messages).contains("omitted newest reply"));
    assert_eq!(
        storage::summaries::get_latest_summary(&pool, &session.id)
            .await
            .unwrap()
            .unwrap()
            .covers_through_message_id,
        first.id
    );
    // Budget cannot fit even the fixed summary instructions: no API call and
    // no change to the saved boundary.
    assert!(matches!(
        worker.compact(&pool, &session.id, &budget(1)).await,
        Err(AgentError::Compaction(_))
    ));
    assert_eq!(provider.call_count(), 1);
    assert_eq!(
        storage::summaries::get_latest_summary(&pool, &session.id)
            .await
            .unwrap()
            .unwrap()
            .covers_through_message_id,
        first.id
    );
    // A larger budget can summarize the entire next exchange, including useful
    // output text, and advance to the final ordinary message.
    worker
        .compact(&pool, &session.id, &budget(2000))
        .await
        .unwrap();
    let input = provider.last_request().unwrap();
    assert_protocol(&input.messages);
    assert!(text(&input.messages).contains("huge useful output"));
    assert!(
        ContextManager::new(pool.clone(), budget(2000)).estimate_tokens(&input.messages) <= 2000
    );
}

/// Native Gemini signatures precede the function call in response content.
struct WithLeadingSignature {
    provider: Arc<FakeProvider>,
    signature: String,
}

#[async_trait::async_trait]
impl LlmExecutor for WithLeadingSignature {
    async fn complete(
        &self,
        model: &str,
        request: ChatRequest,
        options: ChatOptions,
    ) -> Result<ChatResponse, AgentError> {
        let mut response = self.provider.complete(model, request, options).await?;
        if !response.tool_calls().is_empty() {
            let mut parts = vec![ContentPart::ThoughtSignature(self.signature.clone())];
            parts.extend(response.content.parts().clone());
            response.content = MessageContent::from_parts(parts);
        }
        Ok(response)
    }
}

#[tokio::test]
async fn response_local_gemini_ids_keep_both_signed_exchanges_in_live_and_stored_replay() {
    let pool = pool().await;
    let mut gemini_call = call("call#echo#0", "echo");
    gemini_call.thought_signatures = Some(vec!["opaque-gemini-signature".into()]);
    let provider = Arc::new(FakeProvider::new(vec![
        FakeResponse::tool_calls(vec![gemini_call.clone()]),
        FakeResponse::tool_calls(vec![gemini_call]),
        FakeResponse::final_text("done"),
        FakeResponse::final_text("next turn"),
    ]));
    let handler = handler(
        pool.clone(),
        Arc::new(WithLeadingSignature {
            provider: provider.clone(),
            signature: "opaque-gemini-signature".into(),
        }),
        5,
    );
    turn(&handler, "run echo twice").await.unwrap();
    let live = provider.last_request().unwrap();
    let stored = history(&pool, &session(&pool).await).await;
    turn(&handler, "follow up").await.unwrap();
    let reconstructed = provider.last_request().unwrap();
    for input in [&live.messages, &stored, &reconstructed.messages] {
        assert_protocol(input);
        let calls: Vec<_> = input.iter().flat_map(|m| m.content.tool_calls()).collect();
        assert_eq!(calls.len(), 2);
        assert_ne!(calls[0].call_id, calls[1].call_id);
        for call in &calls {
            assert_eq!(call.fn_name, "echo");
            assert_eq!(call.fn_arguments, json!({"text": "echo context"}));
            assert_eq!(
                call.thought_signatures.as_deref(),
                Some(&["opaque-gemini-signature".to_string()][..])
            );
        }
        assert_eq!(input.iter().flat_map(|m| m.content.parts()).filter(|p| matches!(p, ContentPart::ThoughtSignature(s) if s == "opaque-gemini-signature")).count(), 2);
        assert!(!text(input).contains("Historical tool"));
        assert_eq!(
            serde_json::to_value(input).unwrap(),
            serde_json::to_value(normalize_history(input)).unwrap()
        );
    }
}

#[test]
fn namespaced_legacy_outputs_keep_function_names_and_avoid_existing_id_collisions() {
    let input = vec![
        calls(&["call#echo#0", "nerdbot_history_2_0"]),
        outputs(&["call#echo#0", "nerdbot_history_2_0"]),
        calls(&["call#echo#0"]),
        outputs(&["call#echo#0"]),
        outputs(&["call#echo#0"]),
    ];
    // The duplicate output in the second batch must still invalidate it.
    let safe = normalize_history(&input);
    assert_protocol(&safe);
    assert_eq!(safe.iter().flat_map(|m| m.content.tool_calls()).count(), 2);
    let safe = normalize_history(&input[..4]);
    assert_protocol(&safe);
    assert_eq!(safe.iter().flat_map(|m| m.content.tool_calls()).count(), 3);
    let last_call = safe[2].content.tool_calls()[0];
    assert_ne!(last_call.call_id, "nerdbot_history_2_0");
    let ContentPart::ToolResponse(last_output) = &safe[3].content.parts()[0] else {
        panic!("expected a structured output");
    };
    assert_eq!(last_output.call_id, last_call.call_id);
    assert_eq!(last_output.fn_name.as_deref(), Some("echo"));
    assert_eq!(
        serde_json::to_value(&safe).unwrap(),
        serde_json::to_value(normalize_history(&safe)).unwrap()
    );
}

#[tokio::test]
async fn compaction_omits_large_opaque_signatures_and_keeps_signed_call_arguments() {
    let pool = pool().await;
    let session = storage::sessions::create_session(&pool, 1).await.unwrap();
    let signature = "opaque-signature-".repeat(1000);
    let mut call = call("call#echo#0", "echo");
    call.fn_arguments = json!({"text": "essential argument to preserve"});
    call.thought_signatures = Some(vec![signature.clone()]);
    let signed = ChatMessage::assistant(MessageContent::from_parts(vec![
        ContentPart::ThoughtSignature(signature.clone()),
        ContentPart::ToolCall(call),
        ContentPart::Text("meaningful assistant text".into()),
    ]));
    let provider = Arc::new(FakeProvider::new(vec![FakeResponse::final_text("summary")]));
    let worker = CompactionWorker::new_with_preserve(provider.clone(), "fake".into(), 0.0, 0);
    // Valid and incomplete signed exchanges both use useful summary text.
    for complete in [true, false] {
        sqlx::query("DELETE FROM messages WHERE chat_session_id = ?")
            .bind(&session.id)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("DELETE FROM context_summaries WHERE chat_session_id = ?")
            .bind(&session.id)
            .execute(&pool)
            .await
            .unwrap();
        let assistant = messages::create_message(&pool, &session.id, &signed, None)
            .await
            .unwrap();
        let boundary = if complete {
            messages::create_message(&pool, &session.id, &outputs(&["call#echo#0"]), None)
                .await
                .unwrap()
                .id
        } else {
            assistant.id
        };
        worker
            .compact(&pool, &session.id, &budget(2000))
            .await
            .unwrap();
        let prompt = text(&provider.last_request().unwrap().messages);
        assert!(prompt.contains("essential argument to preserve"));
        assert!(prompt.contains("meaningful assistant text"));
        assert!(!prompt.contains("opaque-signature-"));
        assert_eq!(
            storage::summaries::get_latest_summary(&pool, &session.id)
                .await
                .unwrap()
                .unwrap()
                .covers_through_message_id,
            boundary
        );
        let persisted = history(&pool, &session.id).await;
        assert!(persisted[0].content.parts().iter().any(
            |part| matches!(part, ContentPart::ThoughtSignature(value) if value == &signature)
        ));
    }
}

#[tokio::test]
async fn oversized_prompts_return_size_notice_without_calling_llm() {
    for (prompt, limit) in [
        ("oversized user message ".repeat(1000), 2000),
        ("short user message".into(), 1),
    ] {
        let pool = pool().await;
        let provider = Arc::new(FakeProvider::new(vec![FakeResponse::final_text(
            "must not be called",
        )]));
        let mut config = AppConfig::default();
        config.llm.context_window_tokens = limit;
        config.llm.max_output_tokens = 0;
        config.context.reserved_tool_loop_tokens = 0;
        let handler = handler_with_config(pool.clone(), provider.clone(), config);
        let reply = turn(&handler, &prompt).await.unwrap().unwrap();
        assert!(reply.contains("input size limit"));
        assert!(reply.contains("shorten"));
        assert_eq!(provider.call_count(), 0);
        let persisted = history(&pool, &session(&pool).await).await;
        assert_eq!(persisted.len(), 2);
        assert_eq!(persisted[0].role, ChatRole::User);
        assert_eq!(persisted[0].content.joined_texts().unwrap(), prompt);
        assert_eq!(persisted[1].role, ChatRole::Assistant);
        assert_eq!(persisted[1].content.joined_texts().unwrap(), reply);
        assert_protocol(&persisted);
    }
}
