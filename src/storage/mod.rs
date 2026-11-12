//! Storage — SQLite-backed persistence for sessions, messages, jobs, summaries, and todos.
//!
//! Uses sqlx with SQLite for all database operations including schema
//! creation, migrations, CRUD for all entity types.

pub mod jobs;
pub mod messages;
pub mod sessions;
pub mod sqlite;
pub mod summaries;
pub mod todos;

pub use jobs::StoredJob;
pub use messages::StoredMessage;
pub use sessions::ChatSession;
pub use sqlite::Database;
pub use summaries::StoredSummary;
pub use todos::StoredTodo;
