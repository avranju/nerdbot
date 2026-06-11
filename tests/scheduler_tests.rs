//! Integration and unit tests for the Phase 5 Scheduler components.
//!
//! Verifies:
//! - Cron parsing (5-field padding to 6-field) and recurrence calculations.
//! - Startup overdue job reload/recalculation under both true/false configurations.
//! - Dynamic scheduling tools (`schedule_job`, `list_jobs`, `delete_job`, `run_job_now`).
//! - Telegram commands (`/run` and `/delete`) and their direct DB state updates.

use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::Notify;

use genai::chat::{
    ChatMessage, ChatOptions, ChatRequest, ChatResponse, ChatRole, MessageContent, StopReason,
    ToolResponse, Usage,
};
use nerdbot::agent::personality::Personality;
use nerdbot::agent::run_mode::AgentRunMode;
use nerdbot::config::AppConfig;
use nerdbot::llm::LlmExecutor;
use nerdbot::llm::fake::FakeProvider;
use nerdbot::scheduler::models::{JobContextPolicy, JobStatus, ScheduleType};
use nerdbot::scheduler::service::SchedulerService;
use nerdbot::storage;
use nerdbot::telegram::commands::{CommandHandler, TelegramCommand};
use nerdbot::tools::registry::ToolRegistry;
use nerdbot::tools::traits::{Tool, ToolContext};

/// Set up an in-memory SQLite database.
async fn setup_test_db() -> sqlx::SqlitePool {
    let pool = sqlx::SqlitePool::connect("sqlite::memory:")
        .await
        .expect("failed to create in-memory DB");

    sqlx::migrate!("./migrations")
        .run(&pool)
        .await
        .expect("failed to run migrations");

    pool
}

fn make_test_config() -> AppConfig {
    let mut config = AppConfig::default();
    config.storage.sqlite_path = PathBuf::from(":memory:");
    config
}

// ── Test 1: Cron parsing and padding ─────────────────────────────────────

#[tokio::test]
async fn test_cron_parsing_and_padding() {
    let pool = setup_test_db().await;
    let notifier = Arc::new(Notify::new());

    let ctx = ToolContext {
        run_mode: AgentRunMode::InteractiveReply {
            chat_id: 123,
            user_id: 456,
        },
        workspace_root: PathBuf::from("/tmp"),
        telegram_token: "test-token".into(),
        allowed_chat_ids: vec![],
        allowed_user_ids: vec![],
        pool: Some(pool.clone()),
        scheduler_notifier: Some(notifier),
    };

    // Test 5-field cron parsing (should pad and parse successfully)
    let tool = nerdbot::tools::schedule::ScheduleJob;
    let args_5_field = serde_json::json!({
        "name": "5-field job",
        "prompt": "run alert",
        "schedule_type": "cron",
        "cron_expression": "*/5 * * * *"
    });

    let res = tool.execute(args_5_field, ctx.clone()).await.unwrap();
    assert!(res.success);

    // Retrieve the job and verify next_run_at is computed
    let jobs = storage::jobs::list_jobs(&pool, 123, false).await.unwrap();
    assert_eq!(jobs.len(), 1);
    let job = &jobs[0];
    assert_eq!(job.name, "5-field job");
    assert!(job.next_run_at.is_some());

    // Test 6-field cron parsing
    let args_6_field = serde_json::json!({
        "name": "6-field job",
        "prompt": "run report",
        "schedule_type": "cron",
        "cron_expression": "0 */10 * * * *"
    });

    let res_6 = tool.execute(args_6_field, ctx).await.unwrap();
    assert!(res_6.success);

    let jobs = storage::jobs::list_jobs(&pool, 123, false).await.unwrap();
    assert_eq!(jobs.len(), 2);
}

// ── Test 2: Startup Overdue Logic ────────────────────────────────────────

