//! Antigravity quota windows, read out of `agy -p "/usage" --output-format json`.
//!
//! Unlike Claude Code and Codex, Antigravity is not reachable by reading a
//! token off disk and calling an endpoint: `agy` keeps its OAuth credentials
//! in the OS keyring, and the quota RPC
//! (`POST https://cloudcode-pa.googleapis.com/v1internal:retrieveUserQuotaSummary`)
//! is gated on the CLI's own client identity — a valid bearer token alone
//! comes back `403 You do not have a valid license of this product.` So the
//! supported path is the CLI's own documented `--output-format json` flag.
//!
//! Measured on `agy` 1.2.4: the slash command is answered locally from a
//! cached backend reading, so it costs no tokens and starts no LLM turn
//! (`num_turns: 0`). It takes ~3 s, dominated by the 207 MB Go binary's
//! startup, which is why the snapshot is cached for [`CACHE_TTL`].
//!
//! The numbers are the backend's, the same ones the Antigravity IDE shows, so
//! the snapshot is [`QuotaSource::Api`]. Any spawn, timeout, non-`SUCCESS`
//! status or parse failure sets `api_failed`, which is what makes
//! `tray_status::summarize_sticky` hold the previous reading instead of
//! degrading the tray icon.

use std::{
    io::Read,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{Mutex, OnceLock},
    time::{Duration, Instant},
};

use chrono::{DateTime, Utc};
use serde::Deserialize;

use crate::bin_path::{augmented_path, resolve_executable_in, search_summary};

use super::{QuotaSnapshot, QuotaSource, QuotaWindow};

/// Hard ceiling on the subprocess. `agy` normally answers in ~3 s; anything
/// past this is a hung binary, not a slow one.
const SPAWN_TIMEOUT: Duration = Duration::from_secs(10);

/// How long a reading stays fresh. The tray poll floor is 30 s and the modal
/// refreshes on open, so without this a modal opened right after a tray tick
/// would re-spawn the binary for a number it already has.
const CACHE_TTL: Duration = Duration::from_secs(20);

/// Default binary name, used when the agent profile sets no `command`.
const DEFAULT_COMMAND: &str = "agy";

/// Where Antigravity's own installer puts the binary, for cases where it
/// won't be on Aura's `PATH`.
///
/// On macOS and Linux that is `~/.local/bin`, which `bin_path` already
/// searches — and which the installer only adds to `PATH` by appending to the
/// user's *shell profile*, a file no GUI launcher or launchd agent ever reads.
///
/// Windows is the gap: the installer uses `%LOCALAPPDATA%\agy\bin`, which no
/// generic bin-directory list would guess. It does register that in the user
/// `PATH`, but a process started before the install — or a user who passed
/// `--skip-path` — won't see it.
fn agy_install_dirs() -> Vec<PathBuf> {
    #[cfg(target_os = "windows")]
    {
        std::env::var_os("LOCALAPPDATA")
            .map(|base| PathBuf::from(base).join("agy").join("bin"))
            .into_iter()
            .collect()
    }
    #[cfg(not(target_os = "windows"))]
    {
        Vec::new()
    }
}

// ── `/usage` JSON ────────────────────────────────────────────────────────────

/// Top level of `agy -p "/usage" --output-format json`.
///
/// Deliberately partial: `agy` also emits `conversation_id`, `response`,
/// `num_turns`, `usage` and `duration_seconds`, none of which we need. Unknown
/// fields are ignored so a future `agy` release adding keys doesn't break the
/// parse.
#[derive(Debug, Deserialize)]
struct UsageResponse {
    #[serde(default)]
    status: String,
    #[serde(default)]
    command: Option<UsageCommand>,
}

#[derive(Debug, Deserialize)]
struct UsageCommand {
    #[serde(default)]
    data: Option<UsageData>,
}

#[derive(Debug, Deserialize)]
struct UsageData {
    #[serde(default)]
    groups: Vec<UsageGroup>,
}

#[derive(Debug, Deserialize)]
struct UsageGroup {
    #[serde(default)]
    name: String,
    #[serde(default)]
    buckets: Vec<UsageBucket>,
}

