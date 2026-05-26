//! Tool system — trait, registry, and built-in tool implementations.

pub mod calculator;
pub mod echo;
pub mod registry;
pub mod shell;
pub mod traits;

// Built-in tool modules
pub mod files;
pub mod schedule;
pub mod telegram;
pub mod web;
