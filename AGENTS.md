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
- **Telegram:** Long polling or webhook push via axum
- **Web search:** Exa API
- **Config:** TOML files + environment variables
- **Logging:** `tracing` + `tracing-subscriber` with `env-filter`
- **File watching:** `notify` crate for hot-reloading the personality prompt
- **Cron parsing:** `cron` crate
- **Serialization:** `serde` + `serde_json`
- **CLI:** `clap` with derive + `cliclack` interactive prompts

### Source Layout
```
src/
  main.rs          — CLI entry, Telegram poll/push ingress dispatch, webhook server, service wiring (imports from lib crate)
  lib.rs           — Crate root, re-exports all modules for tests and binary crate
  config.rs        — TOML config loader (AppConfig with agent/telegram/storage/workspace/llm/context/scheduler/shell/files/exa sections)
  onboarding.rs    — Interactive `config.toml` generator for first-run setup
  error.rs         — AgentError enum + domain-specific error types

  agent/
    mod.rs
    agent_loop.rs  — Core iterative tool-loop (run_agent) via genai
    outcome.rs     — AgentOutcome enum (FinalText, Silent, Cancelled)
    personality.rs — Personality prompt cache: loads `[agent].personality_file`, watches its directory, filters create/content-modify/name-modify events for that file, and hot-reloads changed contents in memory
    run_mode.rs    — AgentRunMode (InteractiveReply, ScheduledJob, Internal)

  llm/
    mod.rs         — LlmExecutor trait + LlmClient wrapping genai::Client, including configurable retries for transient network/server failures
    fake.rs        — FakeProvider for testing without real LLM APIs

  telegram/
    mod.rs
    bot.rs         — Telegram Bot API client (token-qualified API URLs, long polling, webhook registration, get_file, download_file)
    attachment.rs  — Attachment DTOs, MIME validation, signature inspection, bounded download, LLM content conversion
    commands.rs    — Bot command parsing/handling (/help, /jobs, /run, /delete, /reset_context, /new_topic)
    handler.rs     — MessageHandler: allowlist → session → route → agent loop → reply (with rich message/attachment support)
    markdown.rs    — Markdown parser and converter for escaping Telegram's MarkdownV2 format safely
    service.rs     — TelegramService: send_message, etc.
    update/
      mod.rs       — TelegramUpdate trait and ingress implementation exports
      poll.rs      — TelegramPoll getUpdates implementation
      hook.rs      — TelegramHook axum webhook + mpsc channel implementation

  scheduler/
    mod.rs
    cron.rs        — get_next_cron_run helper
    models.rs      — ScheduledJob, JobStatus, ScheduleType, JobContextPolicy
    runner.rs      — run_scheduled_job: builds AgentContext for scheduled runs; sends fallback Telegram completion notifications unless current run metadata shows send_user_message already succeeded
    service.rs     — SchedulerService: background loop, startup reload, notifier

  tools/
    mod.rs
    traits.rs      — Tool trait + ToolContext + ToolOutput
    registry.rs    — ToolRegistry: register, specs, execute
    calculator.rs  — Math calculator tool
    echo.rs        — Echo/debug tool
    files.rs       — read_file, write_file, append_file, list_directory
    schedule.rs    — schedule_job, list_jobs, delete_job, run_job_now
    shell.rs       — ShellExecute (bubblewrap-sandboxed command execution with namespace isolation)
    telegram.rs    — SendTelegramMessage tool
    web.rs         — WebSearch (Exa) + WebFetch tools

  context/
    mod.rs
    budget.rs      — ContextBudget: token budgeting (soft/hard thresholds, usable budget)
    diagnostics.rs — Shared session context snapshot calculation for diagnostics and compaction pressure
    manager.rs     — ContextManager: bounded context assembly (summary + recent messages) and current-turn datetime enrichment
    summaries.rs   — ContextSummary struct + CRUD via storage layer
    compaction_service.rs — Monitors session pressure, triggers async compaction with per-session state tracking
    compaction_worker.rs  — Loads old history, calls LLM to produce structured summary, persists it

  diagnostics/
    mod.rs
    client.rs       — Unix socket client plus human-readable and JSON renderers
    protocol.rs     — Newline-delimited JSON request/response DTOs for local diagnostics
    server.rs       — Opt-in owner-only Unix socket server for live session snapshots

  storage/
    mod.rs
    sqlite.rs      — Database: SQLite pool, migration init, foreign keys
    sessions.rs    — Chat session CRUD
    messages.rs    — Message persistence (role, content, token estimate)
    summaries.rs   — Context summary CRUD
    jobs.rs        — Scheduled job CRUD

  web/
    mod.rs
    fetcher.rs     — URL fetcher with SSRF protection, redirect limits, size limits, and Exa contents cache freshness control
    search_backend.rs — Web search abstraction

  workspace/
    mod.rs
    sandbox.rs     — Path sandboxing: rejects traversal outside workspace root

tests/             — Integration tests (agent_loop, storage, scheduler, telegram, workspace, context, tools, access_control, config)
docs/              — System design document and other docs
  nerdbot-user.service.example — Sample user systemd unit for running NerdBot from ~/.local/bin with config/secrets under ~/.config/nerdbot
migrations/        — SQLx migrations (00000000000001_init.sql)
Dockerfile         — Multi-stage Docker build (builder → runtime)
docker-compose.yml — Example Docker Compose setup
config.toml.example — Annotated example configuration
personality.md.example — Example personality/system prompt
README.md          — Project documentation
```