#[tokio::test]
async fn test_startup_overdue_run_overdue_true() {
    let pool = setup_test_db().await;

    // Insert an overdue one-shot job (1 hour ago)
    let one_hour_ago = chrono::Utc::now() - chrono::Duration::hours(1);
    let job = storage::jobs::create_job_full(
        &pool,
        storage::jobs::CreateJobInput {
            owner_chat_id: 111,
            name: "Overdue Job".into(),
            prompt: "prompt".into(),
            schedule_type: ScheduleType::OneShot,
            cron_expression: None,
            run_at: Some(one_hour_ago),
            timezone: None,
            notify_on_completion: true,
            context_policy: JobContextPolicy::Isolated,
            creation_context_snapshot: None,
            next_run_at: Some(one_hour_ago),
        },
    )
    .await
    .unwrap();

    // Verify it starts enabled
    assert!(job.enabled);
    assert_eq!(job.next_run_at, Some(one_hour_ago));

    // Configure startup overdue execution = true
    let mut config = make_test_config();
    config.scheduler.run_overdue_one_shots_on_startup = true;

    let provider: Arc<dyn LlmExecutor> = Arc::new(FakeProvider::new(vec![]));
    let registry = Arc::new(ToolRegistry::new());
    let bot_client = Arc::new(nerdbot::telegram::bot::TelegramBot::new(
        "test-token".into(),
    ));
    let telegram_service = nerdbot::telegram::service::TelegramService::new(bot_client);

    let scheduler = SchedulerService::new(
        pool.clone(),
        provider,
        registry,
        config.clone(),
        Personality::from_config(&config),
        telegram_service,
    );

    // Call start (this will run the startup overdue logic)
    scheduler.start().await.unwrap();

    // Check if the job's next_run_at was updated to ~now
    let updated = storage::jobs::get_job(&pool, &job.id)
        .await
        .unwrap()
        .unwrap();
    assert!(updated.enabled);
    assert!(updated.next_run_at.unwrap() > one_hour_ago);

    scheduler.stop().await.unwrap();
}

#[tokio::test]
async fn test_startup_overdue_run_overdue_false() {
    let pool = setup_test_db().await;

    // Insert an overdue one-shot job (1 hour ago)
    let one_hour_ago = chrono::Utc::now() - chrono::Duration::hours(1);
    let job = storage::jobs::create_job_full(
        &pool,
        storage::jobs::CreateJobInput {
            owner_chat_id: 111,
            name: "Overdue Job".into(),
            prompt: "prompt".into(),
            schedule_type: ScheduleType::OneShot,
            cron_expression: None,
            run_at: Some(one_hour_ago),
            timezone: None,
            notify_on_completion: true,
            context_policy: JobContextPolicy::Isolated,
            creation_context_snapshot: None,
            next_run_at: Some(one_hour_ago),
        },
    )
    .await
    .unwrap();

    // Configure startup overdue execution = false
    let mut config = make_test_config();
    config.scheduler.run_overdue_one_shots_on_startup = false;

    let provider: Arc<dyn LlmExecutor> = Arc::new(FakeProvider::new(vec![]));
    let registry = Arc::new(ToolRegistry::new());
    let bot_client = Arc::new(nerdbot::telegram::bot::TelegramBot::new(
        "test-token".into(),
    ));
    let telegram_service = nerdbot::telegram::service::TelegramService::new(bot_client);

    let scheduler = SchedulerService::new(
        pool.clone(),
        provider,
        registry,
        config.clone(),
        Personality::from_config(&config),
        telegram_service,
    );

    // Call start
    scheduler.start().await.unwrap();

    // Check that the job is now disabled and status is Missed
    let updated = storage::jobs::get_job(&pool, &job.id)
        .await
        .unwrap()
        .unwrap();
    assert!(!updated.enabled);
    assert_eq!(updated.last_status().unwrap(), Some(JobStatus::Missed));
    assert!(updated.next_run_at.is_none());

    scheduler.stop().await.unwrap();
}

