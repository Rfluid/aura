use std::{
    path::PathBuf,
    process::{Command, Stdio},
    sync::mpsc,
    thread,
    time::Duration,
};

use crate::config::PluginConfig;
use crate::reader::Period;

use super::{LegacyPluginPanel, PluginPanel};

const TIMEOUT_MS: u64 = 500;

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

        thread::spawn(move || {
            let result = Command::new(&cmd)
                .arg("--period")
                .arg(&period_str)
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
            color: None,
            icon: None,
        }
    }

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
