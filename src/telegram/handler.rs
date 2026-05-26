//! Message handler — routes incoming Telegram messages to commands or the agent loop.
//!
//! This is the glue between Telegram ingress and the agent runtime:
//! 1. Check allowlist
//! 2. Find or create chat session
//! 3. Route: command handler vs agent loop
//! 4. Persist incoming and outgoing messages
//! 5. Return response text for the Telegram service to deliver

use std::sync::Arc;

use genai::chat::{ChatMessage, MessageContent};
use sqlx::SqlitePool;
use tracing::{debug, error, info, instrument, warn};

use crate::agent::agent_loop::{AgentContext, AgentLoopConfig, run_agent};
use crate::agent::outcome::AgentOutcome;
use crate::agent::run_mode::AgentRunMode;
use crate::config::AppConfig;
use crate::context::budget::ContextBudget;
use crate::context::compaction_service::CompactionService;
use crate::context::manager::ContextManager;
use crate::error::AgentError;
use crate::llm::LlmExecutor;
use crate::storage;
use crate::tools::registry::ToolRegistry;

use super::commands::{CommandHandler, TelegramCommand};

/// Handles incoming Telegram messages, routing them to the appropriate handler.
pub struct MessageHandler {
    /// Database connection pool for session/message persistence.
    pool: SqlitePool,
    /// LLM executor.
    llm: Arc<dyn LlmExecutor>,
    /// Tool registry.
    registry: Arc<ToolRegistry>,
    /// Agent loop configuration.
    loop_config: AgentLoopConfig,
    /// Context manager for bounded context assembly.
    context_manager: ContextManager,
    /// Compaction service for background context compaction.
    compaction_service: Arc<CompactionService>,
    /// Application configuration.
    config: AppConfig,
    /// Cached Telegram bot token from the environment.
    bot_token: String,
    /// Notifier to wake up the scheduler loop immediately on job updates.
    scheduler_notifier: Option<Arc<tokio::sync::Notify>>,
}

impl MessageHandler {
    pub fn new(
        pool: SqlitePool,
        llm: Arc<dyn LlmExecutor>,
        registry: Arc<ToolRegistry>,
        config: AppConfig,
        scheduler_notifier: Option<Arc<tokio::sync::Notify>>,
        compaction_service: Arc<CompactionService>,
    ) -> Self {
        let loop_config = AgentLoopConfig {
            max_tool_iterations: config.agent.max_tool_iterations,
            llm_model: config.llm.model.clone(),
            llm_temperature: config.llm.temperature,
            llm_max_output_tokens: config.llm.max_output_tokens,
        };

        // Build the context budget and manager
        let budget = ContextBudget {
            context_window_tokens: 128_000,
            reserved_output_tokens: 4_096,
            reserved_tool_loop_tokens: 8_192,
            soft_compaction_threshold: config.context.soft_compaction_threshold,
            hard_context_threshold: config.context.hard_context_threshold,
        };

        let context_manager = ContextManager::new(pool.clone(), budget);

        let bot_token =
            std::env::var(&config.telegram.bot_token_env).unwrap_or_else(|_| String::new());
        Self {
            pool,
            llm,
            registry,
            loop_config,
            context_manager,
            compaction_service,
            config,
            bot_token,
            scheduler_notifier,
        }
    }

    /// Process an incoming Telegram message.
    ///
    /// Returns the text to send back to the chat, or None if no reply is needed.
    #[instrument(skip(self), fields(chat_id = chat_id, user_id = user_id))]
    pub async fn handle_message(
        &self,
        chat_id: i64,
        user_id: i64,
        text: &str,
    ) -> Result<Option<String>, AgentError> {
        // 1. Check allowlist
        self.check_allowlist(chat_id, user_id)?;

        // 2. Find or create session
        let session = self.ensure_session(chat_id).await?;

        // 3. Update session timestamp
        storage::sessions::update_session(&self.pool, &session.id).await?;

        // 4. Route: command or agent loop
        let response = self
            .route_message(chat_id, user_id, text, &session.id)
            .await;

        // 5. Persist the incoming user message after routing so it is not
        // duplicated in ContextManager's current-turn prompt assembly.
        let user_message = ChatMessage::user(MessageContent::from_text(text));
        let _ =
            storage::messages::create_message(&self.pool, &session.id, &user_message, None).await?;

        // 6. Persist response if one was generated
        if let Ok(Some(ref reply_text)) = response {
            let assistant_msg = ChatMessage::assistant(MessageContent::from_text(reply_text));
            let _ =
                storage::messages::create_message(&self.pool, &session.id, &assistant_msg, None)
                    .await;
        }

        response
    }

    /// Check whether a user/chat is allowed.
    fn check_allowlist(&self, chat_id: i64, user_id: i64) -> Result<(), AgentError> {
        let telegram = &self.config.telegram;

        // Check chat allowlist
        if !telegram.allowed_chat_ids.is_empty() && !telegram.allowed_chat_ids.contains(&chat_id) {
            warn!(chat_id, user_id, "rejected message: chat not in allowlist");
            return Err(AgentError::PermissionDenied);
        }

        // Check user allowlist
        if !telegram.allowed_user_ids.is_empty() && !telegram.allowed_user_ids.contains(&user_id) {
            warn!(chat_id, user_id, "rejected message: user not in allowlist");
            return Err(AgentError::PermissionDenied);
        }

        Ok(())
    }