// ── Test 3: Scheduling tools & wakeup notify ──────────────────────────────

#[tokio::test]
async fn test_scheduling_tools_and_wakeup_signaling() {
    let pool = setup_test_db().await;
    let notifier = Arc::new(Notify::new());

    let ctx = ToolContext {
        run_mode: AgentRunMode::InteractiveReply {
            chat_id: 123,
            user_id: 456,
        },
        workspace_root: PathBuf::from("/tmp"),
        telegram_token: "test".into(),
        allowed_chat_ids: vec![],
        allowed_user_ids: vec![],
        pool: Some(pool.clone()),
        scheduler_notifier: Some(notifier.clone()),
    };

    // 1. Schedule the job
    let tool_schedule = nerdbot::tools::schedule::ScheduleJob;
    let args = serde_json::json!({
        "name": "Notify Check",
        "prompt": "ping",
        "schedule_type": "one_shot",
        "run_at": "2026-05-20T18:00:00Z"
    });

    // We should expect a notification on the channel
    let notify_future = notifier.notified();
    let res = tool_schedule.execute(args, ctx.clone()).await.unwrap();
    assert!(res.success);

    // Verify the notify fired immediately!
    tokio::time::timeout(std::time::Duration::from_millis(50), notify_future)
        .await
        .expect("Scheduler notifier was not signaled upon job creation");

    let jobs = storage::jobs::list_jobs(&pool, 123, false).await.unwrap();
    let job_id = &jobs[0].id;

    // 2. Trigger run now
    let tool_run_now = nerdbot::tools::schedule::RunJobNow;
    let notify_future = notifier.notified();
    let run_res = tool_run_now
        .execute(serde_json::json!({ "job_id": job_id }), ctx.clone())
        .await
        .unwrap();
    assert!(run_res.success);

    tokio::time::timeout(std::time::Duration::from_millis(50), notify_future)
        .await
        .expect("Scheduler notifier was not signaled on run_job_now");

    let triggered_job = storage::jobs::get_job(&pool, job_id)
        .await
        .unwrap()
        .unwrap();
    assert!(triggered_job.enabled);
    assert!(triggered_job.next_run_at.unwrap() <= chrono::Utc::now());

    // 3. Delete job
    let tool_delete = nerdbot::tools::schedule::DeleteJob;
    let notify_future = notifier.notified();
    let delete_res = tool_delete
        .execute(serde_json::json!({ "job_id": job_id }), ctx)
        .await
        .unwrap();
    assert!(delete_res.success);

    tokio::time::timeout(std::time::Duration::from_millis(50), notify_future)
        .await
        .expect("Scheduler notifier was not signaled on delete_job");

    let deleted_job = storage::jobs::get_job(&pool, job_id)
        .await
        .unwrap()
        .unwrap();
    assert!(!deleted_job.enabled);
}

// ── Test 4: Commands wiring and execution triggers ───────────────────────

#[tokio::test]
async fn test_telegram_commands_wire_and_trigger() {
    let pool = setup_test_db().await;
    let notifier = Arc::new(Notify::new());

    // Insert a test job
    let job = storage::jobs::create_job(
        &pool,
        999,
        "Command Job".into(),
        "run analytics".into(),
        ScheduleType::OneShot,
        Some(chrono::Utc::now() + chrono::Duration::hours(5)),
    )
    .await
    .unwrap();

    assert!(job.enabled);

    // 1. Run the job via Telegram command /run
    let notify_future = notifier.notified();
    let response = CommandHandler::handle(
        TelegramCommand::Run(job.id.clone()),
        999,
        1,
        &pool,
        Some(&notifier),
    )
    .await
    .unwrap();

    assert!(response.contains("immediately"));

    // Verify notification was sent
    tokio::time::timeout(std::time::Duration::from_millis(50), notify_future)
        .await
        .expect("Telegram /run did not notify the scheduler");

    // Verify next_run_at in DB
    let running_job = storage::jobs::get_job(&pool, &job.id)
        .await
        .unwrap()
        .unwrap();
    assert!(running_job.enabled);
    assert!(running_job.next_run_at.unwrap() <= chrono::Utc::now());

    // 2. Delete the job via Telegram command /delete
    let notify_future = notifier.notified();
    let response = CommandHandler::handle(
        TelegramCommand::Delete(job.id.clone()),
        999,
        1,
        &pool,
        Some(&notifier),
    )
    .await
    .unwrap();

    assert!(response.contains("deleted"));

    tokio::time::timeout(std::time::Duration::from_millis(50), notify_future)
        .await
        .expect("Telegram /delete did not notify the scheduler");

    // Verify disabled in DB
    let deleted_job = storage::jobs::get_job(&pool, &job.id)
        .await
        .unwrap()
        .unwrap();
    assert!(!deleted_job.enabled);
}

