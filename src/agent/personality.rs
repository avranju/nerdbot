//! Cached personality prompt loader with filesystem change notifications.

use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use notify::event::ModifyKind;
use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
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
    state: Arc<RwLock<PersonalityState>>,
    _watcher: Option<RecommendedWatcher>,
}

struct PersonalityState {
    contents: String,
    hash: u64,
}

impl Personality {
    /// Load the personality prompt from the configured file and start watching
    /// for changes.
    pub fn from_config(config: &AppConfig) -> Self {
        Self::new(config.agent.personality_file.clone())
    }

    /// Load the personality prompt from `path` and start watching for changes.
    pub fn new(path: PathBuf) -> Self {
        let state = Arc::new(RwLock::new(load_state(&path)));
        let watcher = start_watcher(path.clone(), state.clone());

        Self {
            inner: Arc::new(PersonalityInner {
                path,
                state,
                _watcher: watcher,
            }),
        }
    }

    /// Return the cached raw personality prompt.
    pub fn contents(&self) -> String {
        self.inner
            .state
            .read()
            .map(|state| state.contents.clone())
            .unwrap_or_else(|poisoned| poisoned.into_inner().contents.clone())
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

fn start_watcher(
    path: PathBuf,
    state: Arc<RwLock<PersonalityState>>,
) -> Option<RecommendedWatcher> {
    let watch_dir = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));

    let watched_path = path.clone();
    let mut watcher = match notify::recommended_watcher(
        move |result: notify::Result<notify::Event>| {
            let event = match result {
                Ok(event) => event,
                Err(e) => {
                    warn!(error = %e, "personality file watcher error");
                    return;
                }
            };

            if !is_reload_event(&event, &watched_path) {
                return;
            }

            let reloaded = load_state(&watched_path);
            let changed = match state.write() {
                Ok(mut cached) => {
                    if cached.hash == reloaded.hash {
                        false
                    } else {
                        *cached = reloaded;
                        true
                    }
                }
                Err(poisoned) => {
                    let mut cached = poisoned.into_inner();
                    if cached.hash == reloaded.hash {
                        false
                    } else {
                        *cached = reloaded;
                        true
                    }
                }
            };

            if changed {
                debug!(path = %watched_path.display(), "reloaded personality file");
            }
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

fn is_reload_event(event: &Event, watched_path: &Path) -> bool {
    let is_content_change = matches!(
        event.kind,
        EventKind::Create(_)
            | EventKind::Modify(
                ModifyKind::Any | ModifyKind::Data(_) | ModifyKind::Name(_) | ModifyKind::Other
            )
    );
    if !is_content_change {
        return false;
    }

    event
        .paths
        .iter()
        .any(|path| path == watched_path || (path.is_relative() && watched_path.ends_with(path)))
}

fn load_state(path: &Path) -> PersonalityState {
    let contents = if !path.exists() {
        DEFAULT_PERSONALITY.to_string()
    } else {
        match std::fs::read_to_string(path) {
            Ok(contents) => contents,
            Err(e) => {
                warn!(path = %path.display(), error = %e, "failed to read personality file; using default");
                DEFAULT_PERSONALITY.to_string()
            }
        }
    };

    PersonalityState {
        hash: hash_contents(&contents),
        contents,
    }
}

fn hash_contents(contents: &str) -> u64 {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    let mut hasher = DefaultHasher::new();
    contents.hash(&mut hasher);
    hasher.finish()
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use notify::event::{AccessKind, CreateKind, DataChange, MetadataKind, ModifyKind};

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

    #[test]
    fn reloads_when_missing_file_is_created() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("personality.md");
        let personality = Personality::new(path.clone());
        assert!(personality.contents().contains("You are NerdBot"));

        std::fs::write(&path, "created later").unwrap();

        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if personality.contents() == "created later" {
                return;
            }
            std::thread::sleep(Duration::from_millis(50));
        }

        assert_eq!(personality.contents(), "created later");
    }

    #[test]
    fn reload_event_filter_ignores_access_events() {
        let watched = PathBuf::from("/tmp/personality.md");
        let event = Event::new(EventKind::Access(AccessKind::Read)).add_path(watched.clone());

        assert!(!is_reload_event(&event, &watched));
    }

    #[test]
    fn reload_event_filter_ignores_unrelated_paths() {
        let watched = PathBuf::from("/tmp/personality.md");
        let event = Event::new(EventKind::Modify(ModifyKind::Data(DataChange::Content)))
            .add_path(PathBuf::from("/tmp/other.md"));

        assert!(!is_reload_event(&event, &watched));
    }

    #[test]
    fn reload_event_filter_ignores_metadata_only_modify_events() {
        let watched = PathBuf::from("/tmp/personality.md");
        let event = Event::new(EventKind::Modify(ModifyKind::Metadata(MetadataKind::Any)))
            .add_path(watched.clone());

        assert!(!is_reload_event(&event, &watched));
    }

    #[test]
    fn reload_event_filter_accepts_create_or_modify_on_watched_path() {
        let watched = PathBuf::from("/tmp/personality.md");
        let create = Event::new(EventKind::Create(CreateKind::File)).add_path(watched.clone());
        let modify = Event::new(EventKind::Modify(ModifyKind::Data(DataChange::Content)))
            .add_path(watched.clone());

        assert!(is_reload_event(&create, &watched));
        assert!(is_reload_event(&modify, &watched));
    }

    #[test]
    fn identical_contents_have_identical_hashes() {
        assert_eq!(hash_contents("same"), hash_contents("same"));
        assert_ne!(hash_contents("same"), hash_contents("different"));
    }
}
