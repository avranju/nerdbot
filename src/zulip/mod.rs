//! Zulip integration — long polling, messaging, and message routing.
//!
//! Provides a Zulip channel adapter following the same patterns as the
//! Telegram adapter: a bot client, channel service, attachment processing,
//! and dual ingress (long polling + webhook).

pub mod attachment;
pub mod bot;
pub mod service;
pub mod update;

pub use bot::ZulipBot;
pub use service::ZulipService;
pub use update::{ZulipHook, ZulipPoll};
