//! Opt-in Unix domain socket server for live diagnostics.

use std::fs::Permissions;
use std::os::unix::fs::{FileTypeExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use sqlx::SqlitePool;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tracing::{debug, info, warn};

use crate::context::compaction_service::CompactionService;
use crate::diagnostics::protocol::{DiagnosticsRequest, DiagnosticsResponse, SessionDiagnostics};
use crate::error::AgentError;

const MAX_REQUEST_BYTES: u64 = 16 * 1024;

/// Running diagnostics server. Dropping it requests shutdown and removes the socket path.
pub struct DiagnosticsServer {
    socket_path: PathBuf,
    shutdown_tx: watch::Sender<bool>,
    task: Option<JoinHandle<()>>,
}

impl DiagnosticsServer {
    /// Bind and start a diagnostics server on an explicitly configured socket path.
    pub async fn start(
        socket_path: PathBuf,
        pool: SqlitePool,
        compaction_service: Arc<CompactionService>,
        config: crate::config::AppConfig,
        personality: crate::agent::personality::Personality,
        tools: Arc<crate::tools::registry::ToolRegistry>,
    ) -> Result<Self, AgentError> {
        prepare_socket_path(&socket_path).await?;
        let listener = UnixListener::bind(&socket_path).map_err(|e| {
            AgentError::Diagnostics(format!(
                "Failed to bind diagnostics socket {}: {e}",
                socket_path.display()
            ))
        })?;
        std::fs::set_permissions(&socket_path, Permissions::from_mode(0o600)).map_err(|e| {
            AgentError::Diagnostics(format!(
                "Failed to set diagnostics socket permissions for {}: {e}",
                socket_path.display()
            ))
        })?;

        let (shutdown_tx, mut shutdown_rx) = watch::channel(false);
        let task_socket_path = socket_path.clone();
        let config = Arc::new(config);
        let task_config = config.clone();
        let task_personality = personality.clone();
        let task_tools = tools.clone();
        let task = tokio::spawn(async move {
            info!(socket_path = %task_socket_path.display(), "diagnostics socket listening");
            loop {
                tokio::select! {
                    changed = shutdown_rx.changed() => {
                        if changed.is_err() || *shutdown_rx.borrow() {
                            break;
                        }
                    }
                    accepted = listener.accept() => {
                        match accepted {
                            Ok((stream, _)) => {
                                let pool = pool.clone();
                                let compaction_service = compaction_service.clone();
                                let config = task_config.clone();
                                let personality = task_personality.clone();
                                let tools = task_tools.clone();
                                tokio::spawn(async move {
                                    if let Err(e) = handle_connection(stream, &pool, &compaction_service, &config, &personality, &tools).await {
                                        warn!(error = %e, "diagnostics request failed");
                                    }
                                });
                            }
                            Err(e) => {
                                warn!(error = %e, "diagnostics socket accept failed");
                            }
                        }
                    }
                }
            }
            remove_socket_if_present(&task_socket_path);
            info!(socket_path = %task_socket_path.display(), "diagnostics socket stopped");
        });

        Ok(Self {
            socket_path,
            shutdown_tx,
            task: Some(task),
        })
    }

    /// Stop the server and remove its socket path.
    pub async fn stop(&mut self) {
        let _ = self.shutdown_tx.send(true);
        if let Some(task) = self.task.take() {
            let _ = task.await;
        }
        remove_socket_if_present(&self.socket_path);
    }
}

impl Drop for DiagnosticsServer {
    fn drop(&mut self) {
        let _ = self.shutdown_tx.send(true);
        remove_socket_if_present(&self.socket_path);
    }
}

async fn prepare_socket_path(socket_path: &Path) -> Result<(), AgentError> {
    let metadata = match std::fs::symlink_metadata(socket_path) {
        Ok(metadata) => metadata,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => {
            return Err(AgentError::Diagnostics(format!(
                "Failed to inspect diagnostics socket path {}: {e}",
                socket_path.display()
            )));
        }
    };

    if !metadata.file_type().is_socket() {
        return Err(AgentError::Diagnostics(format!(
            "Refusing to replace non-socket diagnostics path {}",
            socket_path.display()
        )));
    }

    if UnixStream::connect(socket_path).await.is_ok() {
        return Err(AgentError::Diagnostics(format!(
            "Diagnostics socket {} is already in use",
            socket_path.display()
        )));
    }

    std::fs::remove_file(socket_path).map_err(|e| {
        AgentError::Diagnostics(format!(
            "Failed to remove stale diagnostics socket {}: {e}",
            socket_path.display()
        ))
    })
}

async fn handle_connection(
    stream: UnixStream,
    pool: &SqlitePool,
    compaction_service: &CompactionService,
    config: &crate::config::AppConfig,
    personality: &crate::agent::personality::Personality,
    tools: &crate::tools::registry::ToolRegistry,
) -> Result<(), AgentError> {
    let (reader, mut writer) = stream.into_split();
    let mut reader = BufReader::new(reader).take(MAX_REQUEST_BYTES + 1);
    let mut line = String::new();
    let bytes_read = reader
        .read_line(&mut line)
        .await
        .map_err(|e| AgentError::Diagnostics(format!("Failed to read diagnostics request: {e}")))?;

    let response = if bytes_read == 0 {
        DiagnosticsResponse::Error {
            message: "Empty diagnostics request".to_string(),
        }
    } else if bytes_read as u64 > MAX_REQUEST_BYTES || !line.ends_with('\n') {
        DiagnosticsResponse::Error {
            message: format!("Diagnostics request exceeds {MAX_REQUEST_BYTES} bytes"),
        }
    } else {
        match serde_json::from_str::<DiagnosticsRequest>(line.trim_end()) {
            Ok(request) => {
                handle_request(
                    request,
                    pool,
                    compaction_service,
                    config,
                    personality,
                    tools,
                )
                .await
            }
            Err(e) => DiagnosticsResponse::Error {
                message: format!("Invalid diagnostics request: {e}"),
            },
        }
    };

    let response = serde_json::to_string(&response).map_err(|e| {
        AgentError::Diagnostics(format!("Failed to encode diagnostics response: {e}"))
    })?;
    writer.write_all(response.as_bytes()).await.map_err(|e| {
        AgentError::Diagnostics(format!("Failed to write diagnostics response: {e}"))
    })?;
    writer
        .write_all(b"\n")
        .await
        .map_err(|e| AgentError::Diagnostics(format!("Failed to write diagnostics response: {e}")))
}

async fn handle_request(
    request: DiagnosticsRequest,
    pool: &SqlitePool,
    compaction_service: &CompactionService,
    config: &crate::config::AppConfig,
    personality: &crate::agent::personality::Personality,
    tools: &crate::tools::registry::ToolRegistry,
) -> DiagnosticsResponse {
    match request {
        DiagnosticsRequest::Ping => DiagnosticsResponse::Pong,
        DiagnosticsRequest::ListSessions => {
            match crate::storage::sessions::list_sessions(pool).await {
                Ok(sessions) => DiagnosticsResponse::Sessions {
                    sessions: sessions.into_iter().map(SessionDiagnostics::from).collect(),
                },
                Err(e) => DiagnosticsResponse::Error {
                    message: e.to_string(),
                },
            }
        }
        DiagnosticsRequest::ShowSession {
            session_id,
            chat_id,
            include_prompts,
        } => {
            let input = ShowSessionInput {
                pool,
                compaction_service,
                config,
                personality,
                tools,
                session_id,
                chat_id,
                include_prompts,
            };
            show_session(input).await
        }
    }
}

struct ShowSessionInput<'a> {
    pool: &'a SqlitePool,
    compaction_service: &'a CompactionService,
    config: &'a crate::config::AppConfig,
    personality: &'a crate::agent::personality::Personality,
    tools: &'a crate::tools::registry::ToolRegistry,
    session_id: Option<String>,
    chat_id: Option<i64>,
    include_prompts: bool,
}

