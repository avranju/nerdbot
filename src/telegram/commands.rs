//! Telegram command parsing and handling.
//!
//! Parses bot command strings (e.g. "/help", "/jobs") and provides
//! handlers that return response text. Handler functions are
//! thin wrappers that delegate to the appropriate services.

use crate::error::AgentError;
use sqlx::SqlitePool;
use tracing::debug;

/// Supported Telegram commands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TelegramCommand {
    Start,
    Help,
    Jobs,
    Run(String),
    Delete(String),
    ResetContext,
}

impl TelegramCommand {
    /// Parse a command string (e.g. "/jobs" -> Jobs).
    ///
    /// Returns `None` if the text does not start with a recognized command.
    /// Leading slashes and bot-mention suffixes (e.g. "/help@MyBot") are
    /// stripped before matching.
    pub fn parse(text: &str) -> Option<Self> {
        // Left-trim only — trailing spaces are semantically meaningful
        // for distinguishing "/run" (no arg) from "/run " (empty arg).
        let text = text.trim_start();

        if text.is_empty() {
            return None;
        }

        // Split into command part and argument part.
        // Handle bot mentions: "/run@MyBot abc123" -> command="/run", arg="abc123"
        let (command_with_mention, rest_after_command) = text.split_once(' ').unwrap_or((text, ""));

        // Did the original text have a space after the command?
        let had_space = !rest_after_command.is_empty() || text.ends_with(' ');

        // Strip bot mention suffix from the command part
        let command = command_with_mention
            .split_once('@')
            .map(|(cmd, _)| cmd)
            .unwrap_or(command_with_mention);

        // Trim the argument portion (but don't trim the leading space used for detection)
        let arg = rest_after_command.trim().to_string();

        // Only commands that take arguments require a space separator.
        // "/run" alone is None, "/run " is Some(Run(""))
        match command {
            "/start" => Some(Self::Start),
            "/help" => Some(Self::Help),
            "/jobs" => Some(Self::Jobs),
            "/reset-context" | "/new-topic" => Some(Self::ResetContext),
            "/run" => {
                if had_space {
                    Some(Self::Run(arg))
                } else {
                    None
                }
            }
            "/delete" => {
                if had_space {
                    Some(Self::Delete(arg))
                } else {
                    None
                }
            }
            _ => None,
        }
    }
}

/// Command handler — executes commands and returns response text.
pub struct CommandHandler;

impl CommandHandler {
    /// Handle a parsed command and return the response text to send to the user.
    pub async fn handle(
        command: TelegramCommand,
        chat_id: i64,
        _user_id: i64,
        pool: &SqlitePool,
        scheduler_notifier: Option<&tokio::sync::Notify>,
    ) -> Result<String, AgentError> {
        debug!(?command, chat_id, "handling Telegram command");

        match command {
            TelegramCommand::Start => Ok(Self::start()),
            TelegramCommand::Help => Ok(Self::help()),
            TelegramCommand::Jobs => Self::jobs(chat_id, pool).await,
            TelegramCommand::Run(job_id) => Self::run_job(chat_id, &job_id, pool, scheduler_notifier).await,
            TelegramCommand::Delete(job_id) => Self::delete_job(chat_id, &job_id, pool, scheduler_notifier).await,
            TelegramCommand::ResetContext => {
                crate::storage::sessions::create_session(pool, chat_id).await?;
                Ok(Self::reset_context())
            }
        }
    }

    fn start() -> String {
        "Welcome to NerdBot! 🤖\n\n\
        I'm your self-hosted AI agent assistant. Here's what I can do:\n\n\
        • Answer questions and have conversations\n\
        • Run tools to help with tasks\n\
        • Schedule recurring jobs\n\n\
        Type /help to see all available commands.\n\
        Just send me a message to chat!"
            .to_string()
    }

    fn help() -> String {
        "🤖 **NerdBot Commands**\n\n\
        /start — Introduction and welcome message\n\
        /help — Show this help text\n\
        /jobs — List all your scheduled jobs\n\
        /run <job-id> — Trigger a scheduled job immediately\n\
        /delete <job-id> — Delete a scheduled job\n\
        /reset-context — Start a fresh conversation (forgets current chat context)\n\n\
        You can also just send me a message to chat normally!"
            .to_string()
    }

    async fn jobs(chat_id: i64, pool: &SqlitePool) -> Result<String, AgentError> {
        let jobs = crate::storage::jobs::list_jobs(pool, chat_id, false).await?;

        if jobs.is_empty() {
            return Ok("📋 You have no scheduled jobs.\n\nCreate a job by asking me to schedule something!".to_string());
        }

        let mut response = String::from("📋 **Your Scheduled Jobs**\n\n");
        for job in &jobs {
            let status = if job.enabled {
                "✅ active"
            } else {
                "❌ disabled"
            };
            let next_run = job
                .next_run_at
                .map(|t| t.format("%Y-%m-%d %H:%M UTC").to_string())
                .unwrap_or_else(|| "not scheduled".to_string());

            response.push_str(&format!(
                "• **{}** (`{}`)\n  {}\n  Next: {}\n\n",
                job.name, job.id, status, next_run
            ));
        }
        Ok(response)
    }

