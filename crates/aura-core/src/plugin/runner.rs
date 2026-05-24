use std::{
    collections::HashSet,
    ffi::OsString,
    path::PathBuf,
    process::{Command, Stdio},
    sync::mpsc,
    thread,
    time::Duration,
};

use crate::config::PluginConfig;
use crate::reader::Period;

use super::{LegacyPluginPanel, PluginPanel};

// Production: tight budget so a hung plugin doesn't freeze the UI.
// Tests: generous budget because macOS process spawning under parallel test
// load can take 1-2 s (SIP/Gatekeeper overhead), causing spurious timeouts.
#[cfg(not(test))]
const TIMEOUT_MS: u64 = 500;
#[cfg(test)]
const TIMEOUT_MS: u64 = 5_000;

pub struct PluginRunner;

fn period_arg(period: Period) -> &'static str {
    match period {
        Period::AllTime => "all",
        Period::Last7Days => "7d",
        Period::Last30Days => "30d",
    }
}

/// Resolve a plugin command to a path. Lookup order:
/// 1. If `cmd` contains a path separator, use as-is (caller specified a path)
/// 2. Otherwise, prefer a sibling of the current executable
///    (covers `cargo run` where `target/debug/aura-plugin-rtk` exists
///    alongside `target/debug/aura`)
/// 3. Fall back to `cmd` as a bare name so `Command::new` searches `$PATH`
fn resolve_command(cmd: &str) -> PathBuf {
    if cmd.contains('/') {
        return PathBuf::from(cmd);
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(parent) = exe.parent() {
            let sibling = parent.join(cmd);
            if sibling.exists() {
                return sibling;
            }
        }
    }
    PathBuf::from(cmd)
}

/// Build the `PATH` to hand to spawned plugins. GUI launchers (Linux .desktop
/// files, macOS .app bundles, systemd user units) do not source the user's
/// shell rc files, so the inherited `PATH` is often missing `~/.local/bin`,
/// `~/.cargo/bin`, `/opt/homebrew/bin`, etc. — exactly the dirs plugins need
/// to find tools they shell out to (e.g. `aura-plugin-rtk` running `rtk`).
fn augmented_path() -> OsString {
    augmented_path_from(dirs::home_dir(), std::env::var_os("PATH"))
}

fn augmented_path_from(home: Option<PathBuf>, existing: Option<OsString>) -> OsString {
    let mut entries: Vec<PathBuf> = Vec::new();
    let mut seen: HashSet<PathBuf> = HashSet::new();

    let push = |p: PathBuf, entries: &mut Vec<PathBuf>, seen: &mut HashSet<PathBuf>| {
        if p.is_dir() && seen.insert(p.clone()) {
            entries.push(p);
        }
    };

    if let Some(home) = home {
        for sub in [".local/bin", ".cargo/bin", ".bun/bin", "bin"] {
            push(home.join(sub), &mut entries, &mut seen);
        }
    }
    // /opt/homebrew/bin is Apple Silicon Homebrew; harmless on Linux (won't exist).
    for p in ["/opt/homebrew/bin", "/usr/local/bin"] {
        push(PathBuf::from(p), &mut entries, &mut seen);
    }

    if let Some(existing) = existing.as_ref() {
        for p in std::env::split_paths(existing) {
            if !p.as_os_str().is_empty() && seen.insert(p.clone()) {
                entries.push(p);
            }
        }
    }

    std::env::join_paths(entries).unwrap_or_else(|_| existing.unwrap_or_default())
}

impl PluginRunner {
    /// Spawn `config.command` (default period: AllTime).
    pub fn run(config: &PluginConfig) -> PluginPanel {
        Self::run_with_period(config, Period::AllTime)
    }