#[derive(Debug, Deserialize)]
struct UsageBucket {
    /// `"gemini-weekly"`, `"gemini-5h"`, `"3p-weekly"`, `"3p-5h"`.
    #[serde(default)]
    id: String,
    /// `"weekly"` or `"5h"`.
    #[serde(default)]
    window: String,
    /// Fraction of the limit still available, 1.0 = untouched.
    #[serde(default)]
    remaining_fraction: f64,
    #[serde(default)]
    reset_time: Option<String>,
}

// ── Window naming and ordering ───────────────────────────────────────────────

/// Short label for a group, so the window reads `"Gemini · 5h"` rather than
/// repeating `agy`'s full `"Claude and GPT models"` in a narrow tray tooltip.
fn group_short_name(group: &str) -> &str {
    match group {
        "Gemini Models" => "Gemini",
        "Claude and GPT models" => "Claude/GPT",
        other => other,
    }
}

/// Human suffix for a bucket's window.
fn window_suffix(window: &str) -> &str {
    match window {
        "5h" => "5h",
        "weekly" => "week",
        other => other,
    }
}

/// Total length of a window, in minutes. Drives the Forecast tab, which
/// derives `started_at` from `resets_at - length`.
fn window_length_minutes(window: &str) -> Option<u32> {
    match window {
        "5h" => Some(300),
        "weekly" => Some(7 * 24 * 60),
        _ => None,
    }
}

/// Sort key that puts the windows in Aura's repo-wide order — session first,
/// week second — rather than the order `/usage` happens to print (which leads
/// with weekly). Position 0 and position 1 are what the `tray_progress_source`
/// / `tray_color_source` defaults read, so they have to mean the same thing on
/// this agent as on every other.
///
/// Gemini's own models come before the third-party group because that is the
/// limit an `agy` user burns by default.
fn bucket_rank(bucket: &UsageBucket) -> u8 {
    match bucket.id.as_str() {
        "gemini-5h" => 0,
        "gemini-weekly" => 1,
        "3p-5h" => 2,
        "3p-weekly" => 3,
        // Unknown bucket from a future release: keep it, but after the four
        // we know, and still session-before-week within its group.
        _ => match bucket.window.as_str() {
            "5h" => 4,
            _ => 5,
        },
    }
}

// ── Public source ────────────────────────────────────────────────────────────

pub struct AntigravityQuota {
    /// The `agy` executable as configured — the profile's `command`, or the
    /// bare default. Resolved to an absolute path at snapshot time by
    /// [`crate::bin_path::resolve_executable`].
    command: String,
    /// The agent's config dir (`~/.gemini/antigravity-cli`). Only used to pick
    /// a working directory for the subprocess.
    data_dir: PathBuf,
}

impl AntigravityQuota {
    pub fn new(data_dir: PathBuf, command: Option<&str>) -> Self {
        Self {
            command: command.unwrap_or(DEFAULT_COMMAND).to_string(),
            data_dir,
        }
    }

    /// Current quota windows. Never `Err` — every failure becomes an
    /// `Unavailable` snapshot carrying an actionable note.
    pub fn snapshot(&self) -> QuotaSnapshot {
        if let Some(cached) = cache_get(&self.command) {
            return cached;
        }
        let snap = self.snapshot_uncached();
        cache_put(&self.command, &snap);
        snap
    }

    fn snapshot_uncached(&self) -> QuotaSnapshot {
        let stdout = match self.run_usage() {
            Ok(out) => out,
            Err(e) => return failed(e),
        };
        parse_usage(&stdout)
    }