async fn show_session(input: ShowSessionInput<'_>) -> DiagnosticsResponse {
    let ShowSessionInput {
        pool,
        compaction_service,
        config,
        personality,
        tools,
        session_id,
        chat_id,
        include_prompts,
    } = input;

    let session = match (session_id, chat_id) {
        (Some(session_id), None) => crate::storage::sessions::get_session(pool, &session_id).await,
        (None, Some(chat_id)) => {
            crate::storage::sessions::get_session_for_chat(pool, chat_id).await
        }
        _ => {
            return DiagnosticsResponse::Error {
                message: "show_session requires exactly one of session_id or chat_id".to_string(),
            };
        }
    };
    let session = match session {
        Ok(Some(session)) => session,
        Ok(None) => {
            return DiagnosticsResponse::Error {
                message: "Session not found".to_string(),
            };
        }
        Err(e) => {
            return DiagnosticsResponse::Error {
                message: e.to_string(),
            };
        }
    };

    let context = match compaction_service.diagnostics_snapshot(&session.id).await {
        Ok(context) => context,
        Err(e) => {
            return DiagnosticsResponse::Error {
                message: e.to_string(),
            };
        }
    };
    let compaction_state = compaction_service.get_state(&session.id).await;

    let effective_personality_body = personality.effective_prompt(&config.agent.default_timezone);
    let personality_char_count = effective_personality_body.len();
    let personality_token_estimate = (personality_char_count / 4).max(1);

    let effective_personality = crate::diagnostics::protocol::PersonalityDiagnostics {
        char_count: personality_char_count,
        token_estimate: personality_token_estimate,
        timezone: config.agent.default_timezone.clone(),
        body: if include_prompts {
            Some(effective_personality_body)
        } else {
            None
        },
    };

    let compaction_prompt = match compaction_service.get_compaction_prompt(&session.id).await {
        Ok(p) => p,
        Err(e) => {
            return DiagnosticsResponse::Error {
                message: format!("Failed to build compaction prompt: {e}"),
            };
        }
    };
    let summary_char_count = compaction_prompt.len();
    let summary_token_estimate = (summary_char_count / 4).max(1);

    let summary_prompt = crate::diagnostics::protocol::SummaryPromptDiagnostics {
        char_count: summary_char_count,
        token_estimate: summary_token_estimate,
        body: if include_prompts {
            Some(compaction_prompt)
        } else {
            None
        },
    };

    let specs = tools.specs();
    let specs_json = serde_json::to_string(&specs).unwrap_or_default();
    let tool_token_estimate = (specs_json.len() / 4).max(1);

    let tool_spec = crate::diagnostics::protocol::ToolSpecDiagnostics {
        count: tools.len(),
        token_estimate: tool_token_estimate,
        specs: if include_prompts {
            Some(
                specs
                    .iter()
                    .map(|s| serde_json::to_value(s).unwrap_or(serde_json::Value::Null))
                    .collect(),
            )
        } else {
            None
        },
    };

    debug!(session_id = %session.id, "served diagnostics session snapshot");
    DiagnosticsResponse::Session {
        session: session.into(),
        context: Box::new(context),
        compaction_state,
        effective_personality: Box::new(effective_personality),
        summary_prompt: Box::new(summary_prompt),
        tool_spec: Box::new(tool_spec),
    }
}

fn remove_socket_if_present(socket_path: &Path) {
    match std::fs::symlink_metadata(socket_path) {
        Ok(metadata) if metadata.file_type().is_socket() => {
            if let Err(e) = std::fs::remove_file(socket_path) {
                warn!(socket_path = %socket_path.display(), error = %e, "failed to remove diagnostics socket");
            }
        }
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => {
            warn!(socket_path = %socket_path.display(), error = %e, "failed to inspect diagnostics socket during cleanup");
        }
    }
}
