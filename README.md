# NerdBot

A minimal, self-hosted AI agent runtime written in Rust.

NerdBot communicates with users through pluggable **communication channels**. Both **Telegram** and **Zulip** are supported channel adapters, each offering long-polling and webhook ingress modes. NerdBot uses an **iterative tool-calling loop** driven by multiple LLM providers (OpenAI, Anthropic, Gemini, OpenRouter, and any OpenAI-compatible endpoint), and persists all conversation state and scheduled jobs in **SQLite**.

## Features

- **Communication channels** — pluggable adapters for Telegram and Zulip, each supporting long-polling and webhook ingress with channel-qualified access control
- **Iterative tool loop** — the harness owns orchestration; the LLM proposes tool calls, Rust validates and executes them, and the loop continues until completion
- **Built-in tools** — scheduling, user messaging, file I/O (sandboxed), web search (Exa), web fetch (with SSRF protection), shell execution (sandboxed)
- **Multiple LLM providers** — OpenAI, Anthropic, Gemini, OpenRouter, and arbitrary OpenAI-compatible endpoints via the `genai` crate
- **Scheduled & recurring jobs** — one-shot and cron-based tasks with configurable context policies
- **Automatic context compaction** — soft/hard token thresholds trigger background summarization so users never need to manually manage sessions
- **Maintenance mode** — temporarily disable interactive messages and scheduled jobs with a configurable reason text; authorized users see only the maintenance response
- **Config hot reload** — NerdBot watches `config.toml` and restarts the runtime when the file changes to a valid configuration; invalid reloads are logged and the current runtime remains active
- **Single binary, Docker-friendly** — multi-stage build, non-root runtime user, no external services required
- **Telegram attachments** — photos, PDFs, and text documents are downloaded, validated (MIME types, magic bytes), and forwarded to the LLM as base64 or extracted text. Supported formats: JPEG, PNG, WebP, GIF, PDF, and text documents (txt, md, json, csv, html, xml, yaml, toml, py, js, sh, etc.).
- **Zulip attachments** — user-uploaded files embedded as markdown links in Zulip messages are extracted, downloaded via authenticated API calls, and forwarded to the LLM as base64 (images) or extracted text (documents). Supported formats: images (JPEG, PNG, WebP, GIF), PDFs, and text documents (txt, md, json, csv, html, xml, yaml, toml, py, js, sh, rs, etc.).

## Quickstart

Use this path for a quick local test that gets NerdBot running and replying in Telegram. Docker is better suited for a production-style run; start here first if you just want to see the project work.

