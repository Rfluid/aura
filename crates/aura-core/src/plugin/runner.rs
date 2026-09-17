use std::{
    path::PathBuf,
    process::{Command, Stdio},
    sync::mpsc,
    thread,
    time::Duration,
};

use crate::bin_path::augmented_path;
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

// Actions run off the render path and may legitimately block on user
// interaction (e.g. a plugin opening a native file-picker dialog), so
// they get a far more generous budget than panel refreshes.
const ACTION_TIMEOUT_MS: u64 = 180_000;

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
        Self::invoke(config, &[], TIMEOUT_MS, period)
    }

    /// Spawn `config.command action <id> --period <p>` — fired when the user
    /// clicks a [`super::PluginButton`]. The plugin performs the action and
    /// prints a refreshed panel. Generous timeout: actions may block on user
    /// interaction (native pickers).
    pub fn run_action(config: &PluginConfig, action: &str, period: Period) -> PluginPanel {
        Self::invoke(
            config,
            &["action".to_string(), action.to_string()],
            ACTION_TIMEOUT_MS,
            period,
        )
    }

    fn invoke(
        config: &PluginConfig,
        leading_args: &[String],
        timeout_ms: u64,
        period: Period,
    ) -> PluginPanel {
        let (tx, rx) = mpsc::channel();
        let cmd = resolve_command(&config.command);
        let period_str = period_arg(period).to_string();
        let leading_args = leading_args.to_vec();

        let path_env = augmented_path();
        thread::spawn(move || {
            let mut builder = Command::new(&cmd);
            builder
                .args(&leading_args)
                .arg("--period")
                .arg(&period_str)
                .env("PATH", path_env)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());
            // When the parent is a GUI process (windows_subsystem = "windows"),
            // Windows creates a console window for every console-subsystem child.
            // CREATE_NO_WINDOW suppresses that flash.
            #[cfg(target_os = "windows")]
            {
                use std::os::windows::process::CommandExt;
                const CREATE_NO_WINDOW: u32 = 0x0800_0000;
                builder.creation_flags(CREATE_NO_WINDOW);
            }
            let _ = tx.send(builder.output());
        });

        let output = match rx.recv_timeout(Duration::from_millis(timeout_ms)) {
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
                    format!("`{}` timed out after {timeout_ms}ms", config.command),
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

    #[cfg(unix)]
    #[test]
    fn run_action_passes_action_args() {
        let dir = tempdir().unwrap();
        // Echo the received args back through the panel title so we can
        // assert the exact invocation shape: `action <id> --period <p>`.
        let script = write_script(
            dir.path(),
            r#"printf '{"title":"%s %s %s %s","sections":[{"id":"s","label":"S","type":"lines","lines":[]}]}' "$1" "$2" "$3" "$4"
"#,
        );

        let panel = PluginRunner::run_action(&cfg(&script), "mute:on", Period::Last7Days);
        assert!(panel.error.is_none());
        assert_eq!(panel.title, "action mute:on --period 7d");
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
    fn returns_error_panel_on_nonzero_exit() {
        let dir = tempdir().unwrap();
        let script = write_script(dir.path(), "echo 'boom' >&2\nexit 1\n");

        let panel = PluginRunner::run(&cfg(&script));
        assert!(panel.error.is_some());
        assert_eq!(panel.error.as_deref(), Some("boom"));
    }
}
