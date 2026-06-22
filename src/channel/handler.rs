//! Channel-agnostic message handler.

use std::sync::Arc;

use genai::chat::{ChatMessage, MessageContent};
use sqlx::SqlitePool;
use tracing::{debug, error, info, instrument, warn};

use crate::agent::agent_loop::{AgentContext, AgentLoopConfig, run_agent};
use crate::agent::outcome::AgentOutcome;
use crate::agent::personality::Personality;
use crate::agent::run_mode::AgentRunMode;
use crate::channel::{
    ChannelRegistry, ConversationAddress, InboundMessage, OutboundMessage, SenderIdentity,
};
use crate::config::AppConfig;
use crate::context::budget::ContextBudget;
use crate::context::compaction_service::CompactionService;
use crate::context::manager::ContextManager;
use crate::error::AgentError;
use crate::llm::LlmExecutor;
use crate::storage;
use crate::telegram::commands::{CommandHandler, TelegramCommand};
use crate::tools::registry::ToolRegistry;

pub struct ChannelMessageHandler {
    pool: SqlitePool,
    llm: Arc<dyn LlmExecutor>,
    registry: Arc<ToolRegistry>,
    channel_registry: Arc<ChannelRegistry>,
    loop_config: AgentLoopConfig,
    context_manager: ContextManager,
    compaction_service: Arc<CompactionService>,
    config: AppConfig,
    personality: Personality,
    scheduler_notifier: Option<Arc<tokio::sync::Notify>>,
}

pub struct ChannelMessageHandlerInput {
    pub pool: SqlitePool,
    pub llm: Arc<dyn LlmExecutor>,
    pub registry: Arc<ToolRegistry>,
    pub channel_registry: Arc<ChannelRegistry>,
    pub config: AppConfig,
    pub personality: Personality,
    pub scheduler_notifier: Option<Arc<tokio::sync::Notify>>,
    pub compaction_service: Arc<CompactionService>,
}

impl ChannelMessageHandler {
    pub fn new(input: ChannelMessageHandlerInput) -> Self {
        let loop_config = AgentLoopConfig {
            max_tool_iterations: input.config.agent.max_tool_iterations,
            llm_model: input.config.llm.model.clone(),
            llm_temperature: input.config.llm.temperature,
            llm_max_output_tokens: input.config.llm.max_output_tokens,
        };
        let budget = ContextBudget::from_llm_and_context(&input.config.llm, &input.config.context);
        let context_manager =
            ContextManager::from_config(input.pool.clone(), budget, &input.config.context);

        Self {
            pool: input.pool,
            llm: input.llm,
            registry: input.registry,
            channel_registry: input.channel_registry,
            loop_config,
            context_manager,
            compaction_service: input.compaction_service,
            config: input.config,
            personality: input.personality,
            scheduler_notifier: input.scheduler_notifier,
        }
    }

    /// Return the maintenance response text when maintenance mode is active.
    pub fn maintenance_response_text(&self) -> Option<String> {
        if self.config.maintenance.enabled {
            Some(self.config.maintenance.message())
        } else {
            None
        }
    }

