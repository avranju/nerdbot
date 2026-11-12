//! Six tools for the persistent, instance-wide todo list.

use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use sqlx::SqlitePool;

use crate::error::AgentError;
use crate::storage::todos::{self, StoredTodo};
use crate::tools::traits::{Tool, ToolContext, ToolOutput};

pub struct TodoAdd;
pub struct TodoListAll;
pub struct TodoListPending;
pub struct TodoComplete;
pub struct TodoUpdate;
pub struct TodoDelete;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TextArgs {
    text: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct IdArgs {
    id: i64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UpdateArgs {
    id: i64,
    text: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ListArgs {}

fn parse_args<T: DeserializeOwned>(args: Value) -> Result<T, AgentError> {
    if !args.is_object() {
        return Err(AgentError::InvalidToolArgs(
            "arguments must be a JSON object".into(),
        ));
    }
    serde_json::from_value(args).map_err(|e| AgentError::InvalidToolArgs(e.to_string()))
}

fn validate_text(text: &str) -> Result<(), AgentError> {
    if text.trim().is_empty() {
        return Err(AgentError::InvalidToolArgs(
            "text must contain non-whitespace characters".into(),
        ));
    }
    Ok(())
}

fn validate_id(id: i64) -> Result<(), AgentError> {
    if id <= 0 {
        return Err(AgentError::InvalidToolArgs(
            "id must be a positive integer".into(),
        ));
    }
    Ok(())
}

fn pool(ctx: &ToolContext) -> Result<&SqlitePool, AgentError> {
    ctx.pool
        .as_ref()
        .ok_or_else(|| AgentError::ToolExecution("Todo storage is not configured".into()))
}

fn todo_output(todo: StoredTodo, action: &str) -> ToolOutput {
    ToolOutput {
        success: true,
        summary: format!("{action} todo #{}.", todo.id),
        data: json!({ "todo": todo }),
    }
}

fn not_found(id: i64) -> ToolOutput {
    ToolOutput {
        success: false,
        summary: format!("Todo #{id} was not found."),
        data: json!({ "id": id, "error": "not_found" }),
    }
}

fn list_schema() -> Value {
    json!({
        "type": "object",
        "properties": {},
        "additionalProperties": false
    })
}

fn id_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "id": { "type": "integer", "minimum": 1, "description": "The stable todo ID returned by todo_add or a todo list tool." }
        },
        "required": ["id"],
        "additionalProperties": false
    })
}

async fn list(args: Value, ctx: ToolContext, pending_only: bool) -> Result<ToolOutput, AgentError> {
    let _: ListArgs = parse_args(args)?;
    let todos = todos::list_todos(pool(&ctx)?, pending_only).await?;
    let count = todos.len();
    let kind = if pending_only {
        "pending todos"
    } else {
        "todos"
    };
    Ok(ToolOutput {
        success: true,
        summary: format!("Found {count} {kind}."),
        data: json!({ "todos": todos, "count": count }),
    })
}

#[async_trait::async_trait]
impl Tool for TodoAdd {
    fn name(&self) -> &str {
        "todo_add"
    }

    fn description(&self) -> &str {
        "Add a pending item to the persistent todo list shared across this NerdBot instance. Returns a stable integer ID that is never reused."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "text": { "type": "string", "minLength": 1, "description": "The todo description; must not be blank." }
            },
            "required": ["text"],
            "additionalProperties": false
        })
    }

    async fn execute(&self, args: Value, ctx: ToolContext) -> Result<ToolOutput, AgentError> {
        let args: TextArgs = parse_args(args)?;
        validate_text(&args.text)?;
        let todo = todos::create_todo(pool(&ctx)?, &args.text).await?;
        Ok(todo_output(todo, "Added"))
    }
}

#[async_trait::async_trait]
impl Tool for TodoListAll {
    fn name(&self) -> &str {
        "todo_list_all"
    }

    fn description(&self) -> &str {
        "List all pending and completed todos in the shared instance-wide list, ordered by stable integer ID. Each item includes id, text, and completed."
    }

    fn input_schema(&self) -> Value {
        list_schema()
    }

    async fn execute(&self, args: Value, ctx: ToolContext) -> Result<ToolOutput, AgentError> {
        list(args, ctx, false).await
    }
}

#[async_trait::async_trait]
impl Tool for TodoListPending {
    fn name(&self) -> &str {
        "todo_list_pending"
    }

    fn description(&self) -> &str {
        "List only pending todos in the shared instance-wide list, ordered by stable integer ID. Each item includes id, text, and completed."
    }

    fn input_schema(&self) -> Value {
        list_schema()
    }

    async fn execute(&self, args: Value, ctx: ToolContext) -> Result<ToolOutput, AgentError> {
        list(args, ctx, true).await
    }
}

#[async_trait::async_trait]
impl Tool for TodoComplete {
    fn name(&self) -> &str {
        "todo_complete"
    }

    fn description(&self) -> &str {
        "Mark a todo in the shared instance-wide list as done using its stable integer ID. Completing an already completed item succeeds."
    }

    fn input_schema(&self) -> Value {
        id_schema()
    }

    async fn execute(&self, args: Value, ctx: ToolContext) -> Result<ToolOutput, AgentError> {
        let args: IdArgs = parse_args(args)?;
        validate_id(args.id)?;
        Ok(match todos::complete_todo(pool(&ctx)?, args.id).await? {
            Some(todo) => todo_output(todo, "Completed"),
            None => not_found(args.id),
        })
    }
}

#[async_trait::async_trait]
impl Tool for TodoUpdate {
    fn name(&self) -> &str {
        "todo_update"
    }

    fn description(&self) -> &str {
        "Update a todo's text in the shared instance-wide list using its stable integer ID. Preserves its ID and pending/completed state."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "id": { "type": "integer", "minimum": 1, "description": "The stable todo ID returned by todo_add or a todo list tool." },
                "text": { "type": "string", "minLength": 1, "description": "The replacement todo description; must not be blank." }
            },
            "required": ["id", "text"],
            "additionalProperties": false
        })
    }

    async fn execute(&self, args: Value, ctx: ToolContext) -> Result<ToolOutput, AgentError> {
        let args: UpdateArgs = parse_args(args)?;
        validate_id(args.id)?;
        validate_text(&args.text)?;
        Ok(
            match todos::update_todo(pool(&ctx)?, args.id, &args.text).await? {
                Some(todo) => todo_output(todo, "Updated"),
                None => not_found(args.id),
            },
        )
    }
}

#[async_trait::async_trait]
impl Tool for TodoDelete {
    fn name(&self) -> &str {
        "todo_delete"
    }

    fn description(&self) -> &str {
        "Permanently delete a todo from the shared instance-wide list using its stable integer ID. Other IDs do not change and deleted IDs are never reused."
    }

    fn input_schema(&self) -> Value {
        id_schema()
    }

    async fn execute(&self, args: Value, ctx: ToolContext) -> Result<ToolOutput, AgentError> {
        let args: IdArgs = parse_args(args)?;
        validate_id(args.id)?;
        if !todos::delete_todo(pool(&ctx)?, args.id).await? {
            return Ok(not_found(args.id));
        }
        Ok(ToolOutput {
            success: true,
            summary: format!("Deleted todo #{}.", args.id),
            data: json!({ "id": args.id }),
        })
    }
}
