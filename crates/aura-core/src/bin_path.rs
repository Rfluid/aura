//! Finding executables when Aura's own `PATH` is the wrong one.
//!
//! Aura normally runs from a GUI launcher — a Linux `.desktop` file, a macOS
//! `.app` bundle, a systemd user unit — and none of those source the user's
//! shell rc files. The inherited `PATH` is whatever the session manager set,
//! which routinely omits the per-user bin directories where CLI tools install
//! themselves. On the reference machine the systemd user manager exports:
//!
//! ```text
//! /home/u/.cargo/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin:…
//! ```
//!
//! — no `~/.local/bin`, which is exactly where `agy` (and plenty else) lands.
//! Running the same binary from an interactive shell works, which is what
//! makes this failure mode confusing to hit.
//!
//! Two consumers, two different needs:
//!
//! - [`augmented_path`] builds the `PATH` handed to a *child* process, so a
//!   tool Aura spawns can find the tools *it* shells out to.
//! - [`resolve_executable`] finds a binary for Aura itself, returning an
//!   absolute path. Setting the child's `PATH` is not enough on its own:
//!   Unix resolves a bare program name against the `PATH` you hand the child,
//!   but Windows resolves it against the *parent's*. Resolving up front works
//!   the same way everywhere, and lets the caller say which directories it
//!   looked in when nothing turns up.

use std::{
    collections::HashSet,
    ffi::OsString,
    path::{Path, PathBuf},
};

/// Per-user bin directories to look in ahead of the inherited `PATH`.
/// Relative to the user's home.
const HOME_BIN_DIRS: &[&str] = &[".local/bin", ".cargo/bin", ".bun/bin", "bin"];

/// System bin directories a GUI `PATH` may omit. `/opt/homebrew/bin` is Apple
/// Silicon Homebrew; harmless elsewhere, since a missing directory is skipped.
const SYSTEM_BIN_DIRS: &[&str] = &["/opt/homebrew/bin", "/usr/local/bin"];

/// Every directory to search, most specific first: the per-user bin dirs, the
/// system ones, then whatever `PATH` already had.
pub fn search_dirs() -> Vec<PathBuf> {
    search_dirs_in(&[])
}

/// [`search_dirs`] with caller-supplied directories tried first — for a tool
/// whose installer uses a location no generic list would guess. Directories
/// that don't exist are dropped, so a caller can pass a platform's path
/// unconditionally.
pub fn search_dirs_in(extra: &[PathBuf]) -> Vec<PathBuf> {
    search_dirs_from(extra, dirs::home_dir(), std::env::var_os("PATH"))
}

fn search_dirs_from(
    extra: &[PathBuf],
    home: Option<PathBuf>,
    existing: Option<OsString>,
) -> Vec<PathBuf> {
    let mut entries: Vec<PathBuf> = Vec::new();
    let mut seen: HashSet<PathBuf> = HashSet::new();

    let mut push = |p: PathBuf| {
        if p.is_dir() && seen.insert(p.clone()) {
            entries.push(p);
        }
    };

    for dir in extra {
        push(dir.clone());
    }
    if let Some(home) = home {
        for sub in HOME_BIN_DIRS {
            push(home.join(sub));
        }
    }
    for p in SYSTEM_BIN_DIRS {
        push(PathBuf::from(p));
    }
    if let Some(existing) = existing.as_ref() {
        for p in std::env::split_paths(existing) {
            if !p.as_os_str().is_empty() {
                push(p);
            }
        }
    }
    entries
}

/// The `PATH` to hand a spawned child process.
pub fn augmented_path() -> OsString {
    augmented_path_from(dirs::home_dir(), std::env::var_os("PATH"))
}

pub(crate) fn augmented_path_from(home: Option<PathBuf>, existing: Option<OsString>) -> OsString {
    let entries = search_dirs_from(&[], home, existing.clone());
    std::env::join_paths(entries).unwrap_or_else(|_| existing.unwrap_or_default())
}

/// A short human summary of where [`resolve_executable`] looked, for an error
/// message. The full list runs to dozens of entries on a developer machine and
/// reads as noise in a tray tooltip, so this names the first few directories —
/// the per-user ones Aura adds, which are the interesting part — and counts the
/// rest. Paths under the user's home are shortened back to `~`.
pub fn search_summary(extra: &[PathBuf]) -> String {
    const SHOWN: usize = 3;
    let home = dirs::home_dir();
    let dirs = search_dirs_in(extra);

    let pretty = |p: &Path| match home.as_ref().and_then(|h| p.strip_prefix(h).ok()) {
        Some(rest) => format!("~/{}", rest.display()),
        None => p.display().to_string(),
    };

    let head: Vec<String> = dirs.iter().take(SHOWN).map(|p| pretty(p)).collect();
    match dirs.len().saturating_sub(head.len()) {
        0 => head.join(", "),
        n => format!("{} and {n} more on PATH", head.join(", ")),
    }
}

