//! Context management — bounded prompts, summaries, compaction.
//!
//! Implementations come in Phase 9.

pub mod budget;
pub mod compaction_service;
pub mod compaction_worker;
pub mod manager;
pub mod summaries;

pub use budget::*;
pub use compaction_service::*;
pub use summaries::*;