    /// Find or create a chat session for the given chat_id.
    async fn ensure_session(&self, chat_id: i64) -> Result<storage::ChatSession, AgentError> {
        let existing = storage::sessions::get_session_for_chat(&self.pool, chat_id).await?;

        match existing {
            Some(session) => {
                debug!(session_id = %session.id, chat_id, "found existing session");
                Ok(session)
            }
            None => {
                let session = storage::sessions::create_session(&self.pool, chat_id).await?;
                info!(session_id = %session.id, chat_id, "created new session");
                Ok(session)
            }
        }
    }

    /// Route a message: check for commands, otherwise run the agent loop.
    async fn route_message(
        &self,
        chat_id: i64,
        user_id: i64,
        text: &str,
        session_id: &str,
    ) -> Result<Option<String>, AgentError> {
        // Check for bot commands
        if let Some(command) = TelegramCommand::parse(text) {
            debug!(?command, chat_id, "handling bot command");
            let response = CommandHandler::handle(
                command,
                chat_id,
                user_id,
                &self.pool,
                self.scheduler_notifier.as_deref(),
            )
            .await?;
            return Ok(Some(response));
        }

        // Check for /reset-context without slash (heuristic)
        if text.trim().eq_ignore_ascii_case("reset context") {
            let response = CommandHandler::handle(
                TelegramCommand::ResetContext,
                chat_id,
                user_id,
                &self.pool,
                self.scheduler_notifier.as_deref(),
            )
            .await?;
            return Ok(Some(response));
        }

        // Regular message: run through the agent loop
        self.run_agent_for_message(chat_id, user_id, text, session_id)
            .await
    }

    /// Run the agent loop for a regular chat message.
    async fn run_agent_for_message(
        &self,
        chat_id: i64,
        user_id: i64,
        text: &str,
        session_id: &str,
    ) -> Result<Option<String>, AgentError> {
        info!(
            chat_id,
            user_id,
            text_len = text.len(),
            "running agent loop"
        );

        // Load personality file if configured
        let personality = self.load_personality().await?;

        // Build bounded context using ContextManager (summary + recent messages)
        let current_user_message = ChatMessage::user(MessageContent::from_text(text));
        let messages = self
            .context_manager
            .assemble_messages(session_id, &personality, current_user_message)
            .await?;

        // Build agent context
        let ctx = AgentContext {
            run_mode: AgentRunMode::InteractiveReply { chat_id, user_id },
            personality: String::new(), // Already included in messages
            messages,
            workspace_root: self.config.workspace.root.clone(),
            telegram_token: self.get_bot_token(),
            allowed_chat_ids: self.config.telegram.allowed_chat_ids.clone(),
            allowed_user_ids: self.config.telegram.allowed_user_ids.clone(),
            pool: Some(self.pool.clone()),
            session_id: session_id.to_string(),
            scheduler_notifier: self.scheduler_notifier.clone(),
        };

        // Run the agent loop
        let result = run_agent(&ctx, self.llm.as_ref(), &self.registry, &self.loop_config).await;

        // Check if compaction is needed after a successful run
        if let Ok(ref agent_result) = result
            && let AgentOutcome::FinalText(_) | AgentOutcome::Silent = agent_result.outcome
        {
            let total_tokens = agent_result.metadata.token_usage.total_tokens;
            if total_tokens > self.context_manager.soft_threshold_tokens() {
                debug!(
                    chat_id,
                    total_tokens,
                    soft_threshold = self.context_manager.soft_threshold_tokens(),
                    "token usage exceeds soft threshold, checking for compaction"
                );
                if let Err(e) = self.compaction_service.check_session(session_id).await {
                    warn!(chat_id, error = %e, "failed to check compaction");
                }
            }
        }

        match result {
            Ok(agent_result) => match agent_result.outcome {
                AgentOutcome::FinalText(text) => {
                    info!(
                        chat_id,
                        iterations = agent_result.metadata.iterations,
                        text_len = text.len(),
                        "agent completed successfully"
                    );
                    Ok(Some(text))
                }
                AgentOutcome::Silent => {
                    debug!(chat_id, "agent completed silently");
                    Ok(None)
                }
            },
            Err(e) => {
                error!(chat_id, error = %e, "agent loop failed");

                // Provide a user-friendly error message
                let user_msg = match &e {
                    AgentError::MaxToolIterationsExceeded(max) => {
                        format!(
                            "⚠️ I tried to complete the task but it required more steps than allowed (max {max} tool iterations). Please try simplifying your request."
                        )
                    }
                    AgentError::PermissionDenied => "🚫 Access denied.".to_string(),
                    _ => {
                        format!("❌ I encountered an error: {e}")
                    }
                };
                Ok(Some(user_msg))
            }
        }
    }

    /// Load personality from the configured Markdown file.
    async fn load_personality(&self) -> Result<String, AgentError> {
        let path = &self.config.agent.personality_file;

        if !path.exists() {
            // No personality file — use a reasonable default
            return Ok("You are NerdBot, a helpful and concise AI assistant. \
                You respond in plain text. You use tools when they would help \
                answer the user's question more accurately. \
                When you don't know something, you say so honestly."
                .to_string());
        }

        tokio::fs::read_to_string(path)
            .await
            .map_err(|e| AgentError::Config(format!("Failed to read personality file: {e}")))
    }

    /// Get the cached Telegram bot token.
    fn get_bot_token(&self) -> String {
        self.bot_token.clone()
    }
}