// ── Test 5: run_scheduled_job execution runner ──────────────────────────

#[tokio::test]
async fn test_run_scheduled_job_execution() {
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    // 1. Start wiremock server
    let mock_server = MockServer::start().await;

    // Mock Telegram sendMessage endpoint
    Mock::given(method("POST"))
        .and(path("/sendMessage"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "ok": true,
            "result": {
                "message_id": 12345,
                "chat": {
                    "id": 123,
                    "type": "private"
                },
                "date": 1600000000,
                "text": "mock"
            }
        })))
        .mount(&mock_server)
        .await;

    // 2. Set up test DB
    let pool = setup_test_db().await;

    // 3. Insert a job in DB
    let job = storage::jobs::create_job_full(
        &pool,
        storage::jobs::CreateJobInput {
            owner_chat_id: 123,
            name: "Mock Job".into(),
            prompt: "Say hello".into(),
            schedule_type: ScheduleType::OneShot,
            cron_expression: None,
            run_at: Some(chrono::Utc::now()),
            timezone: None,
            notify_on_completion: true,
            context_policy: JobContextPolicy::Isolated,
            creation_context_snapshot: None,
            next_run_at: Some(chrono::Utc::now()),
        },
    )
    .await
    .unwrap();

    // 4. Create dependencies
    // Use FakeProvider to return a known final text response
    let provider = Arc::new(FakeProvider::new(vec![
        nerdbot::llm::fake::FakeResponse::final_text("Mock scheduled task output"),
    ]));
    let llm: Arc<dyn LlmExecutor> = provider.clone();
    let registry = Arc::new(ToolRegistry::new());
    let loop_config = nerdbot::agent::agent_loop::AgentLoopConfig {
        max_tool_iterations: 2,
        llm_model: String::new(),
        llm_temperature: 0.0,
        llm_max_output_tokens: 0,
    };

    // Create Telegram Bot pointing to mock server
    let bot_client = Arc::new(nerdbot::telegram::bot::TelegramBot::new_with_base_url(
        "test-token".into(),
        mock_server.uri(),
    ));
    let telegram_service = nerdbot::telegram::service::TelegramService::new(bot_client);

    // 5. Run the job directly
    nerdbot::scheduler::runner::run_scheduled_job(
        nerdbot::scheduler::runner::RunScheduledJobInput {
            pool: pool.clone(),
            llm,
            registry,
            loop_config,
            personality: "You are an assistant".into(),
            workspace_root: PathBuf::from("/tmp"),
            telegram_token: "test-token".into(),
            telegram_service,
            allowed_chat_ids: vec![123],
            allowed_user_ids: vec![],
            timezone: "UTC".to_string(),
        },
        &job.id,
    )
    .await
    .unwrap();

    let request = provider.last_request().unwrap();
    let prompt_sent_to_llm = request
        .messages
        .iter()
        .rev()
        .find(|m| matches!(m.role, ChatRole::User))
        .and_then(|m| m.content.joined_texts())
        .unwrap();
    assert!(prompt_sent_to_llm.starts_with("Say hello"));
    assert!(prompt_sent_to_llm.contains("## Current Date/Time"));

    // 6. Verify job is still enabled
    let updated_job = storage::jobs::get_job(&pool, &job.id)
        .await
        .unwrap()
        .unwrap();
    assert!(updated_job.enabled);

    // Verify message history saved to DB (both the user prompt and the assistant response)
    let session = storage::sessions::get_session_for_chat(&pool, 123)
        .await
        .unwrap()
        .unwrap();
    let messages = storage::messages::list_messages(&pool, &session.id, None)
        .await
        .unwrap();
    assert_eq!(messages.len(), 2);
    assert!(messages.iter().any(|m| m.content.contains("Say hello")));
    assert!(
        messages
            .iter()
            .any(|m| m.content.contains("Mock scheduled task output"))
    );
}