    async fn run_job(
        chat_id: i64,
        job_id: &str,
        pool: &SqlitePool,
        scheduler_notifier: Option<&tokio::sync::Notify>,
    ) -> Result<String, AgentError> {
        let job = crate::storage::jobs::get_job(pool, job_id).await?;

        match job {
            None => Ok(format!("❌ No job found with ID `{job_id}`.")),
            Some(job) if job.owner_chat_id != chat_id => {
                Ok("❌ That job belongs to a different chat.".to_string())
            }
            Some(job) if !job.enabled => {
                Ok(format!("❌ Job `{job_id}` is disabled or deleted and cannot be run."))
            }
            Some(_job) => {
                // Update the job to run immediately and ensure it is enabled
                crate::storage::jobs::update_job_next_run(pool, job_id, Some(chrono::Utc::now()), true).await?;
                
                // Wake up the scheduler
                if let Some(notifier) = scheduler_notifier {
                    notifier.notify_one();
                }

                Ok(format!(
                    "⏳ Job `{job_id}` has been scheduled to run immediately."
                ))
            }
        }
    }

    async fn delete_job(
        chat_id: i64,
        job_id: &str,
        pool: &SqlitePool,
        scheduler_notifier: Option<&tokio::sync::Notify>,
    ) -> Result<String, AgentError> {
        let job = crate::storage::jobs::get_job(pool, job_id).await?;

        match job {
            None => Ok(format!("❌ No job found with ID `{job_id}`.")),
            Some(job) if job.owner_chat_id != chat_id => {
                Ok("❌ That job belongs to a different chat.".to_string())
            }
            Some(_job) => {
                crate::storage::jobs::disable_job(pool, job_id).await?;
                
                // Wake up the scheduler to adjust its timer
                if let Some(notifier) = scheduler_notifier {
                    notifier.notify_one();
                }

                Ok(format!("🗑️ Job `{job_id}` has been deleted."))
            }
        }
    }

    fn reset_context() -> String {
        "🧹 Starting a fresh conversation! Your chat context has been reset.\n\n(Previous messages are still stored but won't be considered in future responses.)"
            .to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_start() {
        assert_eq!(
            TelegramCommand::parse("/start"),
            Some(TelegramCommand::Start)
        );
    }

    #[test]
    fn test_parse_help() {
        assert_eq!(TelegramCommand::parse("/help"), Some(TelegramCommand::Help));
    }

    #[test]
    fn test_parse_help_with_mention() {
        assert_eq!(
            TelegramCommand::parse("/help@MyBot"),
            Some(TelegramCommand::Help)
        );
    }

    #[test]
    fn test_parse_jobs() {
        assert_eq!(TelegramCommand::parse("/jobs"), Some(TelegramCommand::Jobs));
    }

    #[test]
    fn test_parse_run() {
        assert_eq!(
            TelegramCommand::parse("/run abc123"),
            Some(TelegramCommand::Run("abc123".into()))
        );
    }

    #[test]
    fn test_parse_run_with_mention() {
        assert_eq!(
            TelegramCommand::parse("/run@MyBot abc123"),
            Some(TelegramCommand::Run("abc123".into()))
        );
    }

    #[test]
    fn test_parse_run_no_arg_is_none() {
        assert_eq!(TelegramCommand::parse("/run"), None);
    }

    #[test]
    fn test_parse_run_mention_no_arg_is_none() {
        assert_eq!(TelegramCommand::parse("/run@MyBot"), None);
    }

    #[test]
    fn test_parse_delete() {
        assert_eq!(
            TelegramCommand::parse("/delete abc123"),
            Some(TelegramCommand::Delete("abc123".into()))
        );
    }

    #[test]
    fn test_parse_delete_no_arg_is_none() {
        assert_eq!(TelegramCommand::parse("/delete"), None);
    }

    #[test]
    fn test_parse_reset_context() {
        assert_eq!(
            TelegramCommand::parse("/reset-context"),
            Some(TelegramCommand::ResetContext)
        );
    }

    #[test]
    fn test_parse_new_topic_alias() {
        assert_eq!(
            TelegramCommand::parse("/new-topic"),
            Some(TelegramCommand::ResetContext)
        );
    }

    #[test]
    fn test_parse_unknown_command() {
        assert_eq!(TelegramCommand::parse("/unknown"), None);
    }

    #[test]
    fn test_parse_non_command() {
        assert_eq!(TelegramCommand::parse("hello world"), None);
    }

    #[test]
    fn test_parse_empty() {
        assert_eq!(TelegramCommand::parse(""), None);
    }

    #[test]
    fn test_start_response() {
        let text = CommandHandler::start();
        assert!(text.contains("Welcome"));
        assert!(text.contains("/help"));
    }

    #[test]
    fn test_help_response() {
        let text = CommandHandler::help();
        assert!(text.contains("/start"));
        assert!(text.contains("/help"));
        assert!(text.contains("/jobs"));
    }

    #[test]
    fn test_reset_context_response() {
        let text = CommandHandler::reset_context();
        assert!(text.contains("fresh conversation"));
    }
}