    /// Spawn `config.command --period <all|7d|30d>`, wait up to 500ms, parse
    /// stdout as JSON into a `PluginPanel` (new section-based format), falling
    /// back to the legacy flat `{title, lines, error}` shape for older plugins.
    /// Failures are surfaced as a `PluginPanel` with `error` set.
    pub fn run_with_period(config: &PluginConfig, period: Period) -> PluginPanel {
        let (tx, rx) = mpsc::channel();
        let cmd = resolve_command(&config.command);
        let period_str = period_arg(period).to_string();

        let path_env = augmented_path();
        thread::spawn(move || {
            let result = Command::new(&cmd)
                .arg("--period")
                .arg(&period_str)
                .env("PATH", path_env)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .output();
            let _ = tx.send(result);
        });

        let output = match rx.recv_timeout(Duration::from_millis(TIMEOUT_MS)) {
            Ok(Ok(o)) => o,
            Ok(Err(e)) => {
                return PluginPanel::from_error(
                    &config.name,
                    format!("failed to spawn `{}`: {e}", config.command),
                );
            }
            Err(_) => {
                return PluginPanel::from_error(
                    &config.name,
                    format!("`{}` timed out after {TIMEOUT_MS}ms", config.command),
                );
            }
        };

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            let msg = if stderr.is_empty() {
                format!("exited with status {}", output.status)
            } else {
                stderr
            };
            return PluginPanel::from_error(&config.name, msg);
        }

        let stdout = String::from_utf8_lossy(&output.stdout);

