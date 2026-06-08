# NerdBot

A minimal, self-hosted AI agent runtime written in Rust.

NerdBot communicates with users via **Telegram**, uses an **iterative tool-calling loop** driven by multiple LLM providers (OpenAI, Anthropic, Gemini, OpenRouter, and any OpenAI-compatible endpoint), and persists all conversation state and scheduled jobs in **SQLite**.

## Features

- **Telegram integration** — long-polling bot with chat/user allowlists and slash commands (`/help`, `/jobs`, `/run`, `/delete`, `/reset_context`)
- **Iterative tool loop** — the harness owns orchestration; the LLM proposes tool calls, Rust validates and executes them, and the loop continues until completion
- **Built-in tools** — scheduling, Telegram messaging, file I/O (sandboxed), web search (Exa), web fetch (with SSRF protection), shell execution (sandboxed)
- **Multiple LLM providers** — OpenAI, Anthropic, Gemini, OpenRouter, and arbitrary OpenAI-compatible endpoints via the `genai` crate
- **Scheduled & recurring jobs** — one-shot and cron-based tasks with configurable context policies
- **Automatic context compaction** — soft/hard token thresholds trigger background summarization so users never need to manually manage sessions
- **Single binary, Docker-friendly** — multi-stage build, non-root runtime user, no external services required
- **Telegram attachments** — photos, PDFs, and text documents are downloaded, validated (MIME types, magic bytes), and forwarded to the LLM as base64 or extracted text. Supported formats: JPEG, PNG, WebP, GIF, PDF, and text documents (txt, md, json, csv, html, xml, yaml, toml, py, js, sh, etc.).

## Quick Start

### Prerequisites

- Docker and Docker Compose (or a Rust toolchain for local development)
- A Telegram bot token ([@BotFather](https://t.me/BotFather))
- (Optional) An Exa API key for web search

### Docker Compose

```bash
# 1. Create configuration directories
mkdir -p config workspace

# 2. Copy example files
cp config.toml.example config/config.toml
cp personality.md.example config/personality.md

# 3. Set environment variables
export TELEGRAM_BOT_TOKEN="your-bot-token-here"
export EXA_API_KEY="your-exa-key-here"  # optional

# 4. Start
docker compose up -d
```

### Local Development

```bash
# 1. Generate config.toml through the guided onboarding flow
cargo run -- onboard

# 2. Set the environment variables named during onboarding
export TELEGRAM_BOT_TOKEN="your-bot-token-here"
export OPENAI_API_KEY="your-openai-key-here"  # when using OpenAI
export EXA_API_KEY="your-exa-key-here"  # optional

# 3. Run
cargo run
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
cargo run -- --diagnostics-socket /tmp/nerdbot/nerdbot.sock diagnostics show --chat-id 123456
cargo run -- --diagnostics-socket /tmp/nerdbot/nerdbot.sock diagnostics show --session-id <uuid> --json
```

#### Viewing Full Prompt and Spec Bodies

By default, the actual text of prompts and tool specs is hidden from the console rendering to prevent terminal noise. To inspect the full effective personality prompt (with timezone context), the compaction summary prompt, and the full JSON specs of all registered tools, add the `--show-prompts` flag:

```bash
cargo run -- --diagnostics-socket /tmp/nerdbot/nerdbot.sock diagnostics show --chat-id 123456 --show-prompts
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
| `[telegram]` | `bot_token_env` | Environment variable name for the Telegram bot token (default: `TELEGRAM_BOT_TOKEN`) |
| | `allowed_chat_ids` | List of allowed Telegram chat IDs (empty = all) |
| | `allowed_user_ids` | List of allowed Telegram user IDs (empty = all) |
| | `max_attachment_bytes` | Maximum download size for Telegram attachments in bytes (default: `5242880`, 5 MB) |
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
| | `sandbox_mode` | Isolation mode: `none`, `bwrap`, or `bwrap-strict` (default: `none`) |
| | `network_access` | Bubblewrap network policy: `disabled` or `host` (default: `disabled`) |
| `[exa]` | `api_key_env` | Environment variable for Exa API key |
| | `max_results` | Max web search results (default: `5`) |
| | `max_text_chars` | Max characters per fetched page (default: `8000`) |

## Telegram Commands

| Command | Description |
|---------|-------------|
| `/start` | Show bot info and available commands |
| `/help` | Show available commands and capabilities |
| `/jobs` | List all scheduled jobs |
| `/run <job-id>` | Immediately execute a scheduled job |
| `/delete <job-id>` | Delete a scheduled job |
| `/reset_context` | Reset the conversation context for the current chat |

## Architecture

```
Telegram (long polling)
    │
    ▼
MessageHandler ──► Allowlist check ──► Session lookup
    │
    ▼
ContextManager ──► Personality + tool specs + rolling summary + recent turns
    │
    ▼
AgentLoop ──► LLM call ──► Tool calls? ──► Harness executes tools ──► Repeat
    │
    ▼
Final text ──► Persist messages ──► Send to Telegram ──► Check compaction threshold
```

The Rust harness **owns orchestration**. The LLM proposes actions through tool calls. The harness validates arguments, executes them, persists results, and feeds them back — looping until the task is complete or a stop condition fires.

## Project Layout

```
src/
  main.rs              — CLI entry, long polling loop, service wiring
  config.rs            — TOML config loader
  onboarding.rs        — Interactive config.toml generator
  error.rs             — Error types
  agent/               — Agent loop and run modes
  llm/                 — LLM client (via genai crate)
  telegram/            — Telegram bot, commands, message handler
  scheduler/           — Cron/one-shot job service
  tools/               — Built-in tools (echo, calculator, files, web, shell, etc.)
  context/             — Context budgeting, compaction service & worker
  storage/             — SQLite persistence (sessions, messages, summaries, jobs)
  web/                 — Web fetcher with SSRF protection, search backend
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
| `TELEGRAM_BOT_TOKEN` | Yes | Telegram bot token from @BotFather |
| `OPENAI_API_KEY` | No | OpenAI API key (needed for default `gpt-4o` model) |
| `EXA_API_KEY` | No | Exa API key for web search |
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

A sample user service unit is available at [`docs/nerdbot-user.service.example`](docs/nerdbot-user.service.example). It assumes:

- binary: `~/.local/bin/nerdbot`
- config: `~/.config/nerdbot/config.toml`
- secrets env file: `~/.config/nerdbot/nerdbot.env`
- working/data directory: `~/.local/share/nerdbot`

Install it with:

```bash
mkdir -p ~/.config/systemd/user ~/.config/nerdbot ~/.local/bin ~/.local/share/nerdbot
cp target/release/nerdbot ~/.local/bin/nerdbot
cp docs/nerdbot-user.service.example ~/.config/systemd/user/nerdbot.service
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
