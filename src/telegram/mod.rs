//! Telegram integration — long polling, commands, messaging, and message routing.

pub mod attachment;
pub mod bot;
pub mod commands;
pub mod handler;
pub mod markdown;
pub mod service;
pub mod update;

pub use bot::TelegramBot;
pub use service::TelegramService;
pub use update::{TelegramHook, TelegramPoll};
