//! Todo tool dispatch, validation, and persistent ID guarantees.

use genai::chat::ToolCall;
use nerdbot::agent::run_mode::AgentRunMode;
use nerdbot::channel::ChannelAccessPolicy;
use nerdbot::error::AgentError;
use nerdbot::storage::{Database, todos};
use nerdbot::tools::registry::ToolRegistry;
use nerdbot::tools::todo::{
    TodoAdd, TodoComplete, TodoDelete, TodoListAll, TodoListPending, TodoUpdate,
};
use nerdbot::tools::traits::{ToolContext, ToolOutput};
use serde_json::{Value, json};
use tempfile::TempDir;

async fn setup() -> (TempDir, Database) {
    let dir = tempfile::tempdir().unwrap();
    let db = Database::new(dir.path().join("test.db")).await.unwrap();
    db.init().await.unwrap();
    (dir, db)
}

fn context(db: &Database) -> ToolContext {
    ToolContext {
        run_mode: AgentRunMode::Internal {
            reason: "todo test".into(),
        },
        workspace_root: std::env::temp_dir(),
        access_policy: ChannelAccessPolicy::allow_all(),
        channel_registry: None,
        pool: Some(db.pool().clone()),
        scheduler_notifier: None,
    }
}

fn registry() -> ToolRegistry {
    let mut registry = ToolRegistry::new();
    registry.register(TodoAdd).unwrap();
    registry.register(TodoListAll).unwrap();
    registry.register(TodoListPending).unwrap();
    registry.register(TodoComplete).unwrap();
    registry.register(TodoUpdate).unwrap();
    registry.register(TodoDelete).unwrap();
    registry
}

async fn call(
    registry: &ToolRegistry,
    ctx: &ToolContext,
    name: &str,
    args: Value,
) -> Result<ToolOutput, AgentError> {
    registry
        .execute(
            &ToolCall {
                call_id: "test-call".into(),
                fn_name: name.into(),
                fn_arguments: args,
                thought_signatures: None,
            },
            ctx.clone(),
        )
        .await
}

#[tokio::test]
async fn todo_tools_lifecycle() {
    let (_dir, db) = setup().await;
    let ctx = context(&db);
    let registry = registry();
    let specs = registry.specs();
    assert_eq!(specs.len(), 6);
    assert_eq!(specs[0].name.as_str(), "todo_add");
    assert_eq!(specs[5].name.as_str(), "todo_delete");

    for name in ["todo_list_all", "todo_list_pending"] {
        let output = call(&registry, &ctx, name, json!({})).await.unwrap();
        assert!(output.success);
        assert_eq!(output.data, json!({ "todos": [], "count": 0 }));
    }

    let first = call(&registry, &ctx, "todo_add", json!({"text": "Buy milk"}))
        .await
        .unwrap();
    assert!(first.success);
    assert_eq!(
        first.data["todo"],
        json!({"id": 1, "text": "Buy milk", "completed": false})
    );
    let second = call(&registry, &ctx, "todo_add", json!({"text": "Read a book"}))
        .await
        .unwrap();
    assert_eq!(second.data["todo"]["id"], 2);

    let updated = call(
        &registry,
        &ctx,
        "todo_update",
        json!({"id": 2, "text": "Read two books"}),
    )
    .await
    .unwrap();
    assert!(updated.success);
    assert_eq!(
        updated.data["todo"],
        json!({"id": 2, "text": "Read two books", "completed": false})
    );

    for _ in 0..2 {
        let completed = call(&registry, &ctx, "todo_complete", json!({"id": 1}))
            .await
            .unwrap();
        assert!(completed.success);
        assert_eq!(
            completed.data["todo"],
            json!({"id": 1, "text": "Buy milk", "completed": true})
        );
    }
    let updated = call(
        &registry,
        &ctx,
        "todo_update",
        json!({"id": 1, "text": "Buy oat milk"}),
    )
    .await
    .unwrap();
    assert!(updated.success);
    assert_eq!(
        updated.data["todo"],
        json!({"id": 1, "text": "Buy oat milk", "completed": true})
    );

    let all = call(&registry, &ctx, "todo_list_all", json!({}))
        .await
        .unwrap();
    assert_eq!(
        all.data,
        json!({"count": 2, "todos": [
            {"id": 1, "text": "Buy oat milk", "completed": true},
            {"id": 2, "text": "Read two books", "completed": false}
        ]})
    );
    let pending = call(&registry, &ctx, "todo_list_pending", json!({}))
        .await
        .unwrap();
    assert_eq!(
        pending.data,
        json!({"count": 1, "todos": [
            {"id": 2, "text": "Read two books", "completed": false}
        ]})
    );

    let deleted = call(&registry, &ctx, "todo_delete", json!({"id": 1}))
        .await
        .unwrap();
    assert!(deleted.success);
    assert_eq!(deleted.data["id"], 1);
    let all = call(&registry, &ctx, "todo_list_all", json!({}))
        .await
        .unwrap();
    assert_eq!(all.data, pending.data); // Deleting #1 never renumbers #2.
}