/// Find `command` as an absolute path.
///
/// - A leading `~` is expanded, and anything that already looks like a path
///   (contains a separator) is taken at its word — existence is still checked,
///   so a stale `command =` in config reports as "not found" rather than as a
///   spawn error later.
/// - A bare name is searched for across [`search_dirs`].
///
/// Returns `None` when nothing matches; [`search_dirs`] is what the caller
/// should name in the resulting error.
pub fn resolve_executable(command: &str) -> Option<PathBuf> {
    resolve_executable_in(command, &[])
}

/// [`resolve_executable`] with extra directories searched first. See
/// [`search_dirs_in`].
pub fn resolve_executable_in(command: &str, extra: &[PathBuf]) -> Option<PathBuf> {
    let expanded = expand_tilde(command);
    if expanded.components().count() > 1 || command.starts_with('~') {
        return is_executable(&expanded).then_some(expanded);
    }
    search_dirs_in(extra)
        .into_iter()
        .flat_map(|dir| candidate_names(command).map(move |name| dir.join(name)))
        .find(|p| is_executable(p))
}

/// Filenames to try for a bare command name. Unix has exactly one; Windows
/// needs the `PATHEXT` suffixes, since `foo` on disk is `foo.exe` or `foo.cmd`.
fn candidate_names(command: &str) -> impl Iterator<Item = String> + '_ {
    #[cfg(windows)]
    let exts: &[&str] = &["", ".exe", ".cmd", ".bat", ".com"];
    #[cfg(not(windows))]
    let exts: &[&str] = &[""];
    exts.iter().map(move |ext| format!("{command}{ext}"))
}

fn expand_tilde(path: &str) -> PathBuf {
    let home = || dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"));
    if let Some(rest) = path.strip_prefix("~/") {
        home().join(rest)
    } else if path == "~" {
        home()
    } else {
        PathBuf::from(path)
    }
}

#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

