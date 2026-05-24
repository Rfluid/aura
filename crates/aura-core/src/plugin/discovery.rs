//! Filesystem-based plugin discovery and install operations.
//!
//! Third-party plugins drop into `~/.config/aura/plugins/`. Any executable
//! file in that directory is treated as a plugin and merged into the
//! in-memory plugin list at load time. An optional sidecar `<file>.toml`
//! supplies the user-facing name, accent color, and icon path; without
//! one we derive a name from the filename (stripping `aura-plugin-`).
//!
//! The on-disk `config.toml` is never rewritten by discovery — removing a
//! plugin is as simple as deleting the binary, with no stale entries left
//! behind.

use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};

use crate::config::PluginConfig;

/// Optional metadata file alongside a discovered plugin binary.
///
/// Located at `<plugins_dir>/<binary>.toml`. All fields are optional —
/// missing values fall back to derived defaults (name from filename, no
/// color override, no icon override).
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct PluginSidecar {
    /// Display name shown in the tray modal. Defaults to a title-cased
    /// version of the filename (stripping `aura-plugin-`).
    #[serde(default)]
    pub name: Option<String>,
    /// Accent color (hex `#rrggbb` or `#rgb`).
    #[serde(default)]
    pub color: Option<String>,
    /// Icon: embedded asset path, absolute path, or `~/`-relative path.
    #[serde(default)]
    pub icon: Option<String>,
}

/// Default user-level plugins directory: `~/.config/aura/plugins/` on
/// Linux, `~/Library/Application Support/aura/plugins/` on macOS,
/// `%APPDATA%\aura\plugins\` on Windows.
pub fn user_plugins_dir() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("~/.config"))
        .join("aura")
        .join("plugins")
}

/// Resolve the plugins dir that sits next to a given `config.toml` path.
/// Falls back to the user-level dir when the config path has no parent
/// (shouldn't happen in practice — `AppConfig::default_path()` always has
/// a parent).
pub fn plugins_dir_for_config(config_path: &Path) -> PathBuf {
    config_path
        .parent()
        .map(|p| p.join("plugins"))
        .unwrap_or_else(user_plugins_dir)
}

/// Scan `dir` for executable plugin binaries and return one `PluginConfig`
/// per file (sorted by display name).
///
/// Failures are non-fatal: a missing or unreadable directory yields an
/// empty list, individual unreadable sidecars yield a default sidecar.
pub fn discover_plugins(dir: &Path) -> Vec<PluginConfig> {
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return Vec::new(),
    };

    let mut out = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if !is_plugin_candidate(&path) {
            continue;
        }

        let sidecar = read_sidecar(&path).unwrap_or_default();
        let display_name = sidecar
            .name
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| derive_name(&path));

        out.push(PluginConfig {
            name: display_name,
            command: path.to_string_lossy().into_owned(),
            color: sidecar.color,
            icon: sidecar.icon,
        });
    }

    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// Read the optional sidecar TOML for a plugin binary at `binary_path`.
/// Returns `Ok(None)` when no sidecar exists.
pub fn read_sidecar(binary_path: &Path) -> Result<PluginSidecar> {
    let sidecar_path = sidecar_path_for(binary_path);
    if !sidecar_path.exists() {
        return Ok(PluginSidecar::default());
    }
    let content = fs::read_to_string(&sidecar_path)
        .with_context(|| format!("read sidecar {}", sidecar_path.display()))?;
    let sidecar: PluginSidecar = toml::from_str(&content)
        .with_context(|| format!("parse sidecar {}", sidecar_path.display()))?;
    Ok(sidecar)
}

/// Sidecar TOML path for a given binary: `<binary>.toml` in the same
/// directory. Note the `.toml` is appended to the *full* filename, not
/// substituted for an extension — `aura-plugin-foo` → `aura-plugin-foo.toml`,
/// `aura-plugin-foo.exe` → `aura-plugin-foo.exe.toml`.
pub fn sidecar_path_for(binary_path: &Path) -> PathBuf {
    let mut name = binary_path
        .file_name()
        .map(|s| s.to_os_string())
        .unwrap_or_default();
    name.push(".toml");
    binary_path
        .parent()
        .map(|p| p.join(&name))
        .unwrap_or_else(|| PathBuf::from(&name))
}

