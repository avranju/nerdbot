//! NerdBot — a minimal, self-hosted AI agent runtime.
//!
//! This crate provides the core types, traits, and abstractions for the agent.

// Re-export all modules for integration tests
pub mod agent;
pub mod config;
pub mod context;
pub mod error;
pub mod llm;
pub mod onboarding;
pub mod scheduler;
pub mod storage;
pub mod telegram;
pub mod tools;
pub mod web;
pub mod workspace;
