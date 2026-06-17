//! Tests for channel-aware access policy.
//!
//! Verifies:
//! - ConversationAddressPattern matching (exact, thread wildcard, cross-channel rejection)
//! - ChannelAccessPolicy::is_allowed (empty allowlist, full address match)
//! - Config-derived policy (Telegram config → channel-qualified patterns)
//! - send_user_message enforces full address pattern matching
//! - Scheduler derives policy from job owner channel

use std::path::PathBuf;

use nerdbot::channel::{
    ChannelAccessPolicy, ConversationAddress, ConversationAddressPattern, SenderIdentity,
};
use nerdbot::config::AppConfig;
use nerdbot::tools::messaging::SendUserMessage;
use nerdbot::tools::traits::{Tool, ToolContext};

// ── ConversationAddressPattern matching tests ────────────────────────────

#[test]
fn pattern_matches_exact_channel_and_conversation() {
    let pattern = ConversationAddressPattern::new("telegram", "123");
    let address = ConversationAddress::new("telegram", "123", None);
    assert!(pattern.matches(&address));
}

#[test]
fn pattern_rejects_different_channel() {
    let pattern = ConversationAddressPattern::new("telegram", "123");
    let address = ConversationAddress::new("zulip", "123", None);
    assert!(!pattern.matches(&address));
}

#[test]
fn pattern_rejects_different_conversation() {
    let pattern = ConversationAddressPattern::new("telegram", "123");
    let address = ConversationAddress::new("telegram", "456", None);
    assert!(!pattern.matches(&address));
}

#[test]
fn pattern_with_thread_wildcard_matches_any_thread() {
    // Pattern with thread_id = None matches any thread
    let pattern = ConversationAddressPattern::new("telegram", "123");
    assert!(pattern.matches(&ConversationAddress::new("telegram", "123", None)));
    assert!(pattern.matches(&ConversationAddress::new(
        "telegram",
        "123",
        Some("t1".into())
    )));
    assert!(pattern.matches(&ConversationAddress::new(
        "telegram",
        "123",
        Some("t99".into())
    )));
}

#[test]
fn pattern_with_exact_thread_requires_matching_thread() {
    let pattern = ConversationAddressPattern::with_thread("telegram", "123", "t1".into());
    assert!(pattern.matches(&ConversationAddress::new(
        "telegram",
        "123",
        Some("t1".into())
    )));
    assert!(!pattern.matches(&ConversationAddress::new("telegram", "123", None)));
    assert!(!pattern.matches(&ConversationAddress::new(
        "telegram",
        "123",
        Some("t2".into())
    )));
}

#[test]
fn same_conversation_id_on_different_channel_does_not_match() {
    let pattern = ConversationAddressPattern::new("telegram", "123");
    // Same conversation ID but different channel
    assert!(!pattern.matches(&ConversationAddress::new("zulip", "123", None)));
    assert!(!pattern.matches(&ConversationAddress::new("slack", "123", None)));
}

// ── ChannelAccessPolicy tests ───────────────────────────────────────────

#[test]
fn empty_policy_allows_any_address_and_sender() {
    let policy = ChannelAccessPolicy::allow_all();
    let address = ConversationAddress::new("any", "any", None);
    let sender = SenderIdentity::new("any", None);
    assert!(policy.is_allowed(&address, &sender));
}

#[test]
fn policy_allows_matching_address() {
    let policy = ChannelAccessPolicy {
        allowed_conversations: vec![ConversationAddressPattern::new("telegram", "123")],
        allowed_senders: vec![],
    };
    let address = ConversationAddress::new("telegram", "123", None);
    let sender = SenderIdentity::new("any", None);
    assert!(policy.is_allowed(&address, &sender));
}

#[test]
fn policy_rejects_non_matching_address() {
    let policy = ChannelAccessPolicy {
        allowed_conversations: vec![ConversationAddressPattern::new("telegram", "123")],
        allowed_senders: vec![],
    };
    let address = ConversationAddress::new("telegram", "456", None);
    let sender = SenderIdentity::new("any", None);
    assert!(!policy.is_allowed(&address, &sender));
}

#[test]
fn policy_allows_matching_sender() {
    let policy = ChannelAccessPolicy {
        allowed_conversations: vec![],
        allowed_senders: vec!["user123".to_string()],
    };
    let address = ConversationAddress::new("any", "any", None);
    let sender = SenderIdentity::new("user123", None);
    assert!(policy.is_allowed(&address, &sender));
}

#[test]
fn policy_rejects_non_matching_sender() {
    let policy = ChannelAccessPolicy {
        allowed_conversations: vec![],
        allowed_senders: vec!["user123".to_string()],
    };
    let address = ConversationAddress::new("any", "any", None);
    let sender = SenderIdentity::new("user456", None);
    assert!(!policy.is_allowed(&address, &sender));
}