#[tokio::test]
async fn missing_todos_are_reported_without_success() {
    let (_dir, db) = setup().await;
    let ctx = context(&db);
    let registry = registry();
    for (name, args) in [
        ("todo_complete", json!({"id": 42})),
        ("todo_update", json!({"id": 42, "text": "Missing"})),
        ("todo_delete", json!({"id": 42})),
    ] {
        let output = call(&registry, &ctx, name, args).await.unwrap();
        assert!(!output.success);
        assert_eq!(output.data, json!({"id": 42, "error": "not_found"}));
        assert!(output.summary.contains("42"));
    }
}

#[tokio::test]
async fn invalid_arguments_do_not_mutate_todos() {
    let (_dir, db) = setup().await;
    let ctx = context(&db);
    let registry = registry();
    for args in [
        json!({}),
        json!({"text": null}),
        json!({"text": 123}),
        json!({"text": ""}),
        json!({"text": " \n\t\u{2003}"}),
        json!({"text": "Valid", "unexpected": true}),
        Value::Null,
        json!([]),
    ] {
        assert!(matches!(
            call(&registry, &ctx, "todo_add", args).await,
            Err(AgentError::InvalidToolArgs(_))
        ));
    }
    call(&registry, &ctx, "todo_add", json!({"text": "Keep me"}))
        .await
        .unwrap();
    for name in ["todo_complete", "todo_delete", "todo_update"] {
        for id in [
            Value::Null,
            json!(0),
            json!(-1),
            json!(1.5),
            json!("1"),
            json!(true),
            json!(u64::MAX),
        ] {
            let mut args = json!({"id": id});
            if name == "todo_update" {
                args["text"] = json!("New text");
            }
            assert!(matches!(
                call(&registry, &ctx, name, args).await,
                Err(AgentError::InvalidToolArgs(_))
            ));
        }
        assert!(matches!(
            call(&registry, &ctx, name, json!({})).await,
            Err(AgentError::InvalidToolArgs(_))
        ));
    }
    for args in [
        json!({"id": 1}),
        json!({"id": 1, "text": "  "}),
        json!({"id": 1, "text": false}),
    ] {
        assert!(matches!(
            call(&registry, &ctx, "todo_update", args).await,
            Err(AgentError::InvalidToolArgs(_))
        ));
    }
    for name in ["todo_list_all", "todo_list_pending"] {
        for args in [Value::Null, json!([]), json!({"id": 1})] {
            assert!(matches!(
                call(&registry, &ctx, name, args).await,
                Err(AgentError::InvalidToolArgs(_))
            ));
        }
    }
    let remaining = todos::list_todos(db.pool(), false).await.unwrap();
    assert_eq!(remaining.len(), 1);
    assert_eq!(remaining[0].text, "Keep me");
    assert!(!remaining[0].completed);
}

#[tokio::test]
async fn missing_storage_is_an_error_for_every_tool() {
    let (_dir, db) = setup().await;
    let mut ctx = context(&db);
    ctx.pool = None;
    let registry = registry();
    for (name, args) in [
        ("todo_add", json!({"text": "Test"})),
        ("todo_list_all", json!({})),
        ("todo_list_pending", json!({})),
        ("todo_complete", json!({"id": 1})),
        ("todo_update", json!({"id": 1, "text": "Test"})),
        ("todo_delete", json!({"id": 1})),
    ] {
        assert!(matches!(
            call(&registry, &ctx, name, args).await,
            Err(AgentError::ToolExecution(_))
        ));
    }
}

