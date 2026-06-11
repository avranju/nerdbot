//! Cached personality prompt loader with filesystem change notifications.

use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use tracing::{debug, warn};

use crate::agent::system_prompt::append_timezone_context;
use crate::config::AppConfig;

const DEFAULT_PERSONALITY: &str = "You are NerdBot, a helpful and concise AI assistant. \
    You respond in plain text. You use tools when they would help \
    answer the user's question more accurately. \
    When you don't know something, you say so honestly.";

/// In-memory cache for the configured personality file.
///
/// Clones share the same cache and filesystem watcher. Reads are served from
/// memory; the watcher refreshes the cache when the configured file's parent
/// directory reports a change.
#[derive(Clone)]
pub struct Personality {
    inner: Arc<PersonalityInner>,
}

struct PersonalityInner {
    path: PathBuf,
    contents: Arc<RwLock<String>>,
    _watcher: Option<RecommendedWatcher>,
}

impl Personality {
    /// Load the personality prompt from the configured file and start watching
    /// for changes.
    pub fn from_config(config: &AppConfig) -> Self {
        Self::new(config.agent.personality_file.clone())
    }

    /// Load the personality prompt from `path` and start watching for changes.
    pub fn new(path: PathBuf) -> Self {
        let contents = Arc::new(RwLock::new(load_contents(&path)));
        let watcher = start_watcher(path.clone(), contents.clone());

        Self {
            inner: Arc::new(PersonalityInner {
                path,
                contents,
                _watcher: watcher,
            }),
        }
    }

    /// Return the cached raw personality prompt.
    pub fn contents(&self) -> String {
        self.inner
            .contents
            .read()
            .map(|contents| contents.clone())
            .unwrap_or_else(|poisoned| poisoned.into_inner().clone())
    }

    /// Return the cached prompt with runtime timezone context appended.
    pub fn effective_prompt(&self, timezone: &str) -> String {
        append_timezone_context(&self.contents(), timezone)
    }

    /// Return the configured source file path.
    pub fn path(&self) -> &Path {
        &self.inner.path
    }
}

fn start_watcher(path: PathBuf, contents: Arc<RwLock<String>>) -> Option<RecommendedWatcher> {
    let watch_dir = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));

    let watched_path = path.clone();
    let mut watcher = match notify::recommended_watcher(
        move |result: notify::Result<notify::Event>| {
            match result {
                Ok(_) => {}
                Err(e) => {
                    warn!(error = %e, "personality file watcher error");
                    return;
                }
            }

            let reloaded = load_contents(&watched_path);
            match contents.write() {
                Ok(mut cached) => *cached = reloaded,
                Err(poisoned) => *poisoned.into_inner() = reloaded,
            }
            debug!(path = %watched_path.display(), "reloaded personality file");
        },
    ) {
        Ok(watcher) => watcher,
        Err(e) => {
            warn!(path = %path.display(), error = %e, "failed to create personality file watcher");
            return None;
        }
    };

    if let Err(e) = watcher.watch(&watch_dir, RecursiveMode::NonRecursive) {
        warn!(
            path = %path.display(),
            watch_dir = %watch_dir.display(),
            error = %e,
            "failed to watch personality file directory"
        );
        return None;
    }

    Some(watcher)
}

fn load_contents(path: &Path) -> String {
    if !path.exists() {
        return DEFAULT_PERSONALITY.to_string();
    }

    match std::fs::read_to_string(path) {
        Ok(contents) => contents,
        Err(e) => {
            warn!(path = %path.display(), error = %e, "failed to read personality file; using default");
            DEFAULT_PERSONALITY.to_string()
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::*;

    #[test]
    fn returns_default_when_file_is_missing() {
        let temp = tempfile::tempdir().unwrap();
        let personality = Personality::new(temp.path().join("missing.md"));

        assert!(personality.contents().contains("You are NerdBot"));
    }

    #[test]
    fn appends_timezone_context_to_cached_contents() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("personality.md");
        std::fs::write(&path, "Custom prompt.").unwrap();

        let personality = Personality::new(path);
        let effective = personality.effective_prompt("Asia/Kolkata");

        assert!(effective.contains("Custom prompt."));
        assert!(effective.contains("Asia/Kolkata"));
    }

    #[test]
    fn reloads_when_file_changes() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("personality.md");
        std::fs::write(&path, "first").unwrap();
        let personality = Personality::new(path.clone());

        std::fs::write(&path, "second").unwrap();

        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if personality.contents() == "second" {
                return;
            }
            std::thread::sleep(Duration::from_millis(50));
        }

        assert_eq!(personality.contents(), "second");
    }
}
