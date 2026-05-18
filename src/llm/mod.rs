//! LLM provider layer — provider-neutral abstractions.

pub mod fake;
pub mod provider;
pub mod types;

// Re-export all public types for convenient access
pub use types::*;
