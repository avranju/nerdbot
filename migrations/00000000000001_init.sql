CREATE TABLE chat_sessions (
    id TEXT PRIMARY KEY,
    telegram_chat_id INTEGER NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE TABLE messages (
    id TEXT PRIMARY KEY,
    chat_session_id TEXT NOT NULL REFERENCES chat_sessions(id),
    role TEXT NOT NULL,
    content TEXT NOT NULL,
    structured_content_json TEXT,
    token_estimate INTEGER,
    created_at TEXT NOT NULL
);

CREATE TABLE context_summaries (
    id TEXT PRIMARY KEY,
    chat_session_id TEXT NOT NULL REFERENCES chat_sessions(id),
    summary_text TEXT NOT NULL,
    covers_through_message_id TEXT NOT NULL,
    created_at TEXT NOT NULL
);

CREATE TABLE scheduled_jobs (
    id TEXT PRIMARY KEY,
    owner_chat_id INTEGER NOT NULL,
    name TEXT NOT NULL,
    prompt TEXT NOT NULL,
    schedule_type TEXT NOT NULL,
    cron_expression TEXT,
    run_at TEXT,
    timezone TEXT,
    notify_on_completion INTEGER NOT NULL DEFAULT 0,
    context_policy TEXT NOT NULL,
    creation_context_snapshot TEXT,
    enabled INTEGER NOT NULL DEFAULT 1,
    last_run_at TEXT,
    next_run_at TEXT,
    last_status TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
