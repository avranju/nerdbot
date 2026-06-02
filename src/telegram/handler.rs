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
use crate::agent::system_prompt::append_timezone_context;
use crate::config::AppConfig;
use crate::context::budget::ContextBudget;
use crate::context::compaction_service::CompactionService;
use crate::context::manager::ContextManager;
use crate::error::AgentError;
use crate::llm::LlmExecutor;
use crate::storage;
use crate::tools::registry::ToolRegistry;

use super::commands::{CommandHandler, TelegramCommand};
use super::service::TelegramService;

/// A structured inbound Telegram message with optional attachments.
#[derive(Debug)]
pub struct InboundMessage {
    /// Plain text content (caption or message text).
    pub text: String,
    /// Attachment metadata for persistence markers.
    pub attachments: Vec<AttachmentInfo>,
    /// Pre-processed attachment ContentParts (binary or text) for the LLM.
    pub attachment_parts: Vec<genai::chat::ContentPart>,
}

/// Metadata about an attachment for persistence and context budgeting.
#[derive(Debug, Clone)]
pub struct AttachmentInfo {
    /// Display name (sanitized filename or MIME type).
    pub display_name: String,
    /// MIME type.
    pub mime_type: String,
    /// Size in bytes.
    pub size_bytes: u64,
    /// Whether the attachment was downloaded for the LLM.
    pub downloaded: bool,
    /// Compact marker persisted in conversation history for this attachment.
    pub persistence_marker: String,
    /// Extracted text content for text documents (bounded by max_text_chars).
    /// Populated for text-type attachments so that follow-up questions can
    /// reference the document content even though the full text was not
    /// persisted in the current-turn message.
    pub extracted_text: Option<String>,
}

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
    /// Service used to show a typing action during interactive agent turns.
    telegram_service: Option<TelegramService>,
}

