use std::{
    path::{Path, PathBuf},
    sync::mpsc::{channel, Receiver, RecvTimeoutError},
    time::Duration,
};

use anyhow::Result;
use notify_debouncer_mini::{
    new_debouncer,
    notify::{RecommendedWatcher, RecursiveMode},
    DebounceEventResult, Debouncer,
};

const DEBOUNCE_MS: u64 = 500;

/// Watches `{config_path}/projects/` recursively for changes to `*.jsonl`
/// files. Events are debounced over a 500ms quiet window and emitted as
/// batched path lists.
pub struct ProjectsWatcher {
    // Field name starts with `_` to silence dead-code warnings; we only
    // need to keep the debouncer alive — its drop unregisters the watcher.
    _debouncer: Debouncer<RecommendedWatcher>,
    rx: Receiver<Vec<PathBuf>>,
}

impl ProjectsWatcher {
    /// Start watching `{config_path}/projects/`. The directory is created if
    /// it doesn't already exist (the watcher target must exist on Linux).
    pub fn new(config_path: &Path) -> Result<Self> {
        let projects_dir = config_path.join("projects");
        if !projects_dir.exists() {
            std::fs::create_dir_all(&projects_dir)?;
        }

        let (tx, rx) = channel::<Vec<PathBuf>>();

        let mut debouncer = new_debouncer(
            Duration::from_millis(DEBOUNCE_MS),
            move |res: DebounceEventResult| {
                let Ok(events) = res else {
                    return;
                };
                let paths: Vec<PathBuf> = events
                    .into_iter()
                    .map(|e| e.path)
                    .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("jsonl"))
                    .collect();
                if !paths.is_empty() {
                    let _ = tx.send(paths);
                }
            },
        )?;

        debouncer
            .watcher()
            .watch(&projects_dir, RecursiveMode::Recursive)?;

        Ok(Self {
            _debouncer: debouncer,
            rx,
        })
    }

    /// Non-blocking: returns `Some(batch)` if events have been queued.
    pub fn try_recv(&self) -> Option<Vec<PathBuf>> {
        self.rx.try_recv().ok()
    }

    /// Block up to `dur` waiting for the next batched event.
    pub fn recv_timeout(&self, dur: Duration) -> Result<Vec<PathBuf>, RecvTimeoutError> {
        self.rx.recv_timeout(dur)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn watcher_fires_on_new_jsonl_file() {
        let dir = tempdir().unwrap();
        let projects = dir.path().join("projects").join("proj1");
        fs::create_dir_all(&projects).unwrap();

        let watcher = ProjectsWatcher::new(dir.path()).unwrap();

        // Give the OS a moment to register the watcher
        std::thread::sleep(Duration::from_millis(100));

        // Create a JSONL file
        let file = projects.join("session.jsonl");
        fs::write(&file, "{\"type\":\"user\"}\n").unwrap();

        // Wait for the debounced event (debounce + slack)
        let paths = watcher.recv_timeout(Duration::from_secs(3)).unwrap();
        assert!(
            paths.iter().any(|p| p.ends_with("session.jsonl")),
            "expected session.jsonl in {:?}",
            paths
        );
    }

    #[test]
    fn watcher_ignores_non_jsonl_files() {
        let dir = tempdir().unwrap();
        let projects = dir.path().join("projects").join("proj1");
        fs::create_dir_all(&projects).unwrap();

        let watcher = ProjectsWatcher::new(dir.path()).unwrap();
        std::thread::sleep(Duration::from_millis(100));

        // Create a non-JSONL file
        fs::write(projects.join("notes.txt"), "hello").unwrap();

        // Should time out — no JSONL change.
        let result = watcher.recv_timeout(Duration::from_millis(1500));
        assert!(result.is_err(), "expected timeout, got {:?}", result);
    }

    #[test]
    fn watcher_debounces_rapid_writes() {
        let dir = tempdir().unwrap();
        let projects = dir.path().join("projects").join("proj1");
        fs::create_dir_all(&projects).unwrap();

        let watcher = ProjectsWatcher::new(dir.path()).unwrap();
        std::thread::sleep(Duration::from_millis(100));

        let file = projects.join("session.jsonl");
        // Five rapid writes within the debounce window
        for i in 0..5 {
            fs::write(&file, format!("{{\"n\":{}}}\n", i)).unwrap();
            std::thread::sleep(Duration::from_millis(20));
        }

        // Should receive at least one batch (debouncer coalesces them)
        let first = watcher.recv_timeout(Duration::from_secs(3)).unwrap();
        assert!(first.iter().any(|p| p.ends_with("session.jsonl")));
    }
}
