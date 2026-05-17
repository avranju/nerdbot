//! NerdBot — a minimal, self-hosted AI agent runtime.
//!
//! Run as a single binary, Docker-friendly.
//! Supports Telegram as the user-facing channel with iterative tool use
//! driven by multiple LLM providers.

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

use std::path::PathBuf;

use tracing::info;

/// Parse command-line arguments.
struct CliArgs {
    config: PathBuf,
}

fn parse_args() -> CliArgs {
    let args: Vec<String> = std::env::args().collect();
    let mut config = PathBuf::from("config.toml");

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--config" | "-c" => {
                i += 1;
                if i < args.len() {
                    config = PathBuf::from(&args[i]);
                }
            }
            "--help" | "-h" => {
                println!("NerdBot — AI agent runtime");
                println!();
                println!("Usage: nerdbot [OPTIONS]");
                println!();
                println!("Options:");
                println!("  -c, --config <PATH>  Path to TOML config file (default: config.toml)");
                println!("  -h, --help           Show this help");
                std::process::exit(0);
            }
            _ => {}
        }
        i += 1;
    }

    CliArgs { config }
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

    let args = parse_args();

    info!(config_path = %args.config.display(), "starting nerdbot");

    // Phase 1: load and validate configuration
    let config = match config::AppConfig::from_file(&args.config) {
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

    // Phase 1: print module layout summary
    println!("NerdBot v{}", env!("CARGO_PKG_VERSION"));
    println!();
    println!("Module layout:");
    println!("  agent/          — agent loop, run modes, outcomes");
    println!("  config.rs       — TOML configuration loader");
    println!("  context/        — bounded context, compaction, budgeting");
    println!("  error.rs        — application error types");
    println!("  llm/            — provider-neutral types and trait");
    println!("  scheduler/      — persistent job scheduling");
    println!("  storage/        — SQLite persistence layer");
    println!("  telegram/       — Telegram bot integration");
    println!("  tools/          — tool registry and built-in tools");
    println!("  web/            — search backend and page fetcher");
    println!("  workspace/      — sandboxed file access");
    println!();
    println!("Phase 1 complete: core types, traits, and module layout defined.");
    println!("Next: Phase 2 — fake provider, toy tools, agent loop integration tests.");
}
