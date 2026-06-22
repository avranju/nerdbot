//! Config file watcher — monitors config.toml for changes.
//!
/// Watches the parent directory of the configured config file non-recursively,
/// filters create/content-modify/name-modify events for the configured path,
/// and sends `ConfigChange` events through a `tokio::sync::mpsc` channel.
use std::path::{Path, PathBuf};

use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use tokio::sync::mpsc;

use crate::error::AgentError;

/// Signal that the watched config path may have changed.
#[derive(Debug, Clone)]
pub struct ConfigChange {
    pub path: PathBuf,
}

/// Owns the notify `RecommendedWatcher` so config watching remains active.
pub struct ConfigWatcher {
    _watcher: RecommendedWatcher,
}

impl ConfigWatcher {
    /// Start watching the config file's parent directory and return the
    /// watcher plus a receiver for `ConfigChange` events.
    ///
    /// Returns `AgentError::Config` if the watcher cannot be created or the
    /// parent directory cannot be watched.
    pub fn watch(
        config_path: PathBuf,
    ) -> Result<(Self, mpsc::UnboundedReceiver<ConfigChange>), AgentError> {
        // Normalize to an absolute path so that notify's absolute event paths
        // (e.g. `$PWD/./config.toml`) are comparable with the watched path.
        let watched_path = normalize_path(&config_path);
        let (tx, rx) = mpsc::unbounded_channel();

        let mut watcher = match RecommendedWatcher::new(
            move |result: notify::Result<Event>| {
                let event = match result {
                    Ok(event) => event,
                    Err(e) => {
                        tracing::warn!(error = %e, "config file watcher error");
                        return;
                    }
                };

                if !is_config_reload_event(&event, &watched_path) {
                    return;
                }

                if let Err(e) = tx.send(ConfigChange {
                    path: watched_path.clone(),
                }) {
                    tracing::warn!(error = %e, "config file watcher send failed");
                }
            },
            notify::Config::default(),
        ) {
            Ok(watcher) => watcher,
            Err(e) => {
                return Err(AgentError::Config(format!(
                    "Failed to create config file watcher: {e}"
                )));
            }
        };

        let watch_dir = config_path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."));

        if let Err(e) = watcher.watch(&watch_dir, RecursiveMode::NonRecursive) {
            return Err(AgentError::Config(format!(
                "Failed to watch config file directory {}: {e}",
                watch_dir.display()
            )));
        }

        Ok((Self { _watcher: watcher }, rx))
    }
}

/// Normalize a path to an absolute, `.`-free path for reliable comparison
/// against notify event paths.
fn normalize_path(path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .ok()
            .map(|base| base.join(path).components().collect::<PathBuf>())
            .unwrap_or_else(|| path.to_path_buf())
    }
}

/// Filter notify events to only relevant config create/modify/rename events.
fn is_config_reload_event(event: &Event, watched_path: &Path) -> bool {
    let is_content_change = matches!(
        event.kind,
        EventKind::Create(_)
            | EventKind::Modify(
                notify::event::ModifyKind::Any
                    | notify::event::ModifyKind::Data(_)
                    | notify::event::ModifyKind::Name(_)
                    | notify::event::ModifyKind::Other
            )
    );
    if !is_content_change {
        return false;
    }

    event.paths.iter().any(|path| {
        let normalized = normalize_path(path);
        normalized == *watched_path
    })
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use notify::event::{AccessKind, CreateKind, DataChange, MetadataKind, ModifyKind};

    use super::*;

    #[test]
    fn filter_ignores_access_events() {
        let watched = PathBuf::from("/tmp/config.toml");
        let event = Event::new(EventKind::Access(AccessKind::Read)).add_path(watched.clone());
        assert!(!is_config_reload_event(&event, &watched));
    }

    #[test]
    fn filter_ignores_unrelated_paths() {
        let watched = PathBuf::from("/tmp/config.toml");
        let event = Event::new(EventKind::Modify(ModifyKind::Data(DataChange::Content)))
            .add_path(PathBuf::from("/tmp/other.toml"));
        assert!(!is_config_reload_event(&event, &watched));
    }

    #[test]
    fn filter_ignores_metadata_only_modify_events() {
        let watched = PathBuf::from("/tmp/config.toml");
        let event = Event::new(EventKind::Modify(ModifyKind::Metadata(MetadataKind::Any)))
            .add_path(watched.clone());
        assert!(!is_config_reload_event(&event, &watched));
    }

    #[test]
    fn filter_accepts_create_or_modify_on_watched_path() {
        let watched = PathBuf::from("/tmp/config.toml");
        let create = Event::new(EventKind::Create(CreateKind::File)).add_path(watched.clone());
        let modify = Event::new(EventKind::Modify(ModifyKind::Data(DataChange::Content)))
            .add_path(watched.clone());

        assert!(is_config_reload_event(&create, &watched));
        assert!(is_config_reload_event(&modify, &watched));
    }

    #[test]
    fn filter_accepts_absolute_event_path_matching_watched_path() {
        // Simulates notify reporting an absolute path when the watched path
        // was also resolved to absolute.
        let watched = PathBuf::from("/tmp/config.toml");
        let event = Event::new(EventKind::Modify(ModifyKind::Data(DataChange::Content)))
            .add_path(PathBuf::from("/tmp/config.toml"));
        assert!(is_config_reload_event(&event, &watched));
    }

    #[test]
    fn normalize_path_converts_relative_to_absolute() {
        let original = std::env::current_dir().unwrap().join("config.toml");
        let relative = PathBuf::from("config.toml");
        let normalized = normalize_path(&relative);
        assert_eq!(normalized, original);
    }

    #[tokio::test]
    async fn watcher_sends_change_on_file_write() {
        let temp = tempfile::tempdir().unwrap();
        let config_path = temp.path().join("config.toml");
        std::fs::write(&config_path, "[agent]\nname = \"test\"\n").unwrap();

        let (_watcher, mut rx) = ConfigWatcher::watch(config_path.clone()).unwrap();
        // Keep _watcher alive so the underlying notify watcher remains active
        // Give the watcher time to initialize
        tokio::time::sleep(Duration::from_millis(100)).await;

        // Write again to trigger an event
        std::fs::write(&config_path, "[agent]\nname = \"updated\"\n").unwrap();

        // Drain events within a timeout
        let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
        let mut found = false;
        while tokio::time::Instant::now() < deadline {
            if rx.try_recv().is_ok() {
                found = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }

        assert!(found, "Expected a ConfigChange event after file write");
    }
}
