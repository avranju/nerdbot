//! Telegram integration — long polling, commands, messaging.
//!
//! Implementations come in Phase 4.

pub mod bot;
pub mod commands;
pub mod service;

pub use commands::*;