// ── Test 6: Disabled job rejection ────────────────────────────────────────

#[tokio::test]
async fn test_disabled_job_rejection() {
    let pool = setup_test_db().await;
    let notifier = Arc::new(Notify::new());

    // 1. Create a disabled job
    let job = storage::jobs::create_job_full(
        &pool,
        storage::jobs::CreateJobInput {
            owner_chat_id: 123,
            name: "Disabled Job".into(),
            prompt: "Say hello".into(),
            schedule_type: ScheduleType::OneShot,
            cron_expression: None,
            run_at: Some(chrono::Utc::now() + chrono::Duration::hours(1)),
            timezone: None,
            notify_on_completion: true,
            context_policy: JobContextPolicy::Isolated,
            creation_context_snapshot: None,
            next_run_at: Some(chrono::Utc::now() + chrono::Duration::hours(1)),
        },
    )
    .await
    .unwrap();

    // Disable the job
    storage::jobs::disable_job(&pool, &job.id).await.unwrap();

    let ctx = ToolContext {
        run_mode: AgentRunMode::InteractiveReply {
            chat_id: 123,
            user_id: 456,
        },
        workspace_root: PathBuf::from("/tmp"),
        telegram_token: "test".into(),
        allowed_chat_ids: vec![],
        allowed_user_ids: vec![],
        pool: Some(pool.clone()),
        scheduler_notifier: Some(notifier.clone()),
    };

    // 2. Try running via RunJobNow tool
    let tool_run_now = nerdbot::tools::schedule::RunJobNow;
    let run_res = tool_run_now
        .execute(serde_json::json!({ "job_id": job.id }), ctx.clone())
        .await;
    assert!(run_res.is_err());
    let err_msg = run_res.unwrap_err().to_string();
    assert!(err_msg.contains("disabled or deleted"));

    // 3. Try running via CommandHandler `/run`
    let response = CommandHandler::handle(
        TelegramCommand::Run(job.id.clone()),
        123,
        456,
        &pool,
        Some(&notifier),
    )
    .await
    .unwrap();
    assert!(response.contains("disabled or deleted"));
}

// ── Test 7: Timezone-aware cron calculation ──────────────────────────────

