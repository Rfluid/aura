use std::{
    process::{Command, Stdio},
    sync::mpsc,
    thread,
    time::Duration,
};

use crate::config::PluginConfig;

use super::PluginPanel;

const TIMEOUT_MS: u64 = 500;

pub struct PluginRunner;

impl PluginRunner {
    /// Spawn `config.command`, wait up to 500ms, parse stdout as JSON
    /// into a `PluginPanel`. Failures (spawn error, timeout, non-zero
    /// exit, bad JSON) are surfaced as a `PluginPanel` with `error` set —
    /// never as a panic or `Err`.
    pub fn run(config: &PluginConfig) -> PluginPanel {
        let (tx, rx) = mpsc::channel();
        let cmd = config.command.clone();

        thread::spawn(move || {
            let result = Command::new(&cmd)
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
        match serde_json::from_str::<PluginPanel>(&stdout) {
            Ok(panel) => panel,
            Err(e) => PluginPanel::from_error(&config.name, format!("invalid plugin JSON: {e}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::tempdir;

    /// Build a small shell-script "plugin" in a temp dir.
    fn write_script(dir: &std::path::Path, body: &str) -> std::path::PathBuf {
        let path = dir.join("fake-plugin.sh");
        let mut f = std::fs::File::create(&path).unwrap();
        writeln!(f, "#!/bin/sh").unwrap();
        f.write_all(body.as_bytes()).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(&path).unwrap().permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&path, perms).unwrap();
        }
        path
    }

    fn cfg(cmd: &std::path::Path) -> PluginConfig {
        PluginConfig {
            name: "Test".to_string(),
            command: cmd.to_string_lossy().to_string(),
        }
    }

    #[test]
    fn runs_valid_plugin_and_parses_output() {
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
        assert_eq!(panel.lines.len(), 1);
        assert_eq!(panel.lines[0].value, "42");
        assert!(panel.lines[0].highlight);
        assert!(panel.error.is_none());
    }

    #[test]
    fn returns_error_panel_on_missing_binary() {
        let panel = PluginRunner::run(&PluginConfig {
            name: "Missing".to_string(),
            command: "/definitely/does/not/exist/aura-plugin-xyz".to_string(),
        });
        assert!(panel.error.is_some());
        assert_eq!(panel.title, "Missing");
    }

    #[test]
    fn returns_error_panel_on_timeout() {
        let dir = tempdir().unwrap();
        let script = write_script(dir.path(), "sleep 2\n");

        let panel = PluginRunner::run(&cfg(&script));
        assert!(panel.error.is_some());
        assert!(panel.error.as_deref().unwrap().contains("timed out"));
    }

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

    #[test]
    fn returns_error_panel_on_nonzero_exit() {
        let dir = tempdir().unwrap();
        let script = write_script(dir.path(), "echo 'boom' >&2\nexit 1\n");

        let panel = PluginRunner::run(&cfg(&script));
        assert!(panel.error.is_some());
        assert_eq!(panel.error.as_deref(), Some("boom"));
    }
}
