//! Context management — bounded prompts, summaries, compaction.
//!
//! Phase 9 implementation.

pub mod budget;
pub mod compaction_service;
pub mod compaction_worker;
pub mod manager;
pub mod summaries;

pub use budget::*;
pub use compaction_service::*;
pub use compaction_worker::*;
pub use manager::*;
pub use summaries::*;