// ── Install ops (used by the `aura plugin` subcommand) ────────────────────────

/// Arguments accepted by `aura plugin add` — copy a built plugin binary
/// into the user plugins dir and optionally write a sidecar with display
/// metadata.
pub struct AddOptions {
    /// Source path of the plugin binary to install.
    pub source: PathBuf,
    /// Override the destination filename (defaults to the source filename).
    pub dest_name: Option<String>,
    /// Symlink instead of copying. Useful for `cargo build` dev loops on
    /// Unix — re-running `cargo build` updates the live plugin in place.
    /// No-op on Windows (we copy instead).
    pub symlink: bool,
    /// Sidecar display name override.
    pub name: Option<String>,
    /// Sidecar accent color override.
    pub color: Option<String>,
    /// Sidecar icon path override.
    pub icon: Option<String>,
}

/// Result of `aura plugin add`.
#[derive(Debug)]
pub struct AddOutcome {
    /// Final on-disk path of the installed binary (or symlink).
    pub installed: PathBuf,
    /// Path of the sidecar TOML, if one was written.
    pub sidecar: Option<PathBuf>,
}

/// Copy (or symlink) the binary at `opts.source` into `plugins_dir` and
/// write the sidecar TOML when metadata overrides are provided.
pub fn add_plugin(plugins_dir: &Path, opts: AddOptions) -> Result<AddOutcome> {
    if !opts.source.exists() {
        return Err(anyhow!(
            "source path does not exist: {}",
            opts.source.display()
        ));
    }
    if !opts.source.is_file() && !opts.symlink {
        return Err(anyhow!(
            "source path is not a file: {}",
            opts.source.display()
        ));
    }

    fs::create_dir_all(plugins_dir)
        .with_context(|| format!("create plugins dir {}", plugins_dir.display()))?;

    let filename = match &opts.dest_name {
        Some(n) => n.clone(),
        None => opts
            .source
            .file_name()
            .ok_or_else(|| anyhow!("source has no file name component"))?
            .to_string_lossy()
            .into_owned(),
    };
    let dest = plugins_dir.join(&filename);

    // Replace any existing entry (binary or symlink) at the destination,
    // so `aura plugin add` is idempotent for the same source.
    if dest.exists() || dest.symlink_metadata().is_ok() {
        fs::remove_file(&dest)
            .with_context(|| format!("remove existing plugin {}", dest.display()))?;
    }

    if opts.symlink {
        symlink_file(&opts.source, &dest)?;
    } else {
        fs::copy(&opts.source, &dest)
            .with_context(|| format!("copy {} -> {}", opts.source.display(), dest.display()))?;
        ensure_executable(&dest)?;
    }

    let sidecar_written = write_sidecar_if_any(
        &dest,
        PluginSidecar {
            name: opts.name,
            color: opts.color,
            icon: opts.icon,
        },
    )?;

    Ok(AddOutcome {
        installed: dest,
        sidecar: sidecar_written,
    })
}

fn write_sidecar_if_any(binary: &Path, sidecar: PluginSidecar) -> Result<Option<PathBuf>> {
    if sidecar.name.is_none() && sidecar.color.is_none() && sidecar.icon.is_none() {
        return Ok(None);
    }
    let sidecar_path = sidecar_path_for(binary);
    let toml = toml::to_string_pretty(&sidecar).context("serialize plugin sidecar")?;
    fs::write(&sidecar_path, toml)
        .with_context(|| format!("write sidecar {}", sidecar_path.display()))?;
    Ok(Some(sidecar_path))
}

/// Result of `aura plugin remove`.
#[derive(Debug)]
pub struct RemoveOutcome {
    /// Path of the binary that was deleted.
    pub removed_binary: PathBuf,
    /// Path of the sidecar that was deleted, if any.
    pub removed_sidecar: Option<PathBuf>,
}

