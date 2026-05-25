## Project Overview

**NerdBot** is a minimal, self-hosted AI agent runtime written in Rust (Edition 2024).
It runs as a single binary (Docker-friendly) and communicates with users via Telegram.
The core loop: Telegram message → agent loop (LLM proposes tool calls → Rust harness
executes them → results fed back → repeat) → Telegram reply.

### Key Technologies
- **Language:** Rust (Edition 2024)
- **Async runtime:** Tokio (multi-thread, macros, signal, process, time)
- **Database:** SQLite via `sqlx` with `migrate` feature
- **LLM abstraction:** `genai` crate (supports OpenAI, Anthropic, Gemini, OpenRouter, custom OpenAI-compatible endpoints)
- **Telegram:** Long polling (no webhooks)
- **Web search:** Exa API
- **Config:** TOML files + environment variables
- **Logging:** `tracing` + `tracing-subscriber` with `env-filter`
- **Cron parsing:** `cron` crate
- **Serialization:** `serde` + `serde_json`
- **CLI:** `clap` with derive

### Source Layout
```
src/
  main.rs          — CLI entry, long polling loop, service wiring
  lib.rs           — Crate root, re-exports all modules for tests
  config.rs        — TOML config loader (AppConfig with agent/telegram/storage/workspace/llm/context/scheduler/shell/exa sections)
  error.rs         — AgentError enum + domain-specific error types

  agent/
    mod.rs
    agent_loop.rs  — Core iterative tool-loop (run_agent) via genai
    outcome.rs     — AgentOutcome enum (FinalText, Silent, Cancelled)
    run_mode.rs    — AgentRunMode (InteractiveReply, ScheduledJob, Internal)

  llm/
    mod.rs         — LlmExecutor trait + LlmClient wrapping genai::Client
    fake.rs        — FakeProvider for testing without real LLM APIs

  telegram/
    mod.rs
    bot.rs         — Telegram Bot API client (long polling)
    commands.rs    — Bot command parsing/handling (/help, /jobs, /run, /delete, /reset-context)
    handler.rs     — MessageHandler: allowlist → session → route → agent loop → reply
    service.rs     — TelegramService: send_message, etc.

  scheduler/
    mod.rs
    cron.rs        — get_next_cron_run helper
    models.rs      — ScheduledJob, JobStatus, ScheduleType, JobContextPolicy
    runner.rs      — run_scheduled_job: builds AgentContext for scheduled runs
    service.rs     — SchedulerService: background loop, startup reload, notifier

  tools/
    mod.rs
    traits.rs      — Tool trait + ToolContext + ToolOutput
    registry.rs    — ToolRegistry: register, specs, execute
    calculator.rs  — Math calculator tool
    echo.rs        — Echo/debug tool
    files.rs       — read_file, write_file, append_file, list_directory
    schedule.rs    — schedule_job, list_jobs, delete_job, run_job_now
    shell.rs       — ShellExecute (sandboxed command execution)
    telegram.rs    — SendTelegramMessage tool
    web.rs         — WebSearch (Exa) + WebFetch tools

  context/
    mod.rs
    budget.rs      — ContextBudget: token budgeting
    manager.rs     — ContextManager: builds bounded model input
    summaries.rs   — Context summary storage/retrieval
    compaction_service.rs — Monitors session pressure, triggers async compaction
    compaction_worker.rs  — Loads history, calls compaction model, persists summary

  storage/
    mod.rs
    sqlite.rs      — Database: SQLite pool, migration init, foreign keys
    sessions.rs    — Chat session CRUD
    messages.rs    — Message persistence (role, content, token estimate)
    summaries.rs   — Context summary CRUD
    jobs.rs        — Scheduled job CRUD

  web/
    mod.rs
    fetcher.rs     — URL fetcher with SSRF protection, redirect limits, size limits
    search_backend.rs — Web search abstraction

  workspace/
    mod.rs
    sandbox.rs     — Path sandboxing: rejects traversal outside workspace root

tests/             — Integration tests (agent_loop, storage, scheduler, telegram, workspace, context, tools, access_control, config)
docs/              — System design document and other docs
migrations/        — SQLx migrations (00000000000001_init.sql)
```