    /// Spawn `agy -p "/usage" --output-format json` and return its stdout.
    fn run_usage(&self) -> Result<String, String> {
        // Resolve to an absolute path before spawning rather than letting
        // `Command` search. Aura usually runs from a GUI launcher or a systemd
        // user unit, whose `PATH` routinely omits `~/.local/bin` — where `agy`
        // installs itself — so a bare name that resolves fine in a terminal
        // fails here. See `crate::bin_path`.
        let extra = agy_install_dirs();
        let Some(exe) = resolve_executable_in(&self.command, &extra) else {
            return Err(format!(
                "`{}` not found in {} — set `command` on this agent to its full path.",
                self.command,
                search_summary(&extra)
            ));
        };

        // Run from the data dir's parent (`~/.gemini`) rather than whatever
        // directory Aura was launched in: `agy` asks for workspace trust the
        // first time it sees a new project root, and a tray poll must never
        // sit on a prompt.
        let cwd = self
            .data_dir
            .parent()
            .filter(|p| p.is_dir())
            .map(Path::to_path_buf);

        let mut cmd = Command::new(&exe);
        cmd.args(["-p", "/usage", "--output-format", "json"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            // `agy` shells out to its own helpers; hand it the same widened
            // `PATH` plugins get rather than our GUI-inherited one.
            .env("PATH", augmented_path());
        if let Some(dir) = cwd {
            cmd.current_dir(dir);
        }
        // `aura` is built with `windows_subsystem = "windows"`, so Windows
        // opens a console window for every console-subsystem child it spawns.
        // `agy` is one, and this runs on every tray poll — without this the
        // user gets a CMD window flashing at them every 30 seconds. Same
        // treatment the plugin runner already applies.
        #[cfg(target_os = "windows")]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }

        let mut child = cmd
            .spawn()
            .map_err(|e| format!("could not start `{}`: {e}", exe.display()))?;

        // `wait_timeout` would need another dependency for one call site, so
        // poll `try_wait` instead. stdout is drained on a helper thread so a
        // response larger than the pipe buffer can't deadlock the wait.
        let mut pipe = child.stdout.take().expect("stdout is piped");
        let reader = std::thread::spawn(move || {
            let mut buf = String::new();
            pipe.read_to_string(&mut buf).map(|_| buf)
        });

        let deadline = Instant::now() + SPAWN_TIMEOUT;
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) if Instant::now() >= deadline => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(format!(
                        "`{}` timed out after {}s",
                        exe.display(),
                        SPAWN_TIMEOUT.as_secs()
                    ));
                }
                Ok(None) => std::thread::sleep(Duration::from_millis(50)),
                Err(e) => return Err(format!("waiting on `{}`: {e}", exe.display())),
            }
        };

        let stdout = reader
            .join()
            .map_err(|_| "stdout reader panicked".to_string())?
            .map_err(|e| format!("reading `{}` output: {e}", exe.display()))?;

        if !status.success() {
            // `agy` prints its own diagnosis to the log, not stdout, so the
            // exit code is all we can report. Not being logged in is the
            // common case ("error getting token source: You are not logged
            // into Antigravity.").
            return Err(format!(
                "`{}` exited with {status} — run `{} /usage` to check you are logged in",
                exe.display(),
                exe.display()
            ));
        }

        Ok(stdout)
    }
}

/// Turn `agy`'s stdout into a snapshot.
fn parse_usage(stdout: &str) -> QuotaSnapshot {
    // `agy` writes a single JSON object, but a stray banner line ahead of it
    // would be a cheap thing for a future release to add — find the object
    // rather than assuming it starts at byte 0.
    let json = match stdout.find('{') {
        Some(start) => &stdout[start..],
        None => return failed("`agy /usage` printed no JSON"),
    };

    let parsed: UsageResponse = match serde_json::from_str(json) {
        Ok(p) => p,
        Err(e) => return failed(format!("could not parse `agy /usage` output: {e}")),
    };

    if !parsed.status.eq_ignore_ascii_case("SUCCESS") {
        return failed(format!(
            "`agy /usage` returned status `{}` — check you are logged into Antigravity",
            parsed.status
        ));
    }

    let Some(data) = parsed.command.and_then(|c| c.data) else {
        return failed(
            "`agy /usage` returned no quota data — the response shape may have \
             changed in this `agy` release",
        );
    };

    // (rank, window) so the sort below is stable across groups.
    let mut ranked: Vec<(u8, QuotaWindow)> = Vec::new();
    for group in &data.groups {
        let short = group_short_name(&group.name);
        for bucket in &group.buckets {
            ranked.push((bucket_rank(bucket), to_window(short, bucket)));
        }
    }
    ranked.sort_by_key(|(rank, _)| *rank);
    let windows: Vec<QuotaWindow> = ranked.into_iter().map(|(_, w)| w).collect();

    if windows.is_empty() {
        return failed("`agy /usage` reported no quota windows");
    }

    QuotaSnapshot {
        // `/usage` doesn't name the plan, only what's left of it.
        subscription_type: None,
        windows,
        source: QuotaSource::Api,
        note: None,
        api_failed: false,
    }
}