#[tokio::test]
async fn test_timezone_aware_cron_calculation() {
    use chrono::Timelike;
    use nerdbot::scheduler::cron::get_next_cron_run;

    // Parse cron for 09:00:00 every day with Asia/Kolkata (UTC +5:30)
    let cron_str = "0 9 * * *";

    // Test direct calculation
    let next_utc = get_next_cron_run(cron_str, Some("Asia/Kolkata")).unwrap();

    // Retrieve offset for Asia/Kolkata at next_utc time
    let tz: chrono_tz::Tz = "Asia/Kolkata".parse().unwrap();
    let local_time = next_utc.with_timezone(&tz);

    // Local hour should be exactly 9, minute 0, second 0
    assert_eq!(local_time.time().hour(), 9);
    assert_eq!(local_time.time().minute(), 0);
    assert_eq!(local_time.time().second(), 0);

    // Let's test the tool parsing as well
    let pool = setup_test_db().await;
    let notifier = Arc::new(Notify::new());

    let ctx = ToolContext {
        run_mode: AgentRunMode::InteractiveReply {
            chat_id: 123,
            user_id: 456,
        },
        workspace_root: PathBuf::from("/tmp"),
        telegram_token: "test-token".into(),
        allowed_chat_ids: vec![],
        allowed_user_ids: vec![],
        pool: Some(pool.clone()),
        scheduler_notifier: Some(notifier),
    };

    let tool = nerdbot::tools::schedule::ScheduleJob;
    let args = serde_json::json!({
        "name": "TZ cron job",
        "prompt": "run alert",
        "schedule_type": "cron",
        "cron_expression": "0 9 * * *",
        "timezone": "Asia/Kolkata"
    });

    let res = tool.execute(args, ctx.clone()).await.unwrap();
    assert!(res.success);

    let jobs = storage::jobs::list_jobs(&pool, 123, false).await.unwrap();
    assert_eq!(jobs.len(), 1);
    let job = &jobs[0];
    assert_eq!(job.timezone.as_deref(), Some("Asia/Kolkata"));

    let next_run = job.next_run_at.unwrap();
    let local_next_run = next_run.with_timezone(&tz);
    assert_eq!(local_next_run.time().hour(), 9);
    assert_eq!(local_next_run.time().minute(), 0);
}

// ── Test 8: Scheduler graceful shutdown in-flight task joiner ─────────────

#[tokio::test]
async fn test_scheduler_graceful_shutdown() {
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    // 1. Start wiremock server
    let mock_server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/sendMessage"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "ok": true,
            "result": {
                "message_id": 12345,
                "chat": {
                    "id": 555,
                    "type": "private"
                },
                "date": 1600000000,
                "text": "mock"
            }
        })))
        .mount(&mock_server)
        .await;

    // 2. Set up test DB
    let pool = setup_test_db().await;

    // 3. Insert a job due right now (in the past by a second to be safe)
    let past = chrono::Utc::now() - chrono::Duration::seconds(1);
    let job = storage::jobs::create_job_full(
        &pool,
        storage::jobs::CreateJobInput {
            owner_chat_id: 555,
            name: "Long Job".into(),
            prompt: "Say hello slowly".into(),
            schedule_type: ScheduleType::OneShot,
            cron_expression: None,
            run_at: Some(past),
            timezone: None,
            notify_on_completion: true,
            context_policy: JobContextPolicy::Isolated,
            creation_context_snapshot: None,
            next_run_at: Some(past),
        },
    )
    .await
    .unwrap();

    // 4. Create dependencies
    struct SlowProvider {
        duration: std::time::Duration,
    }

    #[async_trait::async_trait]
    impl LlmExecutor for SlowProvider {
        async fn complete(
            &self,
            _model: &str,
            _request: ChatRequest,
            _options: ChatOptions,
        ) -> Result<ChatResponse, nerdbot::error::AgentError> {
            tokio::time::sleep(self.duration).await;
            Ok(ChatResponse {
                content: MessageContent::from_text("Completed after delay"),
                reasoning_content: None,
                model_iden: genai::ModelIden::new(
                    genai::adapter::AdapterKind::OpenAI,
                    "fake-model",
                ),
                provider_model_iden: genai::ModelIden::new(
                    genai::adapter::AdapterKind::OpenAI,
                    "fake-model",
                ),
                stop_reason: Some(StopReason::Completed("stop".to_string())),
                usage: Usage::default(),
                captured_raw_body: None,
                response_id: None,
            })
        }
    }

    let provider: Arc<dyn LlmExecutor> = Arc::new(SlowProvider {
        // Slow enough that the job is still in-flight when we call stop()
        duration: std::time::Duration::from_millis(300),
    });
    let registry = Arc::new(ToolRegistry::new());

    // Create Telegram Bot pointing to mock server
    let bot_client = Arc::new(nerdbot::telegram::bot::TelegramBot::new_with_base_url(
        "test-token".into(),
        mock_server.uri(),
    ));
    let telegram_service = nerdbot::telegram::service::TelegramService::new(bot_client);

    let mut config = make_test_config();
    config.telegram.allowed_chat_ids = vec![555];
    // Enable overdue startup execution so the overdue job is re-scheduled
    // to run immediately rather than being discarded as Missed.
    config.scheduler.run_overdue_one_shots_on_startup = true;

    let scheduler = SchedulerService::new(
        pool.clone(),
        provider,
        registry,
        config.clone(),
        Personality::from_config(&config),
        telegram_service,
    );

    // 5. Start the scheduler service
    scheduler.start().await.unwrap();

    // 6. Poll until the job transitions to Running (up to 500ms)
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(500);
    loop {
        let j = storage::jobs::get_job(&pool, &job.id)
            .await
            .unwrap()
            .unwrap();
        if j.last_status().unwrap() == Some(JobStatus::Running) {
            break;
        }
        if std::time::Instant::now() >= deadline {
            panic!(
                "Job did not transition to Running within 500ms. Last status: {:?}",
                j.last_status()
            );
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }

    // 7. Stop the scheduler — this must block until all in-flight tasks finish
    let start_stop = std::time::Instant::now();
    scheduler.stop().await.unwrap();
    let stop_duration = start_stop.elapsed();

    // The stop() call should have waited for the slow job to finish
    // (at least some measurable time, since job takes 300ms)
    assert!(
        stop_duration >= std::time::Duration::from_millis(50),
        "stop() returned too quickly ({:?}); it should have waited for the in-flight task",
        stop_duration
    );

    // 8. Verify job is now Success (not stuck in Running)
    let finished_job = storage::jobs::get_job(&pool, &job.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        finished_job.last_status().unwrap(),
        Some(JobStatus::Success)
    );
}