impl MessageHandler {
    pub fn new(
        pool: SqlitePool,
        llm: Arc<dyn LlmExecutor>,
        registry: Arc<ToolRegistry>,
        config: AppConfig,
        scheduler_notifier: Option<Arc<tokio::sync::Notify>>,
        telegram_service: Option<TelegramService>,
        compaction_service: Arc<CompactionService>,
    ) -> Self {
        let loop_config = AgentLoopConfig {
            max_tool_iterations: config.agent.max_tool_iterations,
            llm_model: config.llm.model.clone(),
            llm_temperature: config.llm.temperature,
            llm_max_output_tokens: config.llm.max_output_tokens,
        };

        // Build the context budget and manager
        let budget = ContextBudget::from_llm_and_context(&config.llm, &config.context);

        let context_manager = ContextManager::from_config(pool.clone(), budget, &config.context);

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
            telegram_service,
        }
    }

    /// Process an incoming Telegram message.
    ///
    /// Accepts structured messages with optional attachments. Returns the text
    /// to send back to the chat, or None if no reply is needed.
    #[instrument(skip(self), fields(chat_id = chat_id, user_id = user_id))]
    pub async fn handle_message(
        &self,
        chat_id: i64,
        user_id: i64,
        text: &str,
    ) -> Result<Option<String>, AgentError> {
        self.handle_rich_message(
            chat_id,
            user_id,
            &InboundMessage {
                text: text.to_string(),
                attachment_parts: Vec::new(),
                attachments: Vec::new(),
            },
        )
        .await
    }

    /// Process a rich inbound Telegram message with optional attachments.
    ///
    /// Returns the text to send back to the chat, or None if no reply is needed.
    #[instrument(
        skip(self, inbound),
        fields(
            chat_id = chat_id,
            user_id = user_id,
            text_len = inbound.text.len(),
            attachment_count = inbound.attachments.len(),
            attachment_part_count = inbound.attachment_parts.len()
        )
    )]
    pub async fn handle_rich_message(
        &self,
        chat_id: i64,
        user_id: i64,
        inbound: &InboundMessage,
    ) -> Result<Option<String>, AgentError> {
        // 1. Check allowlist BEFORE downloading any files
        self.check_allowlist(chat_id, user_id)?;

        // 2. Find or create session
        let session = self.ensure_session(chat_id).await?;

        // 3. Update session timestamp
        storage::sessions::update_session(&self.pool, &session.id).await?;

        // 4. Route: command or agent loop
        let response = self
            .route_rich_message(chat_id, user_id, inbound, &session.id)
            .await;

        // 5. Persist the incoming user message after routing so it is not
        // duplicated in ContextManager's current-turn prompt assembly.
        // Prepend attachment markers for auditability and context budgeting.
        let persist_text = build_persist_text(inbound);
        let user_message = ChatMessage::user(MessageContent::from_text(&persist_text));
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

        // Regular message: run through the agent loop
        self.run_agent_for_message(chat_id, user_id, text, session_id)
            .await
    }

    /// Route a rich message: check for commands, otherwise run the agent loop
    /// with attachment-aware context assembly.
    async fn route_rich_message(
        &self,
        chat_id: i64,
        user_id: i64,
        inbound: &InboundMessage,
        session_id: &str,
    ) -> Result<Option<String>, AgentError> {
        // Check for bot commands (commands take priority over attachments)
        if TelegramCommand::parse(&inbound.text).is_some() {
            // Fall back to plain text routing for commands
            return self
                .route_message(chat_id, user_id, &inbound.text, session_id)
                .await;
        }

        // Regular message: run through the agent loop with attachment support
        self.run_agent_for_rich_message(chat_id, user_id, inbound, session_id)
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
        let typing_indicator = self
            .telegram_service
            .as_ref()
            .map(|service| service.start_typing(chat_id));

        // Load personality file if configured
        let personality = append_timezone_context(
            &self.load_personality().await?,
            &self.config.agent.default_timezone,
        );

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
        drop(typing_indicator);

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

    /// Run the agent loop for a rich inbound message with attachments.
    ///
    /// Processes attachments (photos, documents) into LLM content parts,
    /// builds a multimodal user message, and runs the agent loop.
    async fn run_agent_for_rich_message(
        &self,
        chat_id: i64,
        user_id: i64,
        inbound: &InboundMessage,
        session_id: &str,
    ) -> Result<Option<String>, AgentError> {
        info!(
            chat_id,
            user_id,
            text_len = inbound.text.len(),
            attachment_count = inbound.attachments.len(),
            "running agent loop with attachments"
        );
        let typing_indicator = self
            .telegram_service
            .as_ref()
            .map(|service| service.start_typing(chat_id));

        // Load personality file if configured
        let personality = append_timezone_context(
            &self.load_personality().await?,
            &self.config.agent.default_timezone,
        );

        // Build the user message with attachment content parts via assemble_rich_user_message.
        let current_user_message = self.assemble_rich_user_message(inbound).await?;

        let messages = self
            .context_manager
            .assemble_messages(session_id, &personality, current_user_message)
            .await?;

        let ctx = AgentContext {
            run_mode: AgentRunMode::InteractiveReply { chat_id, user_id },
            personality: String::new(),
            messages,
            workspace_root: self.config.workspace.root.clone(),
            telegram_token: self.get_bot_token(),
            allowed_chat_ids: self.config.telegram.allowed_chat_ids.clone(),
            allowed_user_ids: self.config.telegram.allowed_user_ids.clone(),
            pool: Some(self.pool.clone()),
            session_id: session_id.to_string(),
            scheduler_notifier: self.scheduler_notifier.clone(),
        };

        let result = run_agent(&ctx, self.llm.as_ref(), &self.registry, &self.loop_config).await;
        drop(typing_indicator);

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

    /// Assemble a user message from an inbound message, processing attachments.
    ///
    /// For text-only messages, returns a simple text ContentPart.
    /// For messages with attachments, builds a multimodal message with
    /// ContentPart::Text (caption) + ContentPart::Binary (downloaded files).
    async fn assemble_rich_user_message(
        &self,
        inbound: &InboundMessage,
    ) -> Result<ChatMessage, AgentError> {
        let mut parts = Vec::new();

        // Add caption/text as text part
        if !inbound.text.is_empty() {
            parts.push(genai::chat::ContentPart::Text(inbound.text.clone()));
        }

        // Add attachment parts (already processed into ContentParts by main.rs)
        for part in &inbound.attachment_parts {
            parts.push(part.clone());
        }

        // If no parts at all, use a default prompt
        if parts.is_empty() {
            parts.push(genai::chat::ContentPart::Text(
                "Please analyze the attached file(s).".to_string(),
            ));
        }

        Ok(ChatMessage::user(MessageContent::from_parts(parts)))
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

/// Build the persisted text for an inbound message, prepending attachment markers.
///
/// Attachment markers like `[image: report.pdf]` are added so that
/// message history reflects what was actually received, even when
/// binary payloads are sent to the LLM as base64 ContentParts.
///
/// For text documents (including JSON, XML, JavaScript, shell scripts,
/// Python, and other supported formats), the extracted text is also
/// included so that follow-up questions can reference the document content.
pub fn build_persist_text(inbound: &InboundMessage) -> String {
    let mut parts = Vec::new();

    // Prepend attachment markers and extracted text for text documents.
    if !inbound.attachments.is_empty() {
        for a in &inbound.attachments {
            let mime_cat = a.mime_type.split('/').next().unwrap_or("file");
            parts.push(a.persistence_marker.clone());
            // Extracted text has already been bounded by the attachment
            // processor using the configured max_text_document_chars limit.
            if let Some(ref text) = a.extracted_text {
                parts.push(format!(
                    "--- {mime_cat}: {} (extracted text) ---\n{text}",
                    a.display_name,
                ));
            }
        }
    }

    // Append the caption/text
    if !inbound.text.is_empty() {
        parts.push(inbound.text.clone());
    }

    parts.join("\n")
}
