//! Telegram integration — long polling, commands, messaging, and message routing.

pub mod bot;
pub mod commands;
pub mod handler;
pub mod service;

pub use bot::TelegramBot;
pub use handler::MessageHandler;
pub use service::TelegramService;