### Runtime Flows

**Telegram message with attachments (rich ingress):**
0. TelegramBot builds production Bot API URLs as `https://api.telegram.org/bot<TOKEN>/<method>`, verifies credentials with `getMe`, and configures the Telegram slash-command menu via `setMyCommands` during startup. `[telegram].mode = "poll"` clears any existing webhook and uses `getUpdates`; after an empty `getUpdates` result, `TelegramPoll::poll` sleeps for `[telegram].poll_interval_secs` (default 5) before returning `None` to the loop. `[telegram].mode = "push"` requires an HTTPS `web_hook_url`, generates a startup secret token, registers it with Telegram via `setWebhook`, and starts a plain HTTP axum server on `[telegram].host`/`port` (default `127.0.0.1:24682`) that validates `X-Telegram-Bot-Api-Secret-Token`.
1. Polling or webhook push receives update; message may include `text`, `caption`, `photo` (array of PhotoSize), and/or `document`
2. `build_inbound_message` (in main.rs) processes the update:
   a. Selects the largest photo variant (by width × height area)
   b. Downloads supported attachments via `TelegramBot::process_attachment` (which calls `get_file` plus the bounded CDN download helper; API and file base URLs are independently injectable for testing)
   c. Classifies each attachment: binary (images, PDFs) → base64-encoded ContentPart::Binary; text documents (txt, md, json, csv, etc.) → ContentPart::Text with filename markers
   d. Validates MIME types and inspects file signatures (PNG, JPEG, GIF, WebP, PDF magic bytes)
   e. Sanitizes filenames (strips path separators, null bytes, truncates to 200 chars)
   f. Builds a user prompt from `caption` (priority) → `text` → default prompt ("Please analyze the attached file(s).")
   g. Never exposes token-qualified Telegram download URLs to the LLM
   h. Omits structured inbound payloads from tracing spans so attachment base64 content is not written to logs; records only text length and attachment counts
3. MessageHandler checks allowlist (chat_id + user_id) BEFORE downloading any files
4. Ensures chat session exists (creates if new)
5. Routes: if `/command` → CommandHandler, else → agent loop
6. ContextManager assembles bounded context: loads latest summary + recent messages from DB, prefers the `recent_turns_to_preserve` window (default 30 messages) while still enforcing the request budget, excludes binary payloads from token estimation, and appends the current date/time as a trailing text part on the current user message without flattening rich attachment parts. The datetime is formatted in 24-hour local time with timezone abbreviation and UTC offset.
7. Starts a Telegram `typing` chat action and refreshes it every 4 seconds while the interactive agent loop runs
8. Agent loop: cached `Personality` contents + configured timezone runtime context + bounded context → iterative tool loop → final text (with token tracking from genai response); typing refresh stops as soon as the run returns
9. Persists current user message and assistant reply → sends to Telegram
10. After successful run: checks if token usage exceeds soft threshold → calls CompactionService for async compaction if needed

**CLI onboarding:**
1. Run `nerdbot onboard` (optionally with `--config <path>`)
2. If the config file exists, load it and use its current values as prompt defaults
3. `cliclack` prompts for agent name, timezone, Telegram token environment variable, Telegram ingress mode (`poll` or `push`), push-mode webhook URL/host/port when applicable, chat/user allowlists, LLM provider/model and optional API-key environment variable, custom endpoint details when needed, shell sandbox mode, and optional Exa API-key environment variable
4. Update the selected values in a valid TOML file without embedding secrets; write fixed deployment defaults for `[agent].personality_file` (`/config/personality.md`), `[workspace].root` (`/workspace`), and `[storage].sqlite_path` (`/data/agent.db`), preserve existing settings outside the guided flow, use `AppConfig` defaults for omitted settings in a new file, and clear `[telegram].web_hook_url` when onboarding is set back to poll mode

**Custom OpenAI-compatible LLM endpoint:**
- `LlmClient::from_config` normalizes configured endpoint URLs with a trailing slash and binds `genai` to the OpenAI adapter, preventing unknown local model names from falling back to native Ollama routing
- If no `llm.api_key_env` is configured, the client supplies an empty placeholder auth value so local servers without authentication work