#[test]
fn policy_requires_both_address_and_sender() {
    let policy = ChannelAccessPolicy {
        allowed_conversations: vec![ConversationAddressPattern::new("telegram", "123")],
        allowed_senders: vec!["user123".to_string()],
    };
    let address = ConversationAddress::new("telegram", "123", None);
    let sender = SenderIdentity::new("user456", None);
    assert!(!policy.is_allowed(&address, &sender));
}

#[test]
fn policy_allows_thread_wildcard_with_specific_thread() {
    let policy = ChannelAccessPolicy {
        allowed_conversations: vec![ConversationAddressPattern::new("telegram", "123")],
        allowed_senders: vec![],
    };
    let address = ConversationAddress::new("telegram", "123", Some("topic-a".into()));
    let sender = SenderIdentity::new("any", None);
    assert!(policy.is_allowed(&address, &sender));
}

#[test]
fn policy_rejects_thread_mismatch_when_pattern_is_specific() {
    let policy = ChannelAccessPolicy {
        allowed_conversations: vec![ConversationAddressPattern::with_thread(
            "telegram",
            "123",
            "topic-a".into(),
        )],
        allowed_senders: vec![],
    };
    let address = ConversationAddress::new("telegram", "123", Some("topic-b".into()));
    let sender = SenderIdentity::new("any", None);
    assert!(!policy.is_allowed(&address, &sender));
}

// ── Config policy lookup tests ──────────────────────────────────────────

#[test]
fn telegram_config_derives_channel_qualified_policy() {
    let mut config = AppConfig::default();
    config.channels.telegram.allowed_conversations = vec!["111".to_string(), "222".to_string()];
    config.channels.telegram.allowed_senders = vec!["333".to_string()];

    let policy = config.channels.access_policy_for("telegram");

    assert_eq!(policy.allowed_conversations.len(), 2);
    assert_eq!(policy.allowed_conversations[0].channel_id, "telegram");
    assert_eq!(policy.allowed_conversations[0].conversation_id, "111");
    assert_eq!(policy.allowed_conversations[1].conversation_id, "222");
    assert_eq!(policy.allowed_senders, vec!["333"]);
}

#[test]
fn unknown_channel_returns_empty_policy() {
    let config = AppConfig::default();
    let policy = config.channels.access_policy_for("zulip");
    assert!(policy.allowed_conversations.is_empty());
    assert!(policy.allowed_senders.is_empty());
}

#[test]
fn derived_policy_allows_telegram_conversations() {
    let mut config = AppConfig::default();
    config.channels.telegram.allowed_conversations = vec!["123".to_string()];

    let policy = config.channels.access_policy_for("telegram");
    let address = ConversationAddress::new("telegram", "123", None);
    let sender = SenderIdentity::new("any", None);
    assert!(policy.is_allowed(&address, &sender));
}

#[test]
fn derived_policy_rejects_non_telegram_channel() {
    let config = AppConfig::default();
    let policy = config.channels.access_policy_for("zulip");
    let address = ConversationAddress::new("zulip", "123", None);
    let sender = SenderIdentity::new("any", None);
    // Empty policy allows everything (no restrictions for unknown channels)
    assert!(policy.is_allowed(&address, &sender));
}

// ── send_user_message full-address enforcement tests ────────────────────

