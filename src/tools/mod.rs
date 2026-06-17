//! Tool system — trait, registry, and built-in tool implementations.

pub mod calculator;
pub mod echo;
pub mod registry;
pub mod shell;
pub mod traits;

// Re-export for external access
pub use shell::BwrapPolicy;
pub use shell::bwrap_available;

// Built-in tool modules
pub mod files;
pub mod messaging;
pub mod schedule;
pub mod web;