**Interactive Telegram message:**
0. TelegramBot builds production Bot API URLs as `https://api.telegram.org/bot<TOKEN>/<method>`, verifies credentials with `getMe`, and configures the Telegram slash-command menu via `setMyCommands` during startup. Reset commands use Telegram-safe underscore names (`/reset_context`, `/new_topic`) because Telegram command menus only allow lowercase letters, digits, and underscores.
1. Polling or webhook push receives update
2. MessageHandler checks allowlist (chat_id + user_id)
3. Ensures chat session exists (creates if new)
4. Routes: if `/command` → CommandHandler, else → agent loop
5. ContextManager assembles bounded context: loads latest summary + recent messages from DB, respects token budget, appends current user message once, and adds the current date/time as trailing 24-hour timezone-qualified text in that user message to preserve cacheable prompt prefixes
6. Starts a Telegram `typing` chat action and refreshes it every 4 seconds while the interactive agent loop runs
7. Agent loop: cached `Personality` contents + configured timezone runtime context + bounded context → iterative tool loop → final text (with token tracking from genai response); typing refresh stops as soon as the run returns
8. Persists current user message and assistant reply → sends to Telegram
9. After successful run: checks if token usage exceeds soft threshold → calls CompactionService for async compaction if needed

**Scheduled job:**
1. SchedulerService background loop detects due job
2. Runs job via scheduler::runner (builds AgentContext with ScheduledJob mode)
3. Agent loop executes with cached `Personality` contents, configured timezone runtime context, and the job prompt enriched with current date/time as trailing user-message text; model may use web_search, send_user_message, etc.
4. Job status updated to Success/Failed
5. If notify_on_completion and the model didn't call send_user_message successfully during the current job run, harness sends final text as a fallback notification. The decision comes from AgentResult metadata produced inside the active agent loop rather than scanning session history, so unrelated tool calls or prior scheduled jobs in the same chat cannot suppress the fallback.

**Personality prompt cache:**
- Startup creates one `agent::personality::Personality` from `[agent].personality_file`, loads the file contents once, and hands clones to `MessageHandler`, `SchedulerService`, and the diagnostics server.
- `Personality` keeps the prompt in memory behind shared ownership, watches the configured file's parent directory with `notify`, only reacts to `Create` or content/name `Modify` events for the configured file path, hashes the file contents, and reloads the cache only when the hash changes. Access and metadata-only events are ignored so reading the file cannot trigger a reload loop.
- If the file is missing or cannot be read, NerdBot uses the built-in default prompt and logs watcher/read failures instead of failing startup.

**Context compaction (background):**
- After each successful agent run, handler checks if total_tokens > soft_threshold
- If above threshold: calls CompactionService, which tracks per-session state (Idle/Running/RunningAndDirty) and prevents concurrent compactions
- CompactionService runs CompactionWorker asynchronously using the same LLM configured in `[llm]`. If no LLM model is configured, compaction is disabled (no-op).
- CompactionService uses `ContextDiagnosticsSnapshot` to estimate uncompacted raw-history tokens from stored estimates with content-based fallback. This shared calculation reports soft/hard threshold distance and replaces the older fixed `message_count * 100` pressure heuristic.
- CompactionWorker loads messages after the latest summary boundary, excludes the configured recent raw-message preservation window, combines eligible older messages with the existing summary, and persists a new structured summary with updated covers_through_message_id
- Hard threshold (default 85%): ContextManager bounds messages to fit budget

**Local diagnostics socket (opt-in):**
1. Start NerdBot with `--diagnostics-socket <path>` to bind a local Unix domain socket; no diagnostics service runs unless this option is provided
2. The socket uses owner-only (`0600`) permissions, refuses to replace regular files or active sockets, removes stale socket nodes, and is cleaned up during graceful shutdown
3. The newline-delimited JSON protocol supports `ping`, `list_sessions`, and `show_session` by database session ID or Telegram chat ID
4. Query the running instance with `nerdbot --diagnostics-socket <path> diagnostics ping`, `list-sessions`, or `show (--chat-id <id> | --session-id <uuid>)`; add `--json` for scripting
5. `show_session` returns the shared context snapshot, live in-memory `CompactionState`, the effective cached personality prompt (with timezone context), the summary prompt metadata, and tool-spec count/cost. The full prompt and spec bodies are hidden by default and returned only when requested (e.g. `--show-prompts`).