1. Create a Telegram bot with [@BotFather](https://t.me/BotFather), then copy the bot token it gives you.

2. Get an LLM API key for the provider you want to use, such as OpenAI, Anthropic, Gemini, OpenRouter, or a custom OpenAI-compatible endpoint. If you want to test web search, also [create an Exa API key](https://exa.ai/docs/reference/getting-started).

```bash
# 3. Create local runtime folders
mkdir -p workspace data
cp personality.md.example personality.md

# 4. Generate config.toml through the guided onboarding flow
cargo run -- onboard
```

During onboarding:

- Choose `poll` for Telegram ingress mode.
- Enter the environment variable name that will hold your Telegram token, usually `TELEGRAM_BOT_TOKEN`.
- (Optional) Enable the Zulip channel and configure its ingress mode, bot email/API key environment variables, Zulip server URL, and access allowlists.
- Choose your LLM provider and model.
- Enter the environment variable name for your LLM API key, such as `OPENAI_API_KEY`, when your provider requires one.
- Enter `EXA_API_KEY` for Exa if you want web search, or leave it blank if not.

After onboarding, make sure `config.toml` points at the local paths you created:

```toml
[agent]
personality_file = "personality.md"

[storage]
sqlite_path = "data/agent.db"

[workspace]
root = "workspace"
```

Then export the secrets named in `config.toml` and start the bot:

```bash
export TELEGRAM_BOT_TOKEN="your-telegram-bot-token"
export OPENAI_API_KEY="your-llm-api-key"     # use the env var/provider you configured
export EXA_API_KEY="your-exa-api-key"        # optional
# For Zulip (if enabled):
# export ZULIP_BOT_EMAIL="name-bot@org.zulipchat.com"
# export ZULIP_BOT_API_KEY="your-32-char-hex-key"
cargo run
```

Open Telegram, send a message to your bot, and you should get a reply.

## Running NerdBot

### Prerequisites

- A Rust toolchain for local development, or Docker and Docker Compose for deployment
- A Telegram bot token ([@BotFather](https://t.me/BotFather))
- (Optional) A Zulip bot email and API key — create a bot user in your Zulip organization and generate an API key via the Zulip web UI
- (Optional) An Exa API key for web search

### Local Development

```bash
# 1. Generate config.toml through the guided onboarding flow
cargo run -- onboard

# 2. Set the environment variables named during onboarding
export TELEGRAM_BOT_TOKEN="your-bot-token-here"
export OPENAI_API_KEY="your-openai-key-here"  # when using OpenAI
export EXA_API_KEY="your-exa-key-here"  # optional
# For Zulip (if enabled):
# export ZULIP_BOT_EMAIL="name-bot@org.zulipchat.com"
# export ZULIP_BOT_API_KEY="your-32-char-hex-key"

# 3. Run
cargo run
```

### Docker Compose

Use Docker Compose when you want a production-style run with mounted config, persistent data, and an isolated workspace.

```bash
# 1. Create configuration directories
mkdir -p config workspace

# 2. Copy example files
cp config.toml.example config/config.toml
cp personality.md.example config/personality.md

# 3. Set environment variables
export TELEGRAM_BOT_TOKEN="your-bot-token-here"
export EXA_API_KEY="your-exa-key-here"  # optional
# For Zulip (if enabled):
# export ZULIP_BOT_EMAIL="name-bot@org.zulipchat.com"
# export ZULIP_BOT_API_KEY="your-32-char-hex-key"

# 4. Start
docker compose up -d
```

### Local Diagnostics Socket

Diagnostics are disabled by default. To expose a local owner-only Unix domain socket for live process state, context snapshots, and prompt/tool spec metadata, pass an explicit socket path:

```bash
cargo run -- --diagnostics-socket /tmp/nerdbot/nerdbot.sock
```

Query the running instance with the same socket path:

```bash
cargo run -- --diagnostics-socket /tmp/nerdbot/nerdbot.sock diagnostics ping
cargo run -- --diagnostics-socket /tmp/nerdbot/nerdbot.sock diagnostics list-sessions
cargo run -- --diagnostics-socket /tmp/nerdbot/nerdbot.sock diagnostics show --channel-id telegram --conversation-id 123456
cargo run -- --diagnostics-socket /tmp/nerdbot/nerdbot.sock diagnostics show --session-id <uuid> --json
```

#### Viewing Full Prompt and Spec Bodies

By default, the actual text of prompts and tool specs is hidden from the console rendering to prevent terminal noise. To inspect the full effective personality prompt (with timezone context), the compaction summary prompt, and the full JSON specs of all registered tools, add the `--show-prompts` flag:

```bash
cargo run -- --diagnostics-socket /tmp/nerdbot/nerdbot.sock diagnostics show --channel-id telegram --conversation-id 123456 --show-prompts
```

#### Docker Diagnostics Mount

When running NerdBot inside a Docker container with diagnostics enabled, you must bind-mount a writable directory from the host to house the Unix socket file. Because NerdBot runs as a non-root user (`nerdbot`, UID `1000` / GID `1000`), the directory mounted from the host must be writable by UID `1000`.

Update your `docker-compose.yml` to specify a socket path inside a mounted directory:

```yaml
services:
  nerdbot:
    image: nerdbot:latest
    user: "1000:1000"
    command: ["--config", "/config/config.toml", "--diagnostics-socket", "/var/run/nerdbot/nerdbot.sock"]
    volumes:
      - ./config:/config:ro
      - ./data:/data:rw
      - ./workspace:/workspace:rw
      - ./run:/var/run/nerdbot:rw  # Mount writable directory for Unix domain socket
```

Before starting the container, create and set correct permissions on the host path:

```bash
mkdir -p ./run
chown -R 1000:1000 ./run
chmod 700 ./run
```

## Configuration

Run `cargo run -- onboard` for an interactive setup flow, or copy `config.toml.example` to `config.toml` and adjust it manually. When the config file already exists, onboarding uses its current values as prompt defaults and preserves settings outside the guided flow. Pass `--config <path>` before the subcommand to generate or edit a different file, for example `cargo run -- --config config/local.toml onboard`. All secrets are read from **environment variables**, never from the config file.

| Section | Key | Description |
|---------|-----|-------------|
| `[agent]` | `name` | Display name for the bot |
| | `personality_file` | Path to the personality Markdown file (default: `/config/personality.md`) |
| | `max_tool_iterations` | Maximum tool-call loop iterations per request (default: `10`) |
| | `default_timezone` | Default IANA timezone for agent behavior |
| `[maintenance]` | `enabled` | Enable maintenance mode — rejects all interactive messages and scheduled jobs with a maintenance response (default: `false`) |
| | `reason` | Optional human-readable reason shown to users (max 1000 characters; empty = no reason line) |
| `[webhook]` | `host` | Local plain-HTTP shared webhook bind host when any channel uses webhook ingress (default: `127.0.0.1`; use a reverse proxy or tunnel for public TLS) |
| | `port` | Local plain-HTTP shared webhook bind port when any channel uses webhook ingress (default: `24682`) |
| `[channels.telegram]` | `enabled` | Enable the Telegram channel adapter (default: `true`) |
| | `ingress` | Telegram ingress transport: `poll` or `webhook` (default: `poll`) |
| | `bot_token_env` | Environment variable name for the Telegram bot token (default: `TELEGRAM_BOT_TOKEN`) |
| | `poll_interval_secs` | Seconds to sleep after an empty Telegram `getUpdates` response in poll mode (default: `5`) |
| | `web_hook_url` | Public HTTPS webhook URL required when `ingress = "webhook"` |
| | `allowed_conversations` | List of Telegram conversation/chat IDs as strings (empty = all). Internally these become channel-qualified allowlist patterns for `channel_id = "telegram"` |
| | `allowed_senders` | List of Telegram sender/user IDs as strings (empty = all) |
| | `max_attachment_bytes` | Maximum download size for Telegram attachments in bytes (default: `5242880`, 5 MB) |
| | `max_text_document_chars` | Max characters when extracting text from text documents (default: `32768`, 32 KB) |
| `[channels.zulip]` | `enabled` | Enable the Zulip channel adapter (default: `false`) |
| | `ingress` | Zulip ingress transport: `poll` (event queue) or `webhook` (outgoing webhook) (default: `poll`) |
| | `bot_email_env` | Environment variable name for the Zulip bot email (default: `ZULIP_BOT_EMAIL`) |
| | `api_key_env` | Environment variable name for the Zulip bot API key (default: `ZULIP_BOT_API_KEY`) |
| | `site_url` | Base URL of the Zulip server (e.g., `https://your-org.zulipchat.com`); required when enabled |
| | `web_hook_token_env` | Environment variable name for the webhook verification token (default: `ZULIP_WEBHOOK_TOKEN`) |
| | `poll_interval_secs` | Seconds to sleep after an empty Zulip events response in poll mode (default: `2`) |
| | `presence_enabled` | Experimental active presence heartbeat. Current Zulip servers reject presence updates from bot accounts, so this defaults to `false` |
| | `presence_ping_interval_secs` | Seconds between Zulip active presence heartbeats when explicitly enabled (default: `60`) |
| | `allowed_conversations` | List of `ConversationAddressPattern` objects with `channel_id`, `conversation_id` (stream name), and optional `thread_id` (topic name). Empty = all streams (default: `[]`) |
| | `allowed_senders` | List of Zulip sender email addresses (empty = all) |
| | `max_attachment_bytes` | Maximum download size for Zulip attachments in bytes (default: `5242880`, 5 MB) |
| | `max_text_document_chars` | Max characters when extracting text from text documents (default: `32768`, 32 KB) |
| `[storage]` | `sqlite_path` | Path to the SQLite database file |
| `[workspace]` | `root` | Sandbox root for file tools |
| | `max_read_bytes` | Max bytes for file reads (default: `262144`) |
| | `max_write_bytes` | Max bytes for file writes (default: `262144`) |
| `[files]` | `max_read_bytes` | Max bytes for file tool reads (default: `262144`) |
| | `max_write_bytes` | Max bytes for file tool writes (default: `262144`) |
| `[llm]` | `model` | Model name (genai resolves provider from prefix) |
| | `endpoint` | Optional custom API endpoint |
| | `api_key_env` | Optional env var for API key override |
| | `temperature` | Sampling temperature (default: `0.2`) |
| | `max_output_tokens` | Max output tokens (default: `4096`) |
| | `max_retries` | Retries after transient LLM network/server failures (default: `4`) |
| | `retry_interval_secs` | Seconds between transient LLM retry attempts (default: `10`) |
| `[context]` | `soft_compaction_threshold` | Fraction that triggers background compaction (default: `0.60`) |
| | `hard_context_threshold` | Fraction that forces context bounding (default: `0.85`) |
| | `recent_turns_to_preserve` | Raw turns kept after summary (default: `30`) |
| | `reserved_tool_loop_tokens` | Tokens reserved for tool-loop headroom (default: `8192`) |
| `[llm]` | `context_window_tokens` | Total context window size in tokens (default: `128000`) |
| `[scheduler]` | `run_overdue_one_shots_on_startup` | Run missed one-shot jobs on startup (default: `false`) |
| `[shell]` | `allowed_commands` | Allowlist for shell execution (empty = allow all) |
| | `denied_commands` | Always-blocked commands |
| | `max_output_bytes` | Max shell output size (default: `1048576`) |
| | `timeout_secs` | Shell command timeout (default: `30`) |
| | `sandbox_mode` | Isolation mode: `none`, `bwrap`, or `bwrap-strict` (default: `none`; `none` is direct host execution, not a security sandbox) |
| | `network_access` | Bubblewrap network policy: `disabled` or `host` (default: `disabled`) |
| `[exa]` | `api_key_env` | Environment variable for Exa API key |
| | `max_results` | Max web search results (default: `5`) |
| | `max_text_chars` | Max characters per fetched page (default: `8000`) |

## Maintenance Mode

Set `maintenance.enabled = true` in `config.toml` to temporarily suspend all interactive user messages and scheduled jobs. While maintenance mode is active:

- Authorized user messages and slash commands receive only the maintenance response (default banner with optional reason)
- No chat sessions are created or persisted for maintenance-mode interactions
- No LLM calls are made
- Telegram and Zulip attachments are not downloaded
- The scheduler does not start new job executions

The maintenance response includes an optional `reason` field from the config:

```toml
[maintenance]
enabled = true
reason = "Database migration in progress"
```

When `reason` is empty or whitespace-only, only the default maintenance banner is returned.

### Config Hot Reload

NerdBot watches `config.toml` and restarts the runtime when the file changes to a valid configuration:

- Editing `config.toml` with `maintenance.enabled = true` while the bot is running triggers a graceful restart; the new runtime starts in maintenance mode
- Setting `maintenance.enabled = false` and saving the file causes a restart that restores normal behavior
- If the new config is invalid (bad TOML, failed validation), the error is logged and the current runtime remains active — no restart occurs
- Invalid recovery (new config fails and old config also fails to restart) causes the process to exit

This means you can toggle maintenance mode on-the-fly without stopping the process.

## Channel Commands

Commands work identically across both Telegram and Zulip. In Zulip streams, prefix commands with a bot mention (e.g., `@**NerdBot** /help`).

| Command | Description |
|---------|-------------|
| `/start` | Show bot info and available commands |
| `/help` | Show available commands and capabilities |
| `/jobs` | List all scheduled jobs |
| `/run <job-id>` | Immediately execute a scheduled job |
| `/delete <job-id>` | Delete a scheduled job |
| `/reset_context` | Reset the conversation context for the current channel conversation |
| `/new_topic` | Alias for `/reset_context` |

## Zulip Integration

NerdBot supports Zulip as a full-featured communication channel alongside Telegram. Zulip messages are mapped to NerdBot's generic conversation model:

- **Stream messages** — `conversation_id` = stream name, `thread_id` = topic name
- **Private messages** — `conversation_id` = sorted comma-separated participant email addresses

### Zulip Setup

1. Create a bot user in your Zulip organization and generate an API key from the bot's settings page.
2. Note the bot's email address (e.g., `name-bot@org.zulipchat.com`).
3. Enable the Zulip channel in `config.toml`:

```toml
[channels.zulip]
enabled = true
ingress = "poll"  # or "webhook" for push-based delivery
bot_email_env = "ZULIP_BOT_EMAIL"
api_key_env = "ZULIP_BOT_API_KEY"
site_url = "https://your-org.zulipchat.com"
# web_hook_token_env = "ZULIP_WEBHOOK_TOKEN"  # required when ingress = "webhook"
allowed_conversations = [
    { channel_id = "zulip", conversation_id = "general" },                    # All topics in "general"
    { channel_id = "zulip", conversation_id = "engineering", thread_id = "alerts" },  # Only "alerts" topic
]
allowed_senders = []  # Empty = allow all senders
```

4. Export the Zulip credentials:

```bash
export ZULIP_BOT_EMAIL="name-bot@org.zulipchat.com"
export ZULIP_BOT_API_KEY="your-32-char-hex-key"
```

### Zulip Ingress Modes

**Poll mode** (default): NerdBot registers a Zulip event queue via `POST /api/v1/register` and polls `GET /api/v1/events` for new messages. The queue automatically refreshes when it expires.

**Webhook mode**: NerdBot runs a shared local HTTP server configured by `[webhook]` that receives channel webhooks and exposes `/health`. Zulip outgoing webhooks should post to `/zulip/hook`. If Telegram webhook mode is enabled too, both Telegram and Zulip routes are served from the same `webhook.host`/`webhook.port`, so one reverse-proxy vhost can forward to one local listener. For example, use `https://nerdbot.example.com/telegram/hook` for Telegram and `https://nerdbot.example.com/zulip/hook` for Zulip. NerdBot acknowledges accepted Zulip webhook requests with `{"response_not_required": true}` and sends the actual bot reply asynchronously through Zulip's REST API.

### Zulip Attachments

Zulip embeds uploaded files as markdown links in message content (e.g., `[report.pdf](/user_uploads/1/99/abc/report.pdf)`). NerdBot automatically:

1. Extracts these links using regex pattern matching
2. Downloads files via the authenticated Zulip API
3. Classifies them as binary (images) or text documents
4. Forwards images as base64-encoded content and text documents as extracted text to the LLM
5. Replaces attachment links in the message text with `[Attachment: filename]` placeholders

### Zulip Typing Indicators

Zulip typing indicators are supported for direct messages only (not streams). The bot caches numeric participant user IDs from inbound direct-message payloads because Zulip's typing endpoint requires user IDs and `type = "direct"`, while message sending can still use email recipients. NerdBot sends a typing refresh every 8 seconds while generating a response and sends `op = "stop"` when the agent run finishes.

### Zulip Presence

Zulip's presence endpoint currently rejects bot-account API requests with `This endpoint does not accept bot requests.` NerdBot therefore leaves presence heartbeats disabled by default, and a bot user may still appear offline even while NerdBot is running. The experimental `presence_enabled` setting remains available, but the heartbeat stops automatically if Zulip returns that bot-account rejection.

For a dedicated human/service account using long polling, presence updates can work. NerdBot resolves the authenticated account's Zulip user ID from `/users/me` and uses that stable ID to ignore its own outbound messages, which avoids reply loops when Zulip event email addresses differ from the configured login email.

### Zulip Message Limits

Zulip has a 10,000-character message limit. NerdBot automatically splits long responses into multiple messages when sending through the Zulip channel.

## Architecture

```
Channel ingress (Telegram/Zulip polling or webhook push)
    │
    ▼
ChannelMessageHandler ──► Channel access policy ──► Session lookup
    │
    ▼
ContextManager ──► Personality + tool specs + rolling summary + recent turns
    │
    ▼
AgentLoop ──► LLM call ──► Tool calls? ──► Harness executes tools ──► Repeat
    │
    ▼
Final text ──► Persist messages ──► Send through ChannelRegistry ──► Check compaction threshold
```

The Rust harness **owns orchestration**. The LLM proposes actions through tool calls. The harness validates arguments, executes them, persists results, and feeds them back — looping until the task is complete or a stop condition fires.

## Project Layout

```
src/
  main.rs              — CLI entry, runtime construction, channel wiring, scheduler/diagnostics startup, ingress dispatch
  config.rs            — TOML config loader
  onboarding.rs        — Interactive config.toml generator
  error.rs             — Error types
  agent/               — Agent loop and run modes
  channel/             — Generic channel types, access policy, registry, and handler
  llm/                 — LLM client (via genai crate)
  telegram/            — Telegram bot, commands, service adapter, ingress, and attachments
  zulip/               — Zulip bot, service adapter, attachment processing, ingress (poll + webhook)
  scheduler/           — Cron/one-shot job service
  tools/               — Built-in tools (echo, calculator, files, web, shell, etc.)
  context/             — Context budgeting, compaction service & worker
  storage/             — SQLite persistence (sessions, messages, summaries, jobs)
  web/                 — Web fetcher with SSRF protection, search backend
  webhook.rs           — Shared Axum webhook server for Telegram/Zulip push ingress and /health
  workspace/           — Path sandboxing for file tools
migrations/            — SQLx database migrations
tests/                 — Integration tests
```

## Docker Deployment

### Directory Structure

```
your-project/
├── config/
│   ├── config.toml          # Application configuration
│   └── personality.md       # Agent personality / system prompt
├── workspace/               # File tool sandbox root (mounted as volume)
├── docker-compose.yml       # Docker Compose configuration
└── Dockerfile               # Multi-stage build (or use the one in this repo)
```

### Environment Variables

| Variable | Required | Description |
|----------|----------|-------------|
| `TELEGRAM_BOT_TOKEN` | Yes* | Telegram bot token from @BotFather (*required if Telegram is enabled) |
| `OPENAI_API_KEY` | No | OpenAI API key (needed for default `gpt-4o` model) |
| `EXA_API_KEY` | No | Exa API key for web search |
| `ZULIP_BOT_EMAIL` | No* | Zulip bot email address (*required if Zulip is enabled) |
| `ZULIP_BOT_API_KEY` | No* | Zulip bot API key (32-char hex string; *required if Zulip is enabled) |
| `ZULIP_WEBHOOK_TOKEN` | No* | Zulip webhook verification token (*required if Zulip webhook ingress is enabled) |
| `RUST_LOG` | No | Log level (e.g., `info`, `debug`, `warn`) |

Any provider-specific API keys should also be set as environment variables and referenced via `api_key_env` in the config.

### Health Checks

For basic liveness, you can check the container status:

```bash
docker compose ps
docker compose logs -f nerdbot
```

## Building from Source

```bash
# With SQLite dev headers installed
cargo build --release

# Run
./target/release/nerdbot --config config.toml
```

## Systemd User Service

A sample user service unit is available at [`nerdbot-user.service.example`](nerdbot-user.service.example). It assumes:

- binary: `~/.local/bin/nerdbot`
- config: `~/.config/nerdbot/config.toml`
- secrets env file: `~/.config/nerdbot/nerdbot.env`
- working/data directory: `~/.local/share/nerdbot`

Install it with:

```bash
mkdir -p ~/.config/systemd/user ~/.config/nerdbot ~/.local/bin ~/.local/share/nerdbot
cp target/release/nerdbot ~/.local/bin/nerdbot
cp nerdbot-user.service.example ~/.config/systemd/user/nerdbot.service
systemctl --user daemon-reload
systemctl --user enable --now nerdbot.service
journalctl --user -u nerdbot.service -f
```

To let the service continue after logout, run `loginctl enable-linger "$USER"` once.

### Cross-compilation

The Dockerfile uses `rust:1.96-slim-bookworm` as the build stage. For cross-compilation (e.g., `aarch64`), use Docker Buildx:

```bash
docker buildx build --platform linux/arm64 -t nerdbot:latest .
```

## License

[MIT](LICENSE)