#[tokio::test]
async fn send_user_message_allows_current_conversation_when_policy_matches() {
    let tool = SendUserMessage;
    let ctx = ToolContext {
        run_mode: nerdbot::agent::run_mode::AgentRunMode::InteractiveReply {
            address: ConversationAddress::new("telegram", "123", None),
            sender: SenderIdentity::new("user1", None),
        },
        workspace_root: PathBuf::from("/tmp"),
        access_policy: ChannelAccessPolicy {
            allowed_conversations: vec![ConversationAddressPattern::new("telegram", "123")],
            allowed_senders: vec![],
        },
        channel_registry: None,
        pool: None,
        scheduler_notifier: None,
    };

    let result = Tool::execute(&tool, serde_json::json!({"text": "hello"}), ctx).await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn send_user_message_rejects_same_conversation_id_on_wrong_channel() {
    let tool = SendUserMessage;
    let ctx = ToolContext {
        run_mode: nerdbot::agent::run_mode::AgentRunMode::InteractiveReply {
            address: ConversationAddress::new("zulip", "123", None),
            sender: SenderIdentity::new("user1", None),
        },
        workspace_root: PathBuf::from("/tmp"),
        access_policy: ChannelAccessPolicy {
            allowed_conversations: vec![ConversationAddressPattern::new("telegram", "123")],
            allowed_senders: vec![],
        },
        channel_registry: None,
        pool: None,
        scheduler_notifier: None,
    };

    // zulip/123 should be rejected even though conversation_id "123" matches
    let result = Tool::execute(&tool, serde_json::json!({"text": "hello"}), ctx).await;
    assert!(result.is_err());
    assert!(matches!(
        result.unwrap_err(),
        nerdbot::error::AgentError::PermissionDenied
    ));
}

#[tokio::test]
async fn send_user_message_explicit_conversation_checked_by_full_address() {
    let tool = SendUserMessage;
    let ctx = ToolContext {
        run_mode: nerdbot::agent::run_mode::AgentRunMode::InteractiveReply {
            address: ConversationAddress::new("telegram", "111", None),
            sender: SenderIdentity::new("user1", None),
        },
        workspace_root: PathBuf::from("/tmp"),
        access_policy: ChannelAccessPolicy {
            allowed_conversations: vec![ConversationAddressPattern::new("telegram", "222")],
            allowed_senders: vec![],
        },
        channel_registry: None,
        pool: None,
        scheduler_notifier: None,
    };

    // Explicit conversation telegram/222 is allowed
    let result = Tool::execute(
        &tool,
        serde_json::json!({
            "text": "hello",
            "conversation": {
                "channel_id": "telegram",
                "conversation_id": "222"
            }
        }),
        ctx,
    )
    .await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn send_user_message_rejects_non_matching_explicit_conversation() {
    let tool = SendUserMessage;
    let ctx = ToolContext {
        run_mode: nerdbot::agent::run_mode::AgentRunMode::InteractiveReply {
            address: ConversationAddress::new("telegram", "111", None),
            sender: SenderIdentity::new("user1", None),
        },
        workspace_root: PathBuf::from("/tmp"),
        access_policy: ChannelAccessPolicy {
            allowed_conversations: vec![ConversationAddressPattern::new("telegram", "222")],
            allowed_senders: vec![],
        },
        channel_registry: None,
        pool: None,
        scheduler_notifier: None,
    };

    // Explicit conversation telegram/333 is not allowed
    let result = Tool::execute(
        &tool,
        serde_json::json!({
            "text": "hello",
            "conversation": {
                "channel_id": "telegram",
                "conversation_id": "333"
            }
        }),
        ctx,
    )
    .await;
    assert!(result.is_err());
    assert!(matches!(
        result.unwrap_err(),
        nerdbot::error::AgentError::PermissionDenied
    ));
}

#[tokio::test]
async fn send_user_message_omitted_conversation_uses_context_address() {
    let tool = SendUserMessage;
    let ctx = ToolContext {
        run_mode: nerdbot::agent::run_mode::AgentRunMode::InteractiveReply {
            address: ConversationAddress::new("telegram", "123", None),
            sender: SenderIdentity::new("user1", None),
        },
        workspace_root: PathBuf::from("/tmp"),
        access_policy: ChannelAccessPolicy {
            allowed_conversations: vec![ConversationAddressPattern::new("telegram", "123")],
            allowed_senders: vec![],
        },
        channel_registry: None,
        pool: None,
        scheduler_notifier: None,
    };

    // No explicit conversation — uses context address which is allowed
    let result = Tool::execute(&tool, serde_json::json!({"text": "hello"}), ctx).await;
    assert!(result.is_ok());
}

// ── Scheduler policy derivation tests ───────────────────────────────────

#[test]
fn scheduler_telegram_job_gets_telegram_policy() {
    let mut config = AppConfig::default();
    config.channels.telegram.allowed_conversations = vec!["123".to_string()];

    // Simulate what the scheduler does: derive policy from job's owner channel
    let owner_address = ConversationAddress::new("telegram", "123", None);
    let policy = config.channels.access_policy_for(&owner_address.channel_id);

    assert!(policy.is_allowed(&owner_address, &SenderIdentity::new("any", None)));
}

#[test]
fn scheduler_non_telegram_job_gets_default_policy() {
    let config = AppConfig::default();

    // A Zulip job with the same conversation ID as Telegram allowlist
    let owner_address = ConversationAddress::new("zulip", "123", None);
    let policy = config.channels.access_policy_for(&owner_address.channel_id);

    // Zulip gets an empty policy (allow-all) — this is correct because
    // the Telegram allowlist should not apply to Zulip jobs
    assert!(policy.is_allowed(&owner_address, &SenderIdentity::new("any", None)));
}

#[test]
fn scheduler_telegram_job_with_wrong_conversation_rejected() {
    let mut config = AppConfig::default();
    config.channels.telegram.allowed_conversations = vec!["123".to_string()];

    // A Telegram job for a different conversation
    let owner_address = ConversationAddress::new("telegram", "456", None);
    let policy = config.channels.access_policy_for(&owner_address.channel_id);

    assert!(!policy.is_allowed(&owner_address, &SenderIdentity::new("any", None)));
}