### Runtime Flows

**Interactive Telegram message:**
1. Long polling receives update
2. MessageHandler checks allowlist (chat_id + user_id)
3. Ensures chat session exists (creates if new)
4. Persists user message
5. Routes: if `/command` → CommandHandler, else → agent loop
6. Agent loop: loads recent messages + personality → iterative tool loop → final text
7. Persists assistant reply → sends to Telegram

**Scheduled job:**
1. SchedulerService background loop detects due job
2. Runs job via scheduler::runner (builds AgentContext with ScheduledJob mode)
3. Agent loop executes with job prompt; model may use web_search, send_user_message, etc.
4. Job status updated to Success/Failed
5. If notify_on_completion and model didn't send a message, harness sends final text

**Context compaction (background):**
- Soft threshold (default 60%) → enqueue async compaction
- Hard threshold (default 85%) → fallback: use summary + fewer recent turns
- Compaction worker produces structured summary with covers_through_message_id
- Future context = latest summary + raw messages after boundary

### Built-in Tools (registered in main.rs)
- `echo` — Debug echo
- `calculator` — Math evaluation
- `schedule_job` / `list_jobs` / `delete_job` / `run_job_now` — Job management
- `send_telegram_message` — Send messages to Telegram chats
- `read_file` / `write_file` / `append_file` / `list_directory` — File I/O (sandboxed)
- `web_search` — Exa-powered web search
- `web_fetch` — Fetch URL content with SSRF protection
- `shell_execute` — Sandboxed command execution

### Key Config Sections (TOML)
- `[agent]` — name, personality_file, max_tool_iterations, default_timezone
- `[telegram]` — bot_token_env, allowed_chat_ids, allowed_user_ids
- `[storage]` — sqlite_path
- `[workspace]` — root, max_read_bytes, max_write_bytes
- `[llm]` — model, endpoint (override), api_key_env (override), temperature, max_output_tokens
- `[context]` — soft/hard thresholds, recent_turns_to_preserve, compactor provider/model
- `[scheduler]` — run_overdue_one_shots_on_startup
- `[shell]` — allowed_commands, denied_commands, max_output_bytes, timeout_secs
- `[exa]` — api_key_env, max_results, max_text_chars

### Error Types
`AgentError` covers: LlmProvider, ToolExecution, ToolNotFound, InvalidToolArgs,
Telegram, Storage, Config, MaxToolIterationsExceeded, Context, Scheduler, WebSearch,
WebFetch, FileIo, SandboxViolation, TokenEstimation, Compaction, PermissionDenied,
Timeout, Generic.

# Agent Instructions & Standing Rules

Standing instructions and behavior rules that **must** be followed by any AI agent working on the NerdBot project.

## Git Commit Constraints
* **Rule:** **Do NOT automatically commit to Git after editing or verifying files.**
* **Behavior:**
  * Only edit and verify files locally.
  * Leave your modifications staged or unstaged in the workspace.
  * **Only perform a Git commit when the user explicitly requests to do so in the chat.**

## Core Architectural Patterns
* Use clean, idiomatic Rust.
* SQLite connections must actively enable `.foreign_keys(true)` constraints via connection options on startup.
* Cache long-lived config and secret variables (like `TELEGRAM_BOT_TOKEN`) on service/handler instantiation rather than reading them from the environment repeatedly.

## Session Continuity
* After completing a feature, fix, or any significant change, **update this AGENTS.md file** to reflect the new state of the codebase. Add or modify sections in "Project Overview" → "Source Layout" or "Runtime Flows" as needed so the next Pi session can build context by scanning this file without exploring the codebase.
* When in doubt, include: what files changed, why, and how the change affects other modules or flows.