/// Remove a discovered plugin by display name. Looks up the plugin via
/// `discover_plugins(plugins_dir)`, then deletes the binary and its
/// sidecar (if present). Returns an error if no matching plugin is found.
pub fn remove_plugin(plugins_dir: &Path, name: &str) -> Result<RemoveOutcome> {
    let discovered = discover_plugins(plugins_dir);
    let matched = discovered
        .into_iter()
        .find(|p| p.name.eq_ignore_ascii_case(name))
        .ok_or_else(|| {
            anyhow!(
                "no discovered plugin named '{name}' in {}",
                plugins_dir.display()
            )
        })?;

    let binary = PathBuf::from(&matched.command);
    fs::remove_file(&binary).with_context(|| format!("remove {}", binary.display()))?;

    let sidecar_path = sidecar_path_for(&binary);
    let removed_sidecar = if sidecar_path.exists() {
        fs::remove_file(&sidecar_path)
            .with_context(|| format!("remove sidecar {}", sidecar_path.display()))?;
        Some(sidecar_path)
    } else {
        None
    };

    Ok(RemoveOutcome {
        removed_binary: binary,
        removed_sidecar,
    })
}

// ── Filesystem helpers ───────────────────────────────────────────────────────

fn is_plugin_candidate(path: &Path) -> bool {
    let Ok(meta) = path.metadata() else {
        return false;
    };
    if !meta.is_file() {
        return false;
    }
    // Skip dotfiles, sidecars, READMEs, and anything obviously not-a-binary.
    if let Some(name) = path.file_name().and_then(|s| s.to_str()) {
        if name.starts_with('.') {
            return false;
        }
    }
    if let Some(ext) = path.extension().and_then(|s| s.to_str()) {
        let lower = ext.to_ascii_lowercase();
        if matches!(
            lower.as_str(),
            "toml" | "md" | "txt" | "json" | "yaml" | "yml" | "lock"
        ) {
            return false;
        }
    }
    is_executable(path, &meta)
}

#[cfg(unix)]
fn is_executable(_path: &Path, meta: &fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt;
    meta.permissions().mode() & 0o111 != 0
}

#[cfg(windows)]
fn is_executable(path: &Path, _meta: &fs::Metadata) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| {
            matches!(
                e.to_ascii_lowercase().as_str(),
                "exe" | "bat" | "cmd" | "ps1" | "com"
            )
        })
        .unwrap_or(false)
}

#[cfg(unix)]
fn ensure_executable(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mut perms = fs::metadata(path)
        .with_context(|| format!("stat {}", path.display()))?
        .permissions();
    let mode = perms.mode() | 0o755;
    perms.set_mode(mode);
    fs::set_permissions(path, perms).with_context(|| format!("chmod +x {}", path.display()))?;
    Ok(())
}

#[cfg(not(unix))]
fn ensure_executable(_path: &Path) -> Result<()> {
    Ok(())
}

#[cfg(unix)]
fn symlink_file(src: &Path, dest: &Path) -> Result<()> {
    std::os::unix::fs::symlink(src, dest)
        .with_context(|| format!("symlink {} -> {}", dest.display(), src.display()))
}

#[cfg(not(unix))]
fn symlink_file(src: &Path, dest: &Path) -> Result<()> {
    // Symlinks on Windows require either dev-mode or elevated rights; we
    // fall back to a copy to keep the contract uniform.
    fs::copy(src, dest)
        .with_context(|| format!("copy {} -> {}", src.display(), dest.display()))
        .map(|_| ())
}