        // Prefer the new sectioned shape; fall back to the legacy flat shape.
        if let Ok(panel) = serde_json::from_str::<PluginPanel>(&stdout) {
            if !panel.sections.is_empty() || panel.error.is_some() {
                return panel;
            }
        }
        match serde_json::from_str::<LegacyPluginPanel>(&stdout) {
            Ok(legacy) => legacy.into(),
            Err(e) => PluginPanel::from_error(&config.name, format!("invalid plugin JSON: {e}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    use std::io::Write;
    #[cfg(unix)]
    use tempfile::tempdir;

    /// Build a small shell-script "plugin" in a temp dir.
    #[cfg(unix)]
    fn write_script(dir: &std::path::Path, body: &str) -> std::path::PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let path = dir.join("fake-plugin.sh");
        let mut f = std::fs::File::create(&path).unwrap();
        writeln!(f, "#!/bin/sh").unwrap();
        f.write_all(body.as_bytes()).unwrap();
        let mut perms = std::fs::metadata(&path).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&path, perms).unwrap();
        path
    }

    #[cfg(unix)]
    fn cfg(cmd: &std::path::Path) -> PluginConfig {
        PluginConfig {
            name: "Test".to_string(),
            command: cmd.to_string_lossy().to_string(),
            color: None,
            icon: None,
        }
    }

    #[cfg(unix)]
    #[test]
    fn runs_legacy_plugin_and_wraps_into_default_section() {
        let dir = tempdir().unwrap();
        let script = write_script(
            dir.path(),
            r#"cat <<'EOF'
{"title":"Test","lines":[{"label":"A","value":"42","highlight":true}]}
EOF
"#,
        );

        let panel = PluginRunner::run(&cfg(&script));
        assert_eq!(panel.title, "Test");
        assert!(panel.error.is_none());
        assert_eq!(panel.sections.len(), 1);
        let section = &panel.sections[0];
        assert_eq!(section.id, "default");
        match &section.content {
            super::super::PluginContent::Lines { lines } => {
                assert_eq!(lines.len(), 1);
                assert_eq!(lines[0].value, "42");
                assert!(lines[0].highlight);
            }
            other => panic!("expected Lines, got {other:?}"),
        }
    }

    #[cfg(unix)]
    #[test]
    fn runs_section_plugin_directly() {
        let dir = tempdir().unwrap();
        let script = write_script(
            dir.path(),
            r##"cat <<'EOF'
{"title":"RTK","sections":[
  {"id":"overview","label":"Overview","type":"lines","lines":[{"label":"A","value":"1"}]},
  {"id":"table","label":"By Command","type":"table","headers":["#","Cmd"],"rows":[{"cells":["1","ls"]}]}
]}
EOF
"##,
        );

        let panel = PluginRunner::run(&cfg(&script));
        assert!(panel.error.is_none());
        assert_eq!(panel.sections.len(), 2);
        assert_eq!(panel.sections[0].id, "overview");
        assert_eq!(panel.sections[1].id, "table");
    }

    #[test]
    fn returns_error_panel_on_missing_binary() {
        let panel = PluginRunner::run(&PluginConfig {
            name: "Missing".to_string(),
            command: "/definitely/does/not/exist/aura-plugin-xyz".to_string(),
            color: None,
            icon: None,
        });
        assert!(panel.error.is_some());
        assert_eq!(panel.title, "Missing");
    }

    #[cfg(unix)]
    #[test]
    fn returns_error_panel_on_timeout() {
        let dir = tempdir().unwrap();
        let script = write_script(dir.path(), "sleep 7\n");

        let panel = PluginRunner::run(&cfg(&script));
        assert!(panel.error.is_some());
        assert!(panel.error.as_deref().unwrap().contains("timed out"));
    }

    #[cfg(unix)]
    #[test]
    fn returns_error_panel_on_bad_json() {
        let dir = tempdir().unwrap();
        let script = write_script(dir.path(), "echo 'not json {{ '\n");

        let panel = PluginRunner::run(&cfg(&script));
        assert!(panel.error.is_some());
        assert!(panel
            .error
            .as_deref()
            .unwrap()
            .contains("invalid plugin JSON"));
    }

    #[cfg(unix)]
    #[test]
    fn augmented_path_prepends_existing_user_dirs() {
        let home = tempdir().unwrap();
        let local_bin = home.path().join(".local/bin");
        let cargo_bin = home.path().join(".cargo/bin");
        std::fs::create_dir_all(&local_bin).unwrap();
        std::fs::create_dir_all(&cargo_bin).unwrap();
        // `~/.bun/bin` deliberately missing — must be skipped.

        let existing = OsString::from("/usr/bin:/bin");
        let merged = augmented_path_from(Some(home.path().to_path_buf()), Some(existing));
        let parts: Vec<PathBuf> = std::env::split_paths(&merged).collect();

        assert!(
            parts.contains(&local_bin),
            "missing ~/.local/bin: {parts:?}"
        );
        assert!(
            parts.contains(&cargo_bin),
            "missing ~/.cargo/bin: {parts:?}"
        );
        assert!(parts.contains(&PathBuf::from("/usr/bin")));
        assert!(parts.contains(&PathBuf::from("/bin")));
        // Augmented dirs must come before the inherited entries.
        let local_idx = parts.iter().position(|p| p == &local_bin).unwrap();
        let usr_idx = parts
            .iter()
            .position(|p| p == &PathBuf::from("/usr/bin"))
            .unwrap();
        assert!(local_idx < usr_idx);
        // Non-existent ~/.bun/bin should not appear.
        assert!(!parts.contains(&home.path().join(".bun/bin")));
    }

    #[cfg(unix)]
    #[test]
    fn augmented_path_dedupes_existing_entries() {
        let home = tempdir().unwrap();
        let local_bin = home.path().join(".local/bin");
        std::fs::create_dir_all(&local_bin).unwrap();

        // Existing PATH already contains ~/.local/bin — should not duplicate.
        let existing = OsString::from(format!("{}:/usr/bin", local_bin.display()));
        let merged = augmented_path_from(Some(home.path().to_path_buf()), Some(existing));
        let count = std::env::split_paths(&merged)
            .filter(|p| p == &local_bin)
            .count();
        assert_eq!(count, 1);
    }

    #[cfg(unix)]
    #[test]
    fn returns_error_panel_on_nonzero_exit() {
        let dir = tempdir().unwrap();
        let script = write_script(dir.path(), "echo 'boom' >&2\nexit 1\n");

        let panel = PluginRunner::run(&cfg(&script));
        assert!(panel.error.is_some());
        assert_eq!(panel.error.as_deref(), Some("boom"));
    }
}
