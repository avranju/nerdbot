CREATE TABLE chat_sessions (
    id TEXT PRIMARY KEY,
    channel_id TEXT NOT NULL,
    conversation_id TEXT NOT NULL,
    thread_id TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE INDEX idx_chat_sessions_address
    ON chat_sessions(channel_id, conversation_id, thread_id, created_at);

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
    owner_channel_id TEXT NOT NULL,
    owner_conversation_id TEXT NOT NULL,
    owner_thread_id TEXT,
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

CREATE INDEX idx_scheduled_jobs_owner
    ON scheduled_jobs(owner_channel_id, owner_conversation_id, owner_thread_id, next_run_at);