### Built-in Tools (registered in main.rs)
- `echo` — Debug echo
- `calculator` — Math evaluation
- `schedule_job` / `list_jobs` / `delete_job` / `run_job_now` — Job management
- `send_telegram_message` — Send messages to Telegram chats
- `read_file` / `write_file` / `append_file` / `list_directory` — File I/O (sandboxed)
- `web_search` — Exa-powered web search
- `web_fetch` — Fetch URL content with SSRF protection. Accepts optional `max_age_hours`; omit it for Exa's default cached contents behavior or set `0` to disable Exa's cache for fresh upstream content.
- `shell_execute` — Sandboxed command execution with optional Bubblewrap namespace isolation (filesystem, PID, network, IPC, UTS). Configurable via `sandbox_mode`: `none` (direct exec), `bwrap` (namespace isolation), `bwrap-strict` (reserved for future resource limits), plus `network_access`: `disabled` (default, passes `--unshare-net`) or `host` (omits `--unshare-net` so bwrap shares host networking).

### Key Config Sections (TOML)
- `[agent]` — name, personality_file, max_tool_iterations, default_timezone
- `[telegram]` — mode (`"poll"` default or `"push"`), bot_token_env, poll_interval_secs (default 5; sleep after empty `getUpdates` in poll mode), HTTPS web_hook_url (required for push), host/port for the local webhook server (default `127.0.0.1:24682`), allowed_chat_ids, allowed_user_ids, max_attachment_bytes (default 5 MB), max_text_document_chars (default 32 KB)
- `[storage]` — sqlite_path
- `[workspace]` — root, max_read_bytes, max_write_bytes
- `[files]` — max_read_bytes, max_write_bytes
- `[llm]` — model, endpoint (override), api_key_env (override), temperature, max_output_tokens, context_window_tokens, max_retries (default 4), retry_interval_secs (default 10)
- `[context]` — soft/hard thresholds, recent_turns_to_preserve, reserved_tool_loop_tokens
- `[scheduler]` — run_overdue_one_shots_on_startup
- `[shell]` — allowed_commands, denied_commands, max_output_bytes, timeout_secs, sandbox_mode (`"none"` | `"bwrap"` | `"bwrap-strict"`), network_access (`"disabled"` | `"host"`)
- `[exa]` — api_key_env, max_results, max_text_chars

### Docker Packaging
- **Dockerfile** — multi-stage build: `rust:1.96-slim-bookworm` for compilation, `debian:bookworm-slim` for runtime with `libsqlite3-0` and `ca-certificates`, non-root `nerdbot` user
- **docker-compose.yml** — named volume for SQLite data, read-only config mount, writable workspace mount, environment-variable-based secrets
- Entrypoint: `nerdbot --config /config/config.toml`
- **config.toml.example** — annotated example configuration covering all sections
- **personality.md.example** — example personality/system prompt file
- **README.md** — comprehensive project documentation (features, quick start, config reference, architecture, deployment)

### Systemd User Service
- **nerdbot-user.service.example** — sample `systemd --user` unit. It runs `%h/.local/bin/nerdbot --config %h/.config/nerdbot/config.toml`, loads optional secrets from `%h/.config/nerdbot/nerdbot.env`, uses `%h/.local/share/nerdbot` as the working directory, restarts on failure, and sets conservative user-service hardening (`UMask=0077`, `NoNewPrivileges=true`, `PrivateTmp=true`). README includes install commands and the `loginctl enable-linger "$USER"` note for running after logout.

### Error Types
`AgentError` covers: LlmProvider, ToolExecution, ToolNotFound, InvalidToolArgs,
Telegram, Storage, Config, MaxToolIterationsExceeded, Context, Scheduler, WebSearch,
WebFetch, FileIo, SandboxViolation, TokenEstimation, Compaction, PermissionDenied,
Timeout, Generic.

### Bubblewrap Sandbox — Known Limitations
- **AppArmor on Ubuntu 24.04+**: The default AppArmor policy blocks unprivileged
  user namespace creation. Workaround: set `kernel.apparmor_restrict_unprivileged_userns=0`
  via sysctl or GRUB. This is a well-known issue affecting Flatpak, Podman, and
  OpenAI Codex as well.
- **Linux-only**: Bubblewrap requires user namespaces (Linux-specific).
- **Kernel version**: User namespaces require kernel 3.8+. Both are widely
  available on modern systems.
- **No L7 network filtering**: Bubblewrap provides network namespace isolation
  (all-or-nothing), not HTTP-level policy. NerdBot defaults to `network_access = "disabled"`;
  setting `network_access = "host"` omits `--unshare-net` and allows normal host networking.

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
* After completing a feature, fix, or any significant change, **update this AGENTS.md file** to reflect the new state of the codebase. Add or modify sections in "Project Overview" → "Source Layout" or "Runtime Flows" as needed so the next coding session can build context by scanning this file without exploring the codebase.
* When in doubt, include: what files changed, why, and how the change affects other modules or flows.
