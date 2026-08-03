## Project Overview

**NerdBot** is a minimal, self-hosted AI agent runtime written in Rust (Edition 2024).
It runs as a single binary (Docker-friendly) and communicates with users through
pluggable communication channels. Telegram and Zulip are supported channel adapters.
The core loop: channel message → agent loop (LLM proposes tool calls → Rust harness
executes them → results fed back → repeat) → channel reply.

### Key Technologies
- **Language:** Rust (Edition 2024)
- **Async runtime:** Tokio (multi-thread, macros, signal, process, time)
- **Database:** SQLite via `sqlx` with `migrate` feature
- **LLM abstraction:** `genai` crate (supports OpenAI, Anthropic, Gemini, OpenRouter, custom OpenAI-compatible endpoints)
- **Channels:** Generic channel abstraction; Telegram and Zulip each support long polling or webhook push via a shared axum webhook server
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
  main.rs          — CLI entry plus startup helpers: tracing/config load, top-level runtime composition, channel registry/service wiring, shared webhook setup, scheduler/diagnostics startup, and high-level channel loop orchestration
  lib.rs           — Crate root, re-exports all modules for tests and binary crate
  config.rs        — TOML config loader (AppConfig with agent/webhook/channels/storage/workspace/llm/context/scheduler/shell/files/exa sections)
  onboarding.rs    — Interactive `config.toml` generator for first-run setup
  error.rs         — AgentError enum + domain-specific error types
  webhook.rs       — Shared Axum webhook server for Telegram/Zulip push ingress routes and `/health`

  attachments.rs   — Shared HEIC/HEIF detection and in-memory conversion to JPEG via `libheif-rs` and `image` crates

  agent/
    mod.rs
    agent_loop.rs  — Core iterative tool-loop (run_agent) via genai
    outcome.rs     — AgentOutcome enum (FinalText, Silent, Cancelled)
    personality.rs — Personality prompt cache: loads `[agent].personality_file`, watches its directory, filters create/content-modify/name-modify events for that file, and hot-reloads changed contents in memory
    run_mode.rs    — AgentRunMode (InteractiveReply, ScheduledJob, Internal)

  llm/
    mod.rs         — LlmExecutor trait + LlmClient wrapping genai::Client, including configurable retries for transient network/server failures
    fake.rs        — FakeProvider for testing without real LLM APIs

  channel/
    mod.rs         — Generic communication channel exports
    types.rs       — ConversationAddress, SenderIdentity, InboundMessage, OutboundMessage, MessageFormat, ConversationAddressPattern, ChannelAccessPolicy
    traits.rs      — ChannelService and ChannelIngress traits plus typing guard
    registry.rs    — ChannelRegistry for outbound routing by channel_id
    handler.rs     — ChannelMessageHandler: access policy → session → route → agent loop → reply

  telegram/
    mod.rs
    bot.rs         — Telegram Bot API client (token-qualified API URLs, long polling, webhook registration, get_file, download_file)
    attachment.rs  — Attachment DTOs, MIME validation, signature inspection, bounded download, LLM content conversion
    commands.rs    — Bot command parsing/handling (/help, /jobs, /run, /delete, /reset_context, /new_topic); `/jobs` lists active jobs only while `/delete` soft-deletes by disabling rows
    handler.rs     — Telegram-facing compatibility adapter over ChannelMessageHandler for Telegram-shaped callers/tests
    inbound.rs     — Telegram inbound adapter that converts raw Telegram messages into channel-generic InboundMessage payload pieces, including photo/document attachment processing before ChannelMessageHandler dispatch
    markdown.rs    — Markdown parser and converter for escaping Telegram's MarkdownV2 format safely
    runtime.rs     — Telegram runtime startup, command-menu setup, poll/webhook ingress loop, allowlist precheck, and raw update dispatch into ChannelMessageHandler
    service.rs     — TelegramService: send_message, typing, ChannelService implementation
    update/
      mod.rs       — TelegramUpdate trait and ingress implementation exports
      poll.rs      — TelegramPoll getUpdates implementation
      hook.rs      — TelegramHook queue-backed webhook ingress; shared HTTP server validates/enqueues updates

  zulip/
    mod.rs         — Exports ZulipBot, ZulipService, ZulipPoll, ZulipHook
    bot.rs         — Zulip HTTP API client (Basic auth, send_message, update_presence, register_queue, get_events, download_file)
    attachment.rs  — Regex extraction of user-upload markdown links, MIME classification, bounded download, LLM content conversion
    runtime.rs     — Zulip runtime startup, retrying bot identity resolution (six attempts with 1/2/4/8/16-second backoff waits), optional presence heartbeat, poll/webhook ingress loop, and raw message dispatch into ChannelMessageHandler
    service.rs     — ZulipService: send_message with 10K char splitting, typing indicators (PMs only), ChannelService implementation
    update/
      mod.rs       — Zulip ingress module entrypoint plus ZulipUpdate raw-message trait
      poll.rs      — ZulipPoll: event queue registration, long-polling loop, BAD_EVENT_QUEUE_ID recovery (including Zulip's HTTP 400 form), raw message buffering
      hook.rs      — ZulipHook queue-backed webhook ingress; shared HTTP server validates/enqueues payloads

  scheduler/
    mod.rs
    cron.rs        — get_next_cron_run helper
    models.rs      — ScheduledJob, JobStatus, ScheduleType, JobContextPolicy
    runner.rs      — run_scheduled_job: builds AgentContext for scheduled runs; sends fallback channel completion notifications unless current run metadata shows send_user_message already succeeded
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
    messaging.rs   — send_user_message tool (enforces full channel conversation identity)
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
  zulip-editable-progress-messages.md — Proposal for Zulip progress/status messages using one editable message per agent run
  nerdbot-user.service.example — Sample user systemd unit for running NerdBot from ~/.local/bin with config/secrets under ~/.config/nerdbot
migrations/        — SQLx migrations (00000000000001_init.sql)
.gitea/workflows/
  docker-image.yml — Manual Gitea Actions workflow that builds the root Dockerfile and pushes git.nerdworks.dev/avranju/nerdbot:<tag>
Dockerfile         — Multi-stage Docker build (builder → runtime)
docker-compose.yml — Example Docker Compose setup
config.toml.example — Annotated example configuration
personality.md.example — Example personality/system prompt
README.md          — Project documentation
```

### Runtime Flows

**Telegram message with attachments (rich ingress):**
0. TelegramBot builds production Bot API URLs as `https://api.telegram.org/bot<TOKEN>/<method>`, verifies credentials with `getMe`, configures the Telegram slash-command menu via `setMyCommands`, and registers TelegramService in ChannelRegistry under channel_id `telegram`. `[channels.telegram].ingress = "poll"` clears any existing webhook and uses `getUpdates`; after an empty `getUpdates` result, `TelegramPoll::poll` sleeps for `[channels.telegram].poll_interval_secs` (default 5) before returning `None` to the loop. `[channels.telegram].ingress = "webhook"` requires an HTTPS `web_hook_url`, generates a startup secret token, registers it with Telegram via `setWebhook`, and uses the shared plain HTTP webhook server on `[webhook].host`/`port` (default `127.0.0.1:24682`) to validate `X-Telegram-Bot-Api-Secret-Token` and enqueue updates. The same server exposes `/health` and can also host Zulip webhook routes.
1. Polling or webhook push receives update; message may include `text`, `caption`, `photo` (array of PhotoSize), and/or `document`
2. `telegram::inbound::build_inbound_message` processes the update:
   a. Selects the largest photo variant (by width × height area)
   b. Downloads supported attachments via `TelegramBot::process_attachment` (which calls `get_file` plus the bounded CDN download helper; API and file base URLs are independently injectable for testing)
   c. Classifies each attachment: binary (images, PDFs, HEIC/HEIF) → base64-encoded ContentPart::Binary (HEIC/HEIF converted to JPEG in memory via `libheif-rs`); text documents (txt, md, json, csv, etc.) → ContentPart::Text with filename markers
   d. Validates MIME types and inspects file signatures (PNG, JPEG, GIF, WebP, PDF, HEIC/HEIF ftyp brand)
   e. Sanitizes filenames (strips path separators, null bytes, truncates to 200 chars)
   f. Builds a user prompt from `caption` (priority) → `text` → default prompt ("Please analyze the attached file(s).")
   g. Never exposes token-qualified Telegram download URLs to the LLM
   h. Omits structured inbound payloads from tracing spans so attachment base64 content is not written to logs; records only text length and attachment counts
3. ChannelMessageHandler checks channel-qualified access policy (derived from inbound message's `address.channel_id`) BEFORE downloading any files
4. Ensures channel-qualified chat session exists (creates if new)
5. Routes: if `/command` → CommandHandler, else → agent loop
6. ContextManager assembles bounded context: loads latest summary + recent messages from DB, prefers the `recent_turns_to_preserve` window (default 30 messages) while still enforcing the request budget, excludes binary payloads from token estimation, and appends the current date/time as a trailing text part on the current user message without flattening rich attachment parts. The datetime is formatted in 24-hour local time with timezone abbreviation and UTC offset.
7. Starts a channel typing indicator when supported; Telegram refreshes `typing` every 4 seconds while the interactive agent loop runs
8. Agent loop: cached `Personality` contents + configured timezone runtime context + bounded context → iterative tool loop → final text (with token tracking from genai response); typing refresh stops as soon as the run returns
9. Persists current user message and assistant reply → sends via ChannelRegistry. If the agent loop returns `AgentOutcome::Silent` after completing tools with no final assistant text, ChannelMessageHandler sends the harness fallback `NerdBot: Agent run completed with no response.` so users can distinguish a harness-generated completion notice from LLM output.
10. After successful run: checks if token usage exceeds soft threshold → calls CompactionService for async compaction if needed

**CLI onboarding:**
1. Run `nerdbot onboard` (optionally with `--config <path>`)
2. If the config file exists, load it and use its current values as prompt defaults
3. `cliclack` prompts for agent name, timezone, Telegram token environment variable, Telegram ingress (`poll` or `webhook`), Telegram webhook URL when applicable, Zulip enable/disable, Zulip bot email/API key env vars, Zulip site URL, Zulip ingress mode, shared webhook host/port when any webhook ingress is enabled, conversation/sender allowlists, LLM provider/model and optional API-key environment variable, custom endpoint details when needed, shell sandbox mode, and optional Exa API-key environment variable
4. Update the selected values in a valid TOML file without embedding secrets; write fixed deployment defaults for `[agent].personality_file` (`/config/personality.md`), `[workspace].root` (`/workspace`), and `[storage].sqlite_path` (`/data/agent.db`), preserve existing settings outside the guided flow, use `AppConfig` defaults for omitted settings in a new file, and clear `[channels.telegram].web_hook_url` when onboarding is set back to poll mode

**Custom OpenAI-compatible LLM endpoint:**
- `LlmClient::from_config` normalizes configured endpoint URLs with a trailing slash and binds `genai` to the OpenAI adapter, preventing unknown local model names from falling back to native Ollama routing
- If no `llm.api_key_env` is configured, the client supplies an empty placeholder auth value so local servers without authentication work

**Interactive Telegram message:**
0. TelegramBot builds production Bot API URLs as `https://api.telegram.org/bot<TOKEN>/<method>`, verifies credentials with `getMe`, and configures the Telegram slash-command menu via `setMyCommands` during startup. Reset commands use Telegram-safe underscore names (`/reset_context`, `/new_topic`) because Telegram command menus only allow lowercase letters, digits, and underscores.
1. Polling or webhook push receives update
2. ChannelMessageHandler checks conversation/sender allowlist
3. Ensures channel-qualified chat session exists (creates if new)
4. Routes: if `/command` → CommandHandler, else → agent loop
5. ContextManager assembles bounded context: loads latest summary + recent messages from DB, respects token budget, appends current user message once, and adds the current date/time as trailing 24-hour timezone-qualified text in that user message to preserve cacheable prompt prefixes
6. Starts a channel typing indicator when supported
7. Agent loop: cached `Personality` contents + configured timezone runtime context + bounded context → iterative tool loop → final text (with token tracking from genai response); typing refresh stops as soon as the run returns
8. Persists current user message and assistant reply → sends through ChannelRegistry. If the agent loop returns `AgentOutcome::Silent` after completing tools with no final assistant text, ChannelMessageHandler sends the harness fallback `NerdBot: Agent run completed with no response.` so users can distinguish a harness-generated completion notice from LLM output.
9. After successful run: checks if token usage exceeds soft threshold → calls CompactionService for async compaction if needed

**Interactive Zulip message:**
0. ZulipBot authenticates via Basic auth (bot_email + api_key), normalizes site_url with trailing slash, and resolves `/users/me` with up to six attempts separated by 1/2/4/8/16-second exponential-backoff waits. It stores the resolved `user_id` plus `full_name`. The stored user ID is used to skip self-sent messages and to exclude the authenticated account from DM participant/typing-recipient resolution, because human/service accounts may expose event email addresses that differ from the configured login/API email. If all identity attempts fail, ingress still starts with the existing degraded fallback behavior. `[channels.zulip].presence_enabled` defaults to false because current Zulip servers reject `POST /api/v1/users/me/presence` for bot accounts with `This endpoint does not accept bot requests.` If presence is explicitly enabled, the Zulip runtime attempts an active-presence heartbeat with `status = "active"`, `ping_only = true`, and `new_user_input = false`; the heartbeat interval is `[channels.zulip].presence_ping_interval_secs` (default 60 seconds), the task is aborted when the Zulip ingress loop exits, and it self-disables after the bot-account rejection. Zulip supports two ingress modes:
   - **Poll**: POST `/api/v1/register` with `apply_markdown=false` to get raw Markdown message content plus queue_id, then loop GET `/api/v1/events` with `dont_block=false`; on `BAD_EVENT_QUEUE_ID` (including Zulip's HTTP 400 response), preserve the machine-readable code and re-register. Poll interval controlled by `[channels.zulip].poll_interval_secs` (default 2s).
   - **Webhook**: `[channels.zulip].web_hook_url` (required; public HTTPS Zulip webhook URL) — path is extracted from this URL (mirroring Telegram's pattern). The shared plain HTTP webhook server on `[webhook].host`/`port` validates `token` field against `web_hook_token_env`, queues accepted payloads, replies to Zulip with `{"response_not_required": true}`, and sends the eventual bot response asynchronously through Zulip's REST API rather than in the webhook HTTP response.
1. Polling or webhook push receives Zulip message event
2. Ingress first decides whether the message is addressed to NerdBot before access checks, sessions, or attachment downloads. Self-sent messages are ignored; stream messages require a direct `@**BotName**` or Zulip raw `@**BotName|user_id**` mention anywhere in the raw Markdown content; one-to-one DMs are accepted without a mention; group DMs require a direct bot mention anywhere in the message. Unknown bot names do not fall back to broad "any mention" matching, so stream/group-DM messages are ignored unless the fetched `/users/me` full name produced a precise bot mention pattern.
3. Accepted messages have the direct bot mention removed from the prompt text when present.
4. ChannelMessageHandler checks channel-qualified access policy
5. Extracts user-upload attachments from markdown links (`[filename](/user_uploads/...)`), downloads via authenticated GET, classifies as binary (images, PDFs, HEIC/HEIF) or text documents. HEIC/HEIF images are converted to JPEG in memory via the shared `attachments` module before being sent to the LLM.
6. Ensures channel-qualified chat session exists (creates if new)
7. Routes: if `/command` → CommandHandler, else → agent loop
8. ContextManager assembles bounded context: loads latest summary + recent messages from DB, respects token budget
9. Starts a channel typing indicator when supported (Zulip typing is direct-message-only, refreshed every 8 seconds, and stopped with a best-effort `op = "stop"` when the agent run finishes). For direct-message typing notifications, NerdBot keeps the stable conversation/session identity as sorted participant emails but caches numeric Zulip user IDs from inbound `display_recipient` payloads because `/api/v1/typing` requires `type = "direct"` and integer user IDs in the `to` array. When `/users/me` provided a current user ID, NerdBot filters that ID rather than relying only on email string equality.
10. Agent loop: cached `Personality` contents + configured timezone runtime context + bounded context → iterative tool loop → final text; typing refresh stops as soon as the run returns
11. Outbound messages split at 10,000 chars via `ZulipService::send_message`
12. Persists current user message and assistant reply → sends through ChannelRegistry. If the agent loop returns `AgentOutcome::Silent` after completing tools with no final assistant text, ChannelMessageHandler sends the harness fallback `NerdBot: Agent run completed with no response.` so users can distinguish a harness-generated completion notice from LLM output.
13. After successful run: checks if token usage exceeds soft threshold → calls CompactionService for async compaction if needed

**Scheduled job:**
1. SchedulerService background loop detects due job
2. Derives the job's channel-aware access policy from `job.owner_address().channel_id` via `config.channels.access_policy_for()`. A Telegram-owned job gets Telegram's policy; a Zulip-owned job gets Zulip's policy (currently empty/allow-all since only Telegram has configured allowlists).
3. Runs job via scheduler::runner (builds AgentContext with ScheduledJob mode and derived access policy)
4. Agent loop executes with cached `Personality` contents, configured timezone runtime context, and the job prompt enriched with current date/time as trailing user-message text; model may use web_search, send_user_message, etc.
5. Job status updated to Success/Failed
6. If notify_on_completion and the model didn't call send_user_message successfully during the current job run, harness sends final text through ChannelRegistry using `job.owner_address()` as the fallback notification target.

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
3. The newline-delimited JSON protocol supports `ping`, `list_sessions`, and `show_session` by database session ID or channel conversation address
4. Query the running instance with `nerdbot --diagnostics-socket <path> diagnostics ping`, `list-sessions`, or `show (--channel-id <id> --conversation-id <id> [--thread-id <id>] | --session-id <uuid>)`; add `--json` for scripting
5. `show_session` returns the shared context snapshot, live in-memory `CompactionState`, the effective cached personality prompt (with timezone context), the summary prompt metadata, and tool-spec count/cost. The full prompt and spec bodies are hidden by default and returned only when requested (e.g. `--show-prompts`).

### Built-in Tools (registered in main.rs)
- `echo` — Debug echo
- `calculator` — Math evaluation
- `schedule_job` / `list_jobs` / `delete_job` / `run_job_now` — Job management; `list_jobs` returns compact job metadata only (id, name, enabled, schedule_type, cron_expression, run_at, next_run_at, last_run_at, last_status, timezone, notify_on_completion, owner_address) and intentionally omits `creation_context_snapshot`, `prompt`, and `context_policy` to prevent context bloat. Snapshots remain persisted in the database and are available for scheduled job execution. `list_jobs` accepts `include_disabled = true` for disabled/deleted job history, and `delete_job` disables persisted jobs so they stop running but remain available for direct lookup/history.
- `send_user_message` — Send messages to the current or explicitly targeted channel conversation. Enforces the run's channel-aware access policy against the full `ConversationAddress` (channel_id + conversation_id + thread_id), not just conversation_id alone. Same conversation ID on a different channel is rejected.
- `read_file` / `write_file` / `append_file` / `list_directory` — File I/O (sandboxed)
- `web_search` — Exa-powered web search
- `web_fetch` — Fetch URL content with SSRF protection. Accepts optional `max_age_hours`; omit it for Exa's default cached contents behavior or set `0` to disable Exa's cache for fresh upstream content.
- `shell_execute` — Sandboxed command execution with optional Bubblewrap namespace isolation (filesystem, PID, network, IPC, UTS). Configurable via `sandbox_mode`: `none` (direct exec), `bwrap` (namespace isolation), `bwrap-strict` (reserved for future resource limits), plus `network_access`: `disabled` (default, passes `--unshare-net`) or `host` (omits `--unshare-net` so bwrap shares host networking).

### Key Config Sections (TOML)
- `[agent]` — name, personality_file, max_tool_iterations, default_timezone
- `[webhook]` — host/port for the shared local webhook server used by Telegram and/or Zulip webhook ingress (default `127.0.0.1:24682`)
- `[channels.telegram]` — enabled, ingress (`"poll"` default or `"webhook"`), bot_token_env, poll_interval_secs (default 5; sleep after empty `getUpdates` in poll mode), HTTPS web_hook_url (required for webhook), allowed_conversations, allowed_senders, max_attachment_bytes (default 5 MB), max_text_document_chars (default 32 KB)
- `[channels.zulip]` — enabled, ingress (`"poll"` default or `"webhook"`), bot_email_env (default `ZULIP_BOT_EMAIL`), api_key_env (default `ZULIP_BOT_API_KEY`), site_url (required), web_hook_token_env (default `ZULIP_WEBHOOK_TOKEN`), web_hook_url (required for webhook; public HTTPS Zulip webhook URL), poll_interval_secs (default 2), presence_enabled (default false because Zulip rejects bot-account presence requests), presence_ping_interval_secs (default 60; must be >0 when presence is enabled), allowed_conversations (ConversationAddressPattern with stream names and optional topics), allowed_senders (email addresses), max_attachment_bytes (default 5 MB), max_text_document_chars (default 32 KB)
- `[storage]` — sqlite_path
- `[workspace]` — root, max_read_bytes, max_write_bytes
- `[files]` — max_read_bytes, max_write_bytes
- `[llm]` — model, endpoint (override), api_key_env (override), temperature, max_output_tokens, context_window_tokens, max_retries (default 4), retry_interval_secs (default 10)
- `[context]` — soft/hard thresholds, recent_turns_to_preserve, reserved_tool_loop_tokens
- `[scheduler]` — run_overdue_one_shots_on_startup
- `[shell]` — allowed_commands, denied_commands, max_output_bytes, timeout_secs, sandbox_mode (`"none"` | `"bwrap"` | `"bwrap-strict"`), network_access (`"disabled"` | `"host"`)
- `[exa]` — api_key_env, max_results, max_text_chars

### Docker Packaging
- **Dockerfile** — multi-stage build: `rust:1.96-slim-trixie` for compilation, `debian:trixie-slim` for runtime with `libsqlite3-0`, `libheif1` (HEIC/HEIF decoding), and `ca-certificates`, non-root `nerdbot` user
- **docker-compose.yml** — named volume for SQLite data, read-only config mount, writable workspace mount, environment-variable-based secrets
- **.gitea/workflows/docker-image.yml** — manually triggered Gitea Actions workflow. It accepts a required `tag` input, builds `git.nerdworks.dev/avranju/nerdbot:<tag>` from the repository root `Dockerfile`, and pushes it to the Gitea container registry using the `REGISTRY_USERNAME` Actions variable and `PACKAGE_TOKEN` Actions secret.
- Entrypoint: `nerdbot --config /config/config.toml`
- **config.toml.example** — annotated example configuration covering all sections
- **personality.md.example** — example personality/system prompt file
- **README.md** — comprehensive project documentation (features, quick start, config reference, architecture, deployment)

### Systemd User Service
- **nerdbot-user.service.example** — sample `systemd --user` unit. It runs `%h/.local/bin/nerdbot --config %h/.config/nerdbot/config.toml`, loads optional secrets from `%h/.config/nerdbot/nerdbot.env`, uses `%h/.local/share/nerdbot` as the working directory, restarts on failure, and sets conservative user-service hardening (`UMask=0077`, `NoNewPrivileges=true`, `PrivateTmp=true`). README includes install commands and the `loginctl enable-linger "$USER"` note for running after logout.

### Error Types
`AgentError` covers: LlmProvider, ToolExecution, ToolNotFound, InvalidToolArgs,
Telegram, Zulip, Storage, Config, MaxToolIterationsExceeded, Context, Scheduler, WebSearch,
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

## Channel Access Policy
* NerdBot uses a channel-aware access policy system: `ChannelAccessPolicy` groups `allowed_conversations` (a list of `ConversationAddressPattern`) and `allowed_senders`.
* `ConversationAddressPattern` includes `channel_id`, `conversation_id`, and `thread_id`. A pattern with `thread_id: None` matches any thread (wildcard); `Some(id)` requires exact thread match.
* `AppConfig.channels.access_policy_for(channel_id)` derives the policy for a channel from its config. Supports `"telegram"` (maps `[channels.telegram].allowed_conversations` to channel-qualified patterns) and `"zulip"` (passes through `[channels.zulip].allowed_conversations` as-is, which are already ConversationAddressPattern structs with stream names and optional topic thread_ids).
* The interactive handler (`ChannelMessageHandler`) derives the policy from the inbound message's `address.channel_id`.
* The scheduler derives the policy from the job's `owner_address().channel_id` before running it.
* `AgentContext` and `ToolContext` carry a single `access_policy: ChannelAccessPolicy` field (replacing the old flat `allowed_conversations: Vec<String>` / `allowed_senders: Vec<String>`).
* `send_user_message` enforces the policy against the full `ConversationAddress` — same conversation ID on a different channel is rejected.
* Telegram config shape is preserved: `allowed_conversations = ["123"]` becomes a channel-qualified pattern internally. Zulip uses full `ConversationAddressPattern` structs directly (e.g., `{ channel_id = "zulip", conversation_id = "general" }` for all topics in stream, or `{ channel_id = "zulip", conversation_id = "engineering", thread_id = "alerts" }` for a specific topic).

## Session Continuity
* After completing a feature, fix, or any significant change, **update this AGENTS.md file** to reflect the new state of the codebase. Add or modify sections in "Project Overview" → "Source Layout" or "Runtime Flows" as needed so the next coding session can build context by scanning this file without exploring the codebase.
* When in doubt, include: what files changed, why, and how the change affects other modules or flows.