#[tokio::test]
async fn ids_are_never_reused_and_survive_restarts() {
    let (dir, db) = setup().await;
    let first = todos::create_todo(db.pool(), "First").await.unwrap();
    let second = todos::create_todo(db.pool(), "Second").await.unwrap();
    assert_eq!(first.id, 1);
    assert_eq!(second.id, 2);
    assert!(todos::delete_todo(db.pool(), second.id).await.unwrap());
    let third = todos::create_todo(db.pool(), "Third").await.unwrap();
    assert_eq!(third.id, 3); // Highest deleted ID is not reused.
    assert!(todos::delete_todo(db.pool(), first.id).await.unwrap());
    assert!(todos::delete_todo(db.pool(), third.id).await.unwrap());
    db.pool().close().await;

    let db = Database::new(dir.path().join("test.db")).await.unwrap();
    db.init().await.unwrap();
    let fourth = todos::create_todo(db.pool(), "Fourth").await.unwrap();
    assert_eq!(fourth.id, 4); // Even an empty list remembers its last ID after restart.
    todos::complete_todo(db.pool(), fourth.id).await.unwrap();
    let updated = todos::update_todo(db.pool(), fourth.id, "Updated")
        .await
        .unwrap()
        .unwrap();
    db.pool().close().await;

    let db = Database::new(dir.path().join("test.db")).await.unwrap();
    db.init().await.unwrap();
    assert_eq!(
        todos::list_todos(db.pool(), false).await.unwrap(),
        vec![updated]
    );
    assert!(todos::list_todos(db.pool(), true).await.unwrap().is_empty());
}

#[tokio::test]
async fn concurrent_adds_receive_unique_ids() {
    let (_dir, db) = setup().await;
    let mut tasks = tokio::task::JoinSet::new();
    for i in 0..20 {
        let pool = db.pool().clone();
        tasks.spawn(async move {
            todos::create_todo(&pool, &format!("Todo {i}"))
                .await
                .unwrap()
                .id
        });
    }
    let mut ids = Vec::new();
    while let Some(result) = tasks.join_next().await {
        ids.push(result.unwrap());
    }
    ids.sort_unstable();
    assert_eq!(ids, (1..=20).collect::<Vec<i64>>());
    assert_eq!(todos::list_todos(db.pool(), false).await.unwrap().len(), 20);
}

#[tokio::test]
async fn todo_text_round_trips_without_sql_interpretation() {
    let (_dir, db) = setup().await;
    let registry = registry();
    let ctx = context(&db);
    let text = "  日本語 🦀\nRobert'); DROP TABLE todos;--  ";
    let added = call(&registry, &ctx, "todo_add", json!({"text": text}))
        .await
        .unwrap();
    assert_eq!(added.data["todo"]["text"], text);
    let replacement = format!("{text} updated");
    let updated = call(
        &registry,
        &ctx,
        "todo_update",
        json!({"id": 1, "text": replacement}),
    )
    .await
    .unwrap();
    assert_eq!(updated.data["todo"]["text"], replacement);
    assert_eq!(
        todos::list_todos(db.pool(), false).await.unwrap()[0].text,
        replacement
    );
}

#[tokio::test]
async fn migration_upgrades_existing_database_without_changing_history() {
    let dir = tempfile::tempdir().unwrap();
    let old_migrations = dir.path().join("old_migrations");
    std::fs::create_dir(&old_migrations).unwrap();
    std::fs::copy(
        concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/migrations/00000000000001_init.sql"
        ),
        old_migrations.join("00000000000001_init.sql"),
    )
    .unwrap();
    let db = Database::new(dir.path().join("test.db")).await.unwrap();
    sqlx::migrate::Migrator::new(old_migrations.as_path())
        .await
        .unwrap()
        .run(db.pool())
        .await
        .unwrap();
    let session = nerdbot::storage::sessions::create_session(db.pool(), 123)
        .await
        .unwrap();
    db.init().await.unwrap();
    db.init().await.unwrap(); // Already-applied migrations remain idempotent.
    let loaded = nerdbot::storage::sessions::get_session(db.pool(), &session.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(loaded.conversation_id, "123");
    assert_eq!(
        todos::create_todo(db.pool(), "After upgrade")
            .await
            .unwrap()
            .id,
        1
    );
}