#[cfg(not(unix))]
fn is_executable(path: &Path) -> bool {
    path.is_file()
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    /// A `PATH` value spelled the way the host does it — `:` on Unix, `;` on
    /// Windows. Building one by hand with `:` made these tests silently
    /// degenerate on Windows: the whole string parsed as a single entry, which
    /// then failed the `is_dir` filter and vanished.
    fn path_env(dirs: &[&Path]) -> OsString {
        std::env::join_paths(dirs).unwrap()
    }

    /// A real directory to stand in for an inherited `PATH` entry. It has to
    /// exist, because `search_dirs_from` drops entries that don't — so
    /// hardcoding `/usr/bin` tested nothing on Windows.
    fn existing_dir(root: &Path, name: &str) -> PathBuf {
        let path = root.join(name);
        fs::create_dir_all(&path).unwrap();
        path
    }

    #[cfg(unix)]
    fn write_exe(dir: &Path, name: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        fs::create_dir_all(dir).unwrap();
        let path = dir.join(name);
        fs::write(&path, "#!/bin/sh\ntrue\n").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    #[test]
    fn home_bin_dirs_come_before_the_inherited_path() {
        // The whole point: a GUI `PATH` that omits `~/.local/bin` still finds
        // what the user installed there.
        let home = tempdir().unwrap();
        fs::create_dir_all(home.path().join(".local/bin")).unwrap();
        let inherited = existing_dir(home.path(), "inherited-bin");

        let dirs = search_dirs_from(
            &[],
            Some(home.path().to_path_buf()),
            Some(path_env(&[&inherited])),
        );
        assert_eq!(dirs.first(), Some(&home.path().join(".local/bin")));
        assert!(dirs.contains(&inherited));
    }

    #[test]
    fn missing_directories_are_skipped() {
        let home = tempdir().unwrap();
        // None of HOME_BIN_DIRS exist under this home.
        let dirs = search_dirs_from(
            &[],
            Some(home.path().to_path_buf()),
            Some(OsString::from("")),
        );
        assert!(dirs.iter().all(|d| !d.starts_with(home.path())));
    }

    #[test]
    fn duplicate_entries_are_collapsed() {
        let home = tempdir().unwrap();
        let local = home.path().join(".local/bin");
        fs::create_dir_all(&local).unwrap();

        // The same dir arrives twice: once from HOME_BIN_DIRS, once from PATH.
        let dirs = search_dirs_from(
            &[],
            Some(home.path().to_path_buf()),
            Some(path_env(&[&local])),
        );
        assert_eq!(dirs.iter().filter(|d| **d == local).count(), 1);
    }

    #[test]
    fn augmented_path_keeps_the_inherited_entries() {
        let home = tempdir().unwrap();
        fs::create_dir_all(home.path().join(".local/bin")).unwrap();

        let inherited = existing_dir(home.path(), "inherited-bin");

        let merged = augmented_path_from(
            Some(home.path().to_path_buf()),
            Some(path_env(&[&inherited])),
        );
        let entries: Vec<PathBuf> = std::env::split_paths(&merged).collect();
        assert!(entries.contains(&inherited), "{entries:?}");
        assert!(
            entries.contains(&home.path().join(".local/bin")),
            "{entries:?}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn resolve_finds_a_bare_name_in_a_home_bin_dir() {
        let home = tempdir().unwrap();
        let expected = write_exe(&home.path().join(".local/bin"), "aura-test-tool");

        // Point the resolver's notion of home at the tempdir, and give it a
        // PATH that deliberately doesn't contain the tool.
        let prev_home = std::env::var_os("HOME");
        let prev_path = std::env::var_os("PATH");
        std::env::set_var("HOME", home.path());
        std::env::set_var("PATH", "/nonexistent-for-this-test");

        let found = resolve_executable("aura-test-tool");

        match prev_home {
            Some(h) => std::env::set_var("HOME", h),
            None => std::env::remove_var("HOME"),
        }
        match prev_path {
            Some(p) => std::env::set_var("PATH", p),
            None => std::env::remove_var("PATH"),
        }

        assert_eq!(found.as_deref(), Some(expected.as_path()));
    }

    #[cfg(unix)]
    #[test]
    fn resolve_rejects_a_non_executable_file() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("not-executable");
        fs::write(&path, "data").unwrap();
        assert_eq!(resolve_executable(path.to_str().unwrap()), None);
    }

    #[cfg(unix)]
    #[test]
    fn resolve_takes_an_absolute_path_as_given() {
        let dir = tempdir().unwrap();
        let exe = write_exe(dir.path(), "tool");
        assert_eq!(
            resolve_executable(exe.to_str().unwrap()).as_deref(),
            Some(exe.as_path())
        );
    }

    #[test]
    fn extra_dirs_are_searched_before_everything_else() {
        // How a caller reaches an installer-specific location no generic list
        // would guess — e.g. `%LOCALAPPDATA%\\agy\\bin` on Windows.
        let home = tempdir().unwrap();
        let extra = home.path().join("vendor-specific");
        fs::create_dir_all(&extra).unwrap();
        fs::create_dir_all(home.path().join(".local/bin")).unwrap();

        let inherited = existing_dir(home.path(), "inherited-bin");
        let dirs = search_dirs_from(
            std::slice::from_ref(&extra),
            Some(home.path().to_path_buf()),
            Some(path_env(&[&inherited])),
        );
        assert_eq!(dirs.first(), Some(&extra));
    }

    #[test]
    fn extra_dirs_that_do_not_exist_are_dropped() {
        // Callers pass a platform's path unconditionally; the other platforms
        // must not end up naming a directory that cannot exist there.
        let home = tempdir().unwrap();
        let missing = home.path().join("not-installed");
        let inherited = existing_dir(home.path(), "inherited-bin");
        let dirs = search_dirs_from(
            std::slice::from_ref(&missing),
            Some(home.path().to_path_buf()),
            Some(path_env(&[&inherited])),
        );
        assert!(!dirs.contains(&missing));
    }

    #[test]
    fn search_summary_stays_short_enough_for_a_tooltip() {
        // A developer machine has dozens of PATH entries; the note this feeds
        // has to stay one readable line, so at most three are named and the
        // rest are counted.
        let summary = search_summary(&[]);
        assert!(!summary.is_empty());
        assert!(
            summary.matches(", ").count() <= 2,
            "too many directories listed: {summary}"
        );
    }

    #[test]
    fn resolve_reports_a_missing_binary_rather_than_guessing() {
        assert_eq!(resolve_executable("aura-no-such-binary-9f3c1d"), None);
        assert_eq!(resolve_executable("/nope/aura-no-such-binary-9f3c1d"), None);
    }
}
