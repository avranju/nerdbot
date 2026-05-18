//! NerdBot — a minimal, self-hosted AI agent runtime.
//!
//! Run as a single binary, Docker-friendly.
//! Supports Telegram as the user-facing channel with iterative tool use
//! driven by multiple LLM providers.

// TODO: Remove this allow once all modules are fully implemented (target Phase 5+).
#![allow(dead_code, unused, unused_imports, unused_variables, unused_assignments)]

use std::path::PathBuf;

use clap::Parser;
use tracing::info;

mod agent;
mod config;
mod context;
mod error;
mod llm;
mod scheduler;
mod storage;
mod telegram;
mod tools;
mod web;
mod workspace;

/// NerdBot — a minimal, self-hosted AI agent runtime.
#[derive(Parser, Debug)]
#[command(version, about)]
struct Cli {
    /// Path to TOML configuration file.
    #[arg(short, long, default_value = "config.toml")]
    config: PathBuf,
}

#[tokio::main]
async fn main() {
    // Initialize structured logging
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info".into()),
        )
        .init();

    let cli = Cli::parse();

    info!(config_path = %cli.config.display(), "starting nerdbot");

    // Phase 1: load and validate configuration
    let config = match config::AppConfig::from_file(&cli.config) {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(error = %e, "no config file found or invalid, using defaults");
            config::AppConfig::default()
        }
    };

    info!(
        agent_name = config.agent.name,
        provider = config.llm.provider,
        model = config.llm.model,
        "configuration loaded"
    );
}