fn to_window(group_short: &str, bucket: &UsageBucket) -> QuotaWindow {
    QuotaWindow {
        label: format!("{group_short} · {}", window_suffix(&bucket.window)),
        // Antigravity reports what's *left*; every other agent reports what's
        // been used, and so does the whole UI downstream.
        used_percentage: Some(((1.0 - bucket.remaining_fraction) * 100.0).clamp(0.0, 100.0)),
        // Quota is consumed cost-weighted, not per token — there is no token
        // count behind these fractions to report.
        used_tokens: None,
        resets_at: bucket.reset_time.as_deref().and_then(parse_reset_time),
        length_minutes: window_length_minutes(&bucket.window),
    }
}

fn parse_reset_time(s: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|d| d.with_timezone(&Utc))
}

/// An unavailable snapshot that also trips `api_failed`, so the tray keeps its
/// last good reading rather than blanking on a transient `agy` hiccup.
fn failed(reason: impl Into<String>) -> QuotaSnapshot {
    QuotaSnapshot {
        api_failed: true,
        ..QuotaSnapshot::unavailable(reason)
    }
}

// ── TTL cache ────────────────────────────────────────────────────────────────

/// Keyed by the configured command so two profiles pointing at different
/// `agy` installs don't read each other's numbers.
type Cache = Mutex<Vec<(String, Instant, QuotaSnapshot)>>;

fn cache() -> &'static Cache {
    static CACHE: OnceLock<Cache> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(Vec::new()))
}

fn cache_get(command: &str) -> Option<QuotaSnapshot> {
    let guard = cache().lock().ok()?;
    guard.iter().find_map(|(cmd, at, snap)| {
        (cmd == command && at.elapsed() < CACHE_TTL).then(|| snap.clone())
    })
}

