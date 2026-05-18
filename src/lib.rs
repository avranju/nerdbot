//! NerdBot — a minimal, self-hosted AI agent runtime.
//!
//! This crate provides the core types, traits, and abstractions for the agent.

// Phase 1: suppress unused/dead code warnings for stub implementations.
// TODO: Remove this allow once all modules are fully implemented (target Phase 5+).
#![allow(
    dead_code,
    unused,
    unused_imports,
    unused_variables,
    unused_assignments
)]

// Re-export all modules for integration tests
pub mod agent;
pub mod config;
pub mod context;
pub mod error;
pub mod llm;
pub mod scheduler;
pub mod storage;
pub mod telegram;
pub mod tools;
pub mod web;
pub mod workspace;