/// Strip a recognised plugin filename prefix/suffix and title-case the
/// remainder. `aura-plugin-rtk-gains.exe` → `Rtk Gains`.
fn derive_name(path: &Path) -> String {
    let raw = path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("plugin");
    // Strip Windows `.exe` so the derived name matches across platforms.
    let stem = raw.strip_suffix(".exe").unwrap_or(raw);
    let core = stem.strip_prefix("aura-plugin-").unwrap_or(stem);

    let words: Vec<String> = core
        .split(['-', '_'])
        .filter(|s| !s.is_empty())
        .map(|w| {
            let mut chars = w.chars();
            match chars.next() {
                Some(c) => c.to_uppercase().chain(chars).collect::<String>(),
                None => String::new(),
            }
        })
        .collect();

    if words.is_empty() {
        core.to_string()
    } else {
        words.join(" ")
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[cfg(unix)]
    fn write_exec(dir: &Path, name: &str, body: &str) -> PathBuf {
        use std::io::Write;
        use std::os::unix::fs::PermissionsExt;
        let path = dir.join(name);
        let mut f = fs::File::create(&path).unwrap();
        writeln!(f, "#!/bin/sh").unwrap();
        f.write_all(body.as_bytes()).unwrap();
        let mut perms = fs::metadata(&path).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&path, perms).unwrap();
        path
    }

    #[test]
    fn discover_returns_empty_for_missing_dir() {
        let dir = tempdir().unwrap();
        let missing = dir.path().join("nope");
        assert!(discover_plugins(&missing).is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn discover_picks_up_executable_without_sidecar() {
        let dir = tempdir().unwrap();
        write_exec(dir.path(), "aura-plugin-hello", "echo {}\n");

        let plugins = discover_plugins(dir.path());
        assert_eq!(plugins.len(), 1);
        assert_eq!(plugins[0].name, "Hello");
        assert!(plugins[0].command.ends_with("aura-plugin-hello"));
        assert!(plugins[0].color.is_none());
    }

    #[cfg(unix)]
    #[test]
    fn discover_reads_sidecar_metadata() {
        let dir = tempdir().unwrap();
        write_exec(dir.path(), "aura-plugin-demo", "echo {}\n");
        fs::write(
            dir.path().join("aura-plugin-demo.toml"),
            r##"
name = "Demo Stats"
color = "#abcdef"
icon = "icons/demo.svg"
"##,
        )
        .unwrap();

        let plugins = discover_plugins(dir.path());
        assert_eq!(plugins.len(), 1);
        assert_eq!(plugins[0].name, "Demo Stats");
        assert_eq!(plugins[0].color.as_deref(), Some("#abcdef"));
        assert_eq!(plugins[0].icon.as_deref(), Some("icons/demo.svg"));
    }

    #[cfg(unix)]
    #[test]
    fn discover_skips_non_executables_and_sidecars() {
        let dir = tempdir().unwrap();
        // Sidecar without matching binary: still skipped.
        fs::write(dir.path().join("stray.toml"), "name = \"Nope\"\n").unwrap();
        // Plain text file: skipped.
        fs::write(dir.path().join("README.md"), "hi").unwrap();
        // Non-executable file: skipped.
        fs::write(dir.path().join("aura-plugin-nope"), "binary").unwrap();
        // Hidden file: skipped.
        write_exec(dir.path(), ".hidden", "echo {}\n");
        // Valid plugin: kept.
        write_exec(dir.path(), "aura-plugin-good", "echo {}\n");

        let plugins = discover_plugins(dir.path());
        assert_eq!(plugins.len(), 1);
        assert_eq!(plugins[0].name, "Good");
    }

    #[cfg(unix)]
    #[test]
    fn discover_sorts_alphabetically_by_name() {
        let dir = tempdir().unwrap();
        write_exec(dir.path(), "aura-plugin-zeta", "echo {}\n");
        write_exec(dir.path(), "aura-plugin-alpha", "echo {}\n");
        write_exec(dir.path(), "aura-plugin-mid", "echo {}\n");

        let plugins = discover_plugins(dir.path());
        let names: Vec<_> = plugins.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, vec!["Alpha", "Mid", "Zeta"]);
    }

    #[cfg(unix)]
    #[test]
    fn add_plugin_copies_and_writes_sidecar() {
        let src_dir = tempdir().unwrap();
        let plugins_dir = tempdir().unwrap();
        let src = write_exec(src_dir.path(), "aura-plugin-demo", "echo {}\n");

        let outcome = add_plugin(
            plugins_dir.path(),
            AddOptions {
                source: src.clone(),
                dest_name: None,
                symlink: false,
                name: Some("Demo".into()),
                color: Some("#112233".into()),
                icon: None,
            },
        )
        .unwrap();

        assert!(outcome.installed.exists());
        assert!(outcome.sidecar.is_some());
        let sidecar = outcome.sidecar.unwrap();
        assert!(sidecar.exists());
        let parsed: PluginSidecar = toml::from_str(&fs::read_to_string(&sidecar).unwrap()).unwrap();
        assert_eq!(parsed.name.as_deref(), Some("Demo"));
        assert_eq!(parsed.color.as_deref(), Some("#112233"));

        // Discovery should now find it.
        let plugins = discover_plugins(plugins_dir.path());
        assert_eq!(plugins.len(), 1);
        assert_eq!(plugins[0].name, "Demo");
    }

    #[cfg(unix)]
    #[test]
    fn add_plugin_is_idempotent_on_repeated_calls() {
        let src_dir = tempdir().unwrap();
        let plugins_dir = tempdir().unwrap();
        let src = write_exec(src_dir.path(), "aura-plugin-demo", "echo {}\n");

        for _ in 0..2 {
            add_plugin(
                plugins_dir.path(),
                AddOptions {
                    source: src.clone(),
                    dest_name: None,
                    symlink: false,
                    name: None,
                    color: None,
                    icon: None,
                },
            )
            .unwrap();
        }

        let plugins = discover_plugins(plugins_dir.path());
        assert_eq!(plugins.len(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn add_plugin_skips_sidecar_when_no_metadata() {
        let src_dir = tempdir().unwrap();
        let plugins_dir = tempdir().unwrap();
        let src = write_exec(src_dir.path(), "aura-plugin-bare", "echo {}\n");

        let outcome = add_plugin(
            plugins_dir.path(),
            AddOptions {
                source: src,
                dest_name: None,
                symlink: false,
                name: None,
                color: None,
                icon: None,
            },
        )
        .unwrap();
        assert!(outcome.sidecar.is_none());
    }

    #[cfg(unix)]
    #[test]
    fn remove_plugin_deletes_binary_and_sidecar() {
        let src_dir = tempdir().unwrap();
        let plugins_dir = tempdir().unwrap();
        let src = write_exec(src_dir.path(), "aura-plugin-demo", "echo {}\n");

        let outcome = add_plugin(
            plugins_dir.path(),
            AddOptions {
                source: src,
                dest_name: None,
                symlink: false,
                name: Some("Demo".into()),
                color: None,
                icon: None,
            },
        )
        .unwrap();
        let sidecar = outcome.sidecar.unwrap();
        let installed = outcome.installed;

        let removed = remove_plugin(plugins_dir.path(), "Demo").unwrap();
        assert_eq!(removed.removed_binary, installed);
        assert_eq!(removed.removed_sidecar.as_ref(), Some(&sidecar));
        assert!(!installed.exists());
        assert!(!sidecar.exists());
    }

    #[cfg(unix)]
    #[test]
    fn remove_plugin_errors_on_unknown_name() {
        let plugins_dir = tempdir().unwrap();
        let err = remove_plugin(plugins_dir.path(), "ghost").unwrap_err();
        assert!(err.to_string().contains("ghost"));
    }

    #[test]
    fn derive_name_handles_common_prefixes() {
        assert_eq!(
            derive_name(&PathBuf::from("/x/aura-plugin-rtk-gains")),
            "Rtk Gains"
        );
        assert_eq!(
            derive_name(&PathBuf::from("/x/aura-plugin-hello.exe")),
            "Hello"
        );
        assert_eq!(derive_name(&PathBuf::from("/x/standalone")), "Standalone");
        assert_eq!(
            derive_name(&PathBuf::from("/x/snake_case_demo")),
            "Snake Case Demo"
        );
    }
}