fn cache_put(command: &str, snap: &QuotaSnapshot) {
    let Ok(mut guard) = cache().lock() else {
        return;
    };
    let now = Instant::now();
    match guard.iter_mut().find(|(cmd, _, _)| cmd == command) {
        Some(entry) => *entry = (command.to_string(), now, snap.clone()),
        None => guard.push((command.to_string(), now, snap.clone())),
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// Trimmed from a live `agy` 1.2.4 run. Note the group order: `/usage`
    /// prints weekly before 5h, which is the order we deliberately undo.
    const HEALTHY: &str = r#"{
      "conversation_id": "",
      "status": "SUCCESS",
      "num_turns": 0,
      "usage": { "input_tokens": 0, "output_tokens": 0, "total_tokens": 0 },
      "command": {
        "name": "usage",
        "data": {
          "description": "Within each group, models share a weekly limit and a 5-hour limit.",
          "groups": [
            {
              "name": "Gemini Models",
              "description": "Models within this group: Gemini Flash, Gemini Pro",
              "buckets": [
                { "id": "gemini-weekly", "name": "Weekly Limit Remaining", "window": "weekly",
                  "remaining_fraction": 0.75, "reset_time": "2026-09-23T22:57:05Z" },
                { "id": "gemini-5h", "name": "Five Hour Limit Remaining", "window": "5h",
                  "remaining_fraction": 0.9, "reset_time": "2026-09-17T03:57:05Z" }
              ]
            },
            {
              "name": "Claude and GPT models",
              "description": "Models within this group: Claude Opus, Claude Sonnet, GPT-OSS",
              "buckets": [
                { "id": "3p-weekly", "name": "Weekly Limit Remaining", "window": "weekly",
                  "remaining_fraction": 1, "reset_time": "2026-09-24T01:05:28Z" },
                { "id": "3p-5h", "name": "Five Hour Limit Remaining", "window": "5h",
                  "remaining_fraction": 0.5, "reset_time": "2026-09-17T06:05:28Z" }
              ]
            }
          ]
        }
      }
    }"#;

    #[test]
    fn healthy_response_parses_as_api_source() {
        let snap = parse_usage(HEALTHY);
        assert_eq!(snap.source, QuotaSource::Api);
        assert!(!snap.api_failed);
        assert!(snap.note.is_none());
        assert!(snap.subscription_type.is_none());
        assert_eq!(snap.windows.len(), 4);
    }

    #[test]
    fn windows_are_reordered_session_before_week() {
        let snap = parse_usage(HEALTHY);
        let labels: Vec<&str> = snap.windows.iter().map(|w| w.label.as_str()).collect();
        assert_eq!(
            labels,
            [
                "Gemini · 5h",
                "Gemini · week",
                "Claude/GPT · 5h",
                "Claude/GPT · week"
            ]
        );
    }

    #[track_caller]
    fn assert_pct(actual: Option<f64>, expected: f64) {
        let actual = actual.expect("window reports a percentage");
        assert!(
            (actual - expected).abs() < 1e-9,
            "expected ~{expected}%, got {actual}%",
        );
    }

    #[test]
    fn remaining_fraction_is_inverted_into_used_percentage() {
        let snap = parse_usage(HEALTHY);
        // remaining 0.9 → used 10%; remaining 0.75 → used 25%.
        assert_pct(snap.windows[0].used_percentage, 10.0);
        assert_pct(snap.windows[1].used_percentage, 25.0);
        // remaining 1.0 → used 0%, not a missing reading.
        assert_pct(snap.windows[3].used_percentage, 0.0);
    }

    #[test]
    fn windows_carry_length_so_forecast_can_project() {
        let snap = parse_usage(HEALTHY);
        assert_eq!(snap.windows[0].length_minutes, Some(300));
        assert_eq!(snap.windows[1].length_minutes, Some(10_080));
        // Cost-weighted fractions, not tokens.
        assert!(snap.windows.iter().all(|w| w.used_tokens.is_none()));
    }

    #[test]
    fn reset_times_are_parsed() {
        let snap = parse_usage(HEALTHY);
        assert_eq!(
            snap.windows[0].resets_at.map(|t| t.to_rfc3339()),
            Some("2026-09-17T03:57:05+00:00".to_string())
        );
    }

    #[test]
    fn forecast_projects_every_window() {
        let snap = parse_usage(HEALTHY);
        // A window with no `length_minutes` is silently dropped by the
        // Forecast tab — assert all four survive.
        let reset = snap.windows[0].resets_at.unwrap();
        let fc = crate::quota::forecast::forecast(&snap, reset - chrono::Duration::hours(1));
        assert_eq!(fc.windows.len(), 4);
    }

    #[test]
    fn not_logged_in_fails_without_degrading_the_tray() {
        // `agy` answers a logged-out `/usage` with a non-SUCCESS status.
        let out = r#"{"conversation_id":"","status":"ERROR","response":"error getting token source: You are not logged into Antigravity.","num_turns":0}"#;
        let snap = parse_usage(out);
        assert_eq!(snap.source, QuotaSource::Unavailable);
        assert!(snap.api_failed, "tray must hold its last good reading");
        assert!(snap.note.unwrap().contains("logged into Antigravity"));
        assert!(snap.windows.is_empty());
    }

    #[test]
    fn malformed_json_is_reported_not_panicked() {
        let snap = parse_usage("{not json at all");
        assert_eq!(snap.source, QuotaSource::Unavailable);
        assert!(snap.api_failed);
        assert!(snap.note.unwrap().contains("could not parse"));
    }

    #[test]
    fn no_json_at_all_is_reported() {
        let snap = parse_usage("agy: command panicked\n");
        assert_eq!(snap.source, QuotaSource::Unavailable);
        assert!(snap.note.unwrap().contains("printed no JSON"));
    }

    #[test]
    fn missing_command_data_names_the_shape_change() {
        let out = r#"{"status":"SUCCESS","command":{"name":"usage"}}"#;
        let snap = parse_usage(out);
        assert_eq!(snap.source, QuotaSource::Unavailable);
        assert!(snap
            .note
            .unwrap()
            .contains("response shape may have changed"));
    }

    #[test]
    fn empty_groups_are_reported_rather_than_shown_as_zero_usage() {
        let out = r#"{"status":"SUCCESS","command":{"name":"usage","data":{"groups":[]}}}"#;
        let snap = parse_usage(out);
        assert_eq!(snap.source, QuotaSource::Unavailable);
        assert!(snap.note.unwrap().contains("no quota windows"));
    }

    #[test]
    fn unknown_fields_and_unknown_buckets_survive() {
        // A future `agy` adds a group and a key we've never seen.
        let out = r#"{
          "status": "SUCCESS",
          "brand_new_top_level_key": 42,
          "command": { "name": "usage", "data": { "groups": [
            { "name": "Imagen Models", "buckets": [
              { "id": "imagen-5h", "window": "5h", "remaining_fraction": 0.2,
                "reset_time": "2026-09-17T06:05:28Z", "brand_new_bucket_key": true }
            ]}
          ]}}
        }"#;
        let snap = parse_usage(out);
        assert_eq!(snap.source, QuotaSource::Api);
        assert_eq!(snap.windows.len(), 1);
        // Unrecognised group name passes through verbatim.
        assert_eq!(snap.windows[0].label, "Imagen Models · 5h");
        assert_pct(snap.windows[0].used_percentage, 80.0);
    }

    #[test]
    fn unknown_buckets_sort_after_the_known_four() {
        let out = r#"{
          "status": "SUCCESS",
          "command": { "name": "usage", "data": { "groups": [
            { "name": "Imagen Models", "buckets": [
              { "id": "imagen-weekly", "window": "weekly", "remaining_fraction": 1 },
              { "id": "imagen-5h", "window": "5h", "remaining_fraction": 1 }
            ]},
            { "name": "Gemini Models", "buckets": [
              { "id": "gemini-weekly", "window": "weekly", "remaining_fraction": 1 },
              { "id": "gemini-5h", "window": "5h", "remaining_fraction": 1 }
            ]}
          ]}}
        }"#;
        let snap = parse_usage(out);
        let labels: Vec<&str> = snap.windows.iter().map(|w| w.label.as_str()).collect();
        // Positions 0 and 1 still mean session and week on the group the
        // tray defaults read.
        assert_eq!(
            labels,
            [
                "Gemini · 5h",
                "Gemini · week",
                "Imagen Models · 5h",
                "Imagen Models · week"
            ]
        );
    }

    #[test]
    fn banner_before_the_json_is_skipped() {
        let out = format!("Updating to agy 1.2.5...\n{HEALTHY}");
        let snap = parse_usage(&out);
        assert_eq!(snap.source, QuotaSource::Api);
        assert_eq!(snap.windows.len(), 4);
    }

    #[test]
    fn missing_binary_is_reported_without_panicking() {
        // Spawning is reached only through `command`, so a path that cannot
        // exist exercises the failure branch without invoking the real `agy`.
        let q = AntigravityQuota::new(
            std::env::temp_dir(),
            Some("aura-test-no-such-binary-0e1f2a3b"),
        );
        let snap = q.snapshot();
        assert_eq!(snap.source, QuotaSource::Unavailable);
        assert!(snap.api_failed);
        let note = snap.note.unwrap();
        assert!(note.contains("not found"), "{note}");
        // The note has to say where we looked and what to do — "not found on
        // PATH" is not actionable when the failure is that our PATH is the
        // wrong one.
        assert!(note.contains("not found in"), "{note}");
        assert!(note.contains("`command`"), "{note}");
    }
}