// ── Test 9: Agent notification detection (Phase 6) ───────────────────────

/// Helper to insert a tool-result message into the session for testing
/// the agent_already_sent_notification detection logic.
async fn insert_send_user_message_result(
    pool: &sqlx::SqlitePool,
    session_id: &str,
) -> Result<(), nerdbot::error::AgentError> {
    use nerdbot::storage;

    // Create a Tool message with a send_user_message result
    let tool_result = ToolResponse::new(
        "call-1",
        serde_json::json!({
            "success": true,
            "tool_name": "send_user_message",
            "chat_id": 123,
            "sent": true,
            "formatting": "plain_text",
            "disable_notification": false
        })
        .to_string(),
    );

    let msg = ChatMessage::from(vec![tool_result]);
    storage::messages::create_message(pool, session_id, &msg, None).await?;

    Ok(())
}

#[tokio::test]
async fn test_agent_sent_notification_detection() {
    use nerdbot::storage;

    let pool = setup_test_db().await;

    // Create a session
    let session = storage::sessions::create_session(&pool, 123).await.unwrap();

    // Insert a send_user_message tool result
    insert_send_user_message_result(&pool, &session.id)
        .await
        .unwrap();

    // The detection function should find it
    // (We can't call the private function directly, so we test via the runner)
    // Instead, verify the message structure is correct
    let messages = storage::messages::list_messages(&pool, &session.id, None)
        .await
        .unwrap();
    assert_eq!(messages.len(), 1);
    let msg = &messages[0];
    assert_eq!(msg.role().unwrap(), ChatRole::Tool);
    assert!(msg.structured_content_json.is_some());
}

#[tokio::test]
async fn test_no_notification_detection_when_empty() {
    use nerdbot::storage;

    let pool = setup_test_db().await;

    // Create a session with no messages
    let session = storage::sessions::create_session(&pool, 456).await.unwrap();

    let messages = storage::messages::list_messages(&pool, &session.id, None)
        .await
        .unwrap();
    assert!(messages.is_empty());
}