    #[instrument(
        skip(self, inbound),
        fields(
            channel_id = %address.channel_id,
            conversation_id = %address.conversation_id,
            thread_id = ?address.thread_id,
            sender_id = %sender.sender_id,
            text_len = inbound.text.len(),
            attachment_count = inbound.attachments.len(),
            attachment_part_count = inbound.attachment_parts.len()
        )
    )]
    pub async fn handle_rich_message(
        &self,
        address: &ConversationAddress,
        sender: &SenderIdentity,
        inbound: &InboundMessage,
    ) -> Result<Option<String>, AgentError> {
        self.check_allowlist(address, sender)?;

        // Short-circuit with maintenance message when maintenance mode is active.
        if self.config.maintenance.enabled {
            let maintenance_text = self.config.maintenance.message();
            debug!(
                ?address,
                ?sender,
                "maintenance mode active — returning maintenance response"
            );
            return Ok(Some(maintenance_text));
        }

        let session = self.ensure_session(address).await?;
        storage::sessions::update_session(&self.pool, &session.id).await?;

        let response = self
            .route_rich_message(address, sender, inbound, &session.id)
            .await;

        let persist_text = build_persist_text(inbound);
        let user_message = ChatMessage::user(MessageContent::from_text(&persist_text));
        let _ =
            storage::messages::create_message(&self.pool, &session.id, &user_message, None).await?;

        if let Ok(Some(ref reply_text)) = response {
            let assistant_msg = ChatMessage::assistant(MessageContent::from_text(reply_text));
            let _ =
                storage::messages::create_message(&self.pool, &session.id, &assistant_msg, None)
                    .await;
        }

        response
    }

    pub async fn handle_event(
        &self,
        event: crate::channel::ChannelInboundEvent,
    ) -> Result<(), AgentError> {
        match self
            .handle_rich_message(&event.address, &event.sender, &event.message)
            .await
        {
            Ok(Some(response)) => {
                self.channel_registry
                    .send_message(&event.address, OutboundMessage::markdown(response))
                    .await
            }
            Ok(None) => Ok(()),
            Err(e) => Err(e),
        }
    }

    /// Handle a Zulip message with two-phase processing: resolve address, check access policy,
    /// then download and process attachments. This ensures unauthorized senders never trigger
    /// attachment downloads.
    #[instrument(
        skip(self, msg, bot),
        fields(
            message_id = msg.id,
            sender = msg.sender_email,
            channel_id = "zulip",
        )
    )]
    pub async fn handle_zulip_message(
        &self,
        msg: &crate::zulip::bot::ZulipMessage,
        bot: &crate::zulip::bot::ZulipBot,
        bot_email: &str,
        max_attachment_bytes: usize,
        max_text_document_chars: usize,
    ) -> Result<(), AgentError> {
        let Some(content) = crate::zulip::update::addressed_zulip_content(msg, bot) else {
            debug!(
                message_id = msg.id,
                message_type = msg.message_type,
                "skipping Zulip message not addressed to bot"
            );
            return Ok(());
        };

        // Phase 1: Resolve address and sender without downloading attachments
        let (address, sender_email, sender_full_name, _) =
            crate::zulip::update::resolve_zulip_message_for_user(msg, bot_email, bot.user_id());
        if address.thread_id.is_none() {
            bot.cache_typing_recipient_ids(
                &address.conversation_id,
                crate::zulip::update::resolve_zulip_private_recipient_ids_for_user(
                    msg,
                    bot_email,
                    bot.user_id(),
                ),
            );
        }

        let sender = SenderIdentity::new(sender_email, Some(sender_full_name));

        // Phase 2: Check access policy BEFORE downloading attachments
        self.check_allowlist(&address, &sender)?;

        // Short-circuit with maintenance message when maintenance mode is active.
        if self.config.maintenance.enabled {
            let maintenance_text = self.config.maintenance.message();
            debug!(
                message_id = msg.id,
                "maintenance mode active — returning maintenance response"
            );
            self.channel_registry
                .send_message(&address, OutboundMessage::markdown(maintenance_text))
                .await?;
            return Ok(());
        }

        // Phase 3: Download and process attachments (only if access is allowed)
        let (clean_text, attachment_parts, attachments) =
            crate::zulip::attachment::process_inbound_attachments(
                &content,
                bot,
                max_attachment_bytes,
                max_text_document_chars,
            )
            .await?;

        let inbound = InboundMessage {
            text: clean_text,
            attachments,
            attachment_parts,
        };

        match self.handle_rich_message(&address, &sender, &inbound).await {
            Ok(Some(response)) => {
                self.channel_registry
                    .send_message(&address, OutboundMessage::markdown(response))
                    .await
            }
            Ok(None) => Ok(()),
            Err(e) => Err(e),
        }
    }

    fn check_allowlist(
        &self,
        address: &ConversationAddress,
        sender: &SenderIdentity,
    ) -> Result<(), AgentError> {
        let policy = self.config.channels.access_policy_for(&address.channel_id);

        if !policy.is_allowed(address, sender) {
            warn!(?address, ?sender, "rejected message: not in access policy");
            return Err(AgentError::PermissionDenied);
        }

        Ok(())
    }

    async fn ensure_session(
        &self,
        address: &ConversationAddress,
    ) -> Result<storage::ChatSession, AgentError> {
        let existing = storage::sessions::get_session_for_address(&self.pool, address).await?;

        match existing {
            Some(session) => {
                debug!(session_id = %session.id, ?address, "found existing session");
                Ok(session)
            }
            None => {
                let session = storage::sessions::create_session(&self.pool, address).await?;
                info!(session_id = %session.id, ?address, "created new session");
                Ok(session)
            }
        }
    }

    async fn route_rich_message(
        &self,
        address: &ConversationAddress,
        sender: &SenderIdentity,
        inbound: &InboundMessage,
        session_id: &str,
    ) -> Result<Option<String>, AgentError> {
        if let Some(command) = TelegramCommand::parse(&inbound.text) {
            debug!(?command, ?address, "handling channel command");
            let response = CommandHandler::handle(
                command,
                address,
                &self.pool,
                self.scheduler_notifier.as_deref(),
            )
            .await?;
            return Ok(Some(response));
        }

        self.run_agent_for_rich_message(address, sender, inbound, session_id)
            .await
    }

    async fn run_agent_for_rich_message(
        &self,
        address: &ConversationAddress,
        sender: &SenderIdentity,
        inbound: &InboundMessage,
        session_id: &str,
    ) -> Result<Option<String>, AgentError> {
        info!(
            ?address,
            ?sender,
            text_len = inbound.text.len(),
            attachment_count = inbound.attachments.len(),
            "running agent loop with channel message"
        );
        let typing_indicator = self.channel_registry.start_typing(address);

        let personality = self
            .personality
            .effective_prompt(&self.config.agent.default_timezone);

        let current_user_message = assemble_rich_user_message(inbound);
        let messages = self
            .context_manager
            .assemble_messages(
                session_id,
                &personality,
                current_user_message,
                &self.config.agent.default_timezone,
            )
            .await?;

        let ctx = AgentContext {
            run_mode: AgentRunMode::InteractiveReply {
                address: address.clone(),
                sender: sender.clone(),
            },
            personality: String::new(),
            messages,
            workspace_root: self.config.workspace.root.clone(),
            access_policy: self.config.channels.access_policy_for(&address.channel_id),
            channel_registry: Some(self.channel_registry.clone()),
            pool: Some(self.pool.clone()),
            session_id: session_id.to_string(),
            scheduler_notifier: self.scheduler_notifier.clone(),
        };

        let result = run_agent(&ctx, self.llm.as_ref(), &self.registry, &self.loop_config).await;
        drop(typing_indicator);

        if let Ok(ref agent_result) = result
            && let AgentOutcome::FinalText(_) | AgentOutcome::Silent = agent_result.outcome
        {
            let total_tokens = agent_result.metadata.token_usage.total_tokens;
            if total_tokens > self.context_manager.soft_threshold_tokens()
                && let Err(e) = self.compaction_service.check_session(session_id).await
            {
                warn!(?address, error = %e, "failed to check compaction");
            }
        }

        match result {
            Ok(agent_result) => match agent_result.outcome {
                AgentOutcome::FinalText(text) => Ok(Some(text)),
                AgentOutcome::Silent => Ok(None),
            },
            Err(e) => {
                error!(?address, error = %e, "agent loop failed");
                let user_msg = match &e {
                    AgentError::MaxToolIterationsExceeded(max) => {
                        format!(
                            "⚠️ I tried to complete the task but it required more steps than allowed (max {max} tool iterations). Please try simplifying your request."
                        )
                    }
                    AgentError::PermissionDenied => "🚫 Access denied.".to_string(),
                    _ => format!("❌ I encountered an error: {e}"),
                };
                Ok(Some(user_msg))
            }
        }
    }
}

fn assemble_rich_user_message(inbound: &InboundMessage) -> ChatMessage {
    let mut parts = Vec::new();
    if !inbound.text.is_empty() {
        parts.push(genai::chat::ContentPart::Text(inbound.text.clone()));
    }
    for part in &inbound.attachment_parts {
        parts.push(part.clone());
    }
    if parts.is_empty() {
        parts.push(genai::chat::ContentPart::Text(
            "Please analyze the attached file(s).".to_string(),
        ));
    }
    ChatMessage::user(MessageContent::from_parts(parts))
}

pub fn build_persist_text(inbound: &InboundMessage) -> String {
    let mut parts = Vec::new();

    if !inbound.attachments.is_empty() {
        for a in &inbound.attachments {
            let mime_cat = a.mime_type.split('/').next().unwrap_or("file");
            parts.push(a.persistence_marker.clone());
            if let Some(ref text) = a.extracted_text {
                parts.push(format!(
                    "--- {mime_cat}: {} (extracted text) ---\n{text}",
                    a.display_name,
                ));
            }
        }
    }

    if !inbound.text.is_empty() {
        parts.push(inbound.text.clone());
    }

    parts.join("\n")
}
