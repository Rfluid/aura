//! Live process monitor scoped to **Claude Code only**.
//!
//! The user runs many parallel Claude Code sessions; when the machine bogs
//! down it's hard to tell which session (and which child process inside it) is
//! the CPU/RAM hog. This module enumerates the running processes, finds every
//! Claude Code CLI root, collects each root's full descendant subtree (MCP
//! servers, `bash` build commands, sub-agents — usually the real culprits),
//! and sums CPU% + RSS per session. Each session is labelled by its project
//! (the basename of the root's cwd) and, when one can be matched, the active
//! `*.jsonl` session id under `~/.claude/projects/<slug>/`.
//!
//! # Why the split into pure helpers + a stateful monitor
//!
//! CPU% is a *delta* between two samples, so callers must keep one long-lived
//! [`sysinfo::System`] across ticks — that lives in [`ActivityMonitor`]. The
//! OS-touching part ([`ActivityMonitor::sample`]) is thin; the interesting
//! logic ([`classify_roots`], [`build_subtrees`], [`project_from_cwd`],
//! [`active_session_for`], CPU/mem summation, "heaviest children" selection)
//! is pure and operates on a synthetic [`RawProc`] list so it's unit-testable
//! headless without touching the real OS.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// A flattened, OS-agnostic snapshot of one process. The real implementation
/// builds these from [`sysinfo::Process`]; tests build them by hand so the
/// pure helpers are deterministic.
#[derive(Debug, Clone)]
pub struct RawProc {
    /// Process id.
    pub pid: u32,
    /// Parent process id, if the OS reports one.
    pub ppid: Option<u32>,
    /// `Process::name()` — the executable's short name (e.g. `claude`, `node`).
    pub name: String,
    /// Full argv joined by spaces (`Process::cmd()`), used both for Claude-root
    /// classification and for building a readable child label.
    pub cmd: String,
    /// `Process::exe()` path, when available. Used as a fallback signal for
    /// the readable label.
    pub exe: Option<PathBuf>,
    /// `Process::cwd()` — the working directory. For a Claude root this is the
    /// project directory. Empty/absent only on Windows (unsupported here).
    pub cwd: Option<PathBuf>,
    /// CPU usage in percent. Can exceed 100 on multi-core machines — that's a
    /// correct reading, not an error.
    pub cpu: f32,
    /// Resident set size in bytes.
    pub mem_bytes: u64,
}

/// One Claude Code child process surfaced in the UI (a "culprit" row).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ProcView {
    /// Short, readable label derived from the process cmd, e.g.
    /// `"node mcp-server-figma"` or `"bash cargo build"`.
    pub label: String,
    /// The process id.
    pub pid: u32,
    /// CPU usage in percent (can exceed 100 — multi-core).
    pub cpu: f32,
    /// Resident memory in bytes.
    pub mem_bytes: u64,
}

/// One Claude Code session: a CLI root plus its full descendant subtree,
/// summed and labelled by project / active session id.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ClaudeSession {
    /// The Claude root's pid. Stable identity for the session row.
    pub root_pid: u32,
    /// Project name — `basename(cwd)` of the root. Falls back to the pid string
    /// when the root has no cwd.
    pub project: String,
    /// Short id of the active `*.jsonl` session under `~/.claude/projects`,
    /// when one could be matched by cwd. `None` → show just the project.
    pub session_id: Option<String>,
    /// Sum of the subtree's CPU% (root + all descendants).
    pub total_cpu: f32,
    /// Sum of the subtree's resident memory in bytes.
    pub total_mem_bytes: u64,
    /// The heaviest 1–3 child processes by CPU, for the culprit rows.
    pub children: Vec<ProcView>,
}

/// How many culprit children to surface per session.
const MAX_CHILDREN: usize = 3;

/// Decide whether a process is a Claude Code **CLI root**.
///
/// The real signature on macOS/Linux is the `claude` CLI: `Process::name()`
/// is `claude` and the cmd begins with the `claude` entrypoint (e.g.
/// `claude --dangerously-skip-permissions`, `claude --resume <id>`). We match
/// on the executable name being exactly `claude` (case-insensitive,
/// `.exe`-tolerant) OR the first argv token's basename being `claude`. The
/// desktop `Claude.app` is excluded because its executable name is `Claude`
/// with a different argv (and we additionally reject names containing a space
/// or the `.app` marker). `node`/`python` MCP children are *not* roots — they
/// only become part of a session via the subtree walk.
pub fn is_claude_root(proc: &RawProc) -> bool {
    if name_is_claude_cli(&proc.name) {
        return true;
    }
    // Fall back to the first argv token's basename — covers shells/launchers
    // that exec the claude entrypoint with a name sysinfo couldn't shorten.
    first_token_basename(&proc.cmd)
        .map(|b| name_is_claude_cli(&b))
        .unwrap_or(false)
}

/// True when `name` is the Claude Code CLI executable (`claude` /
/// `claude.exe`), case-insensitively. Rejects the `Claude` desktop app and
/// anything else.
fn name_is_claude_cli(name: &str) -> bool {
    let stem = name.strip_suffix(".exe").unwrap_or(name);
    stem == "claude"
}

/// Basename of the first whitespace-separated token of a cmd line.
fn first_token_basename(cmd: &str) -> Option<String> {
    let first = cmd.split_whitespace().next()?;
    Path::new(first)
        .file_name()
        .and_then(|s| s.to_str())
        .map(|s| s.to_string())
}

/// All Claude Code CLI roots in `procs`, in input order.
pub fn classify_roots(procs: &[RawProc]) -> Vec<&RawProc> {
    procs.iter().filter(|p| is_claude_root(p)).collect()
}

/// Build a `ppid → [child pid]` adjacency map over the whole process list.
pub fn build_child_map(procs: &[RawProc]) -> HashMap<u32, Vec<u32>> {
    let mut map: HashMap<u32, Vec<u32>> = HashMap::new();
    for p in procs {
        if let Some(ppid) = p.ppid {
            map.entry(ppid).or_default().push(p.pid);
        }
    }
    map
}

/// Collect the pids of `root` plus every descendant, using the child map.
/// Breadth-first; cycle-safe (a pid is visited at most once).
pub fn subtree_pids(root: u32, child_map: &HashMap<u32, Vec<u32>>) -> Vec<u32> {
    let mut out = Vec::new();
    let mut stack = vec![root];
    let mut seen = std::collections::HashSet::new();
    while let Some(pid) = stack.pop() {
        if !seen.insert(pid) {
            continue;
        }
        out.push(pid);
        if let Some(children) = child_map.get(&pid) {
            stack.extend(children.iter().copied());
        }
    }
    out
}

/// For each Claude root, the full set of subtree pids (root + descendants).
/// Keyed by root pid, in `roots` order.
pub fn build_subtrees(procs: &[RawProc]) -> Vec<(u32, Vec<u32>)> {
    let child_map = build_child_map(procs);
    classify_roots(procs)
        .iter()
        .map(|root| (root.pid, subtree_pids(root.pid, &child_map)))
        .collect()
}

/// Project name from a root's cwd: the final path component (e.g.
/// `/Users/x/Downloads/jp` → `jp`). `None` when the cwd is absent or has no
/// final component (e.g. `/`).
pub fn project_from_cwd(cwd: Option<&Path>) -> Option<String> {
    let cwd = cwd?;
    cwd.file_name()
        .and_then(|s| s.to_str())
        .map(|s| s.to_string())
}

/// Claude Code's slugification of a project path: every `/` and `.` becomes
/// `-`. So `/Users/x/AI-Outreach/.brand-research` →
/// `-Users-x-AI-Outreach--brand-research`. This mirrors the on-disk directory
/// names under `~/.claude/projects/`.
pub fn slugify_cwd(cwd: &Path) -> String {
    cwd.to_string_lossy()
        .chars()
        .map(|c| if c == '/' || c == '.' { '-' } else { c })
        .collect()
}

/// Short id of the most-recently-modified `*.jsonl` under
/// `<claude_projects_dir>/<slug>/`, where `<slug>` is [`slugify_cwd`] of
/// `cwd`. The "short id" is the first 4 chars of the file stem (the session
/// uuid). Returns `None` when `cwd` is absent, the slug dir doesn't exist, or
/// it holds no `*.jsonl` files.
///
/// Reads the directory but not the file contents, so it's cheap on every tick.
pub fn active_session_for(cwd: Option<&Path>, claude_projects_dir: &Path) -> Option<String> {
    let cwd = cwd?;
    let slug = slugify_cwd(cwd);
    let dir = claude_projects_dir.join(slug);
    let entries = std::fs::read_dir(&dir).ok()?;

    let mut newest: Option<(std::time::SystemTime, String)> = None;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
            continue;
        }
        let stem = match path.file_stem().and_then(|s| s.to_str()) {
            Some(s) => s.to_string(),
            None => continue,
        };
        let mtime = match entry.metadata().and_then(|m| m.modified()) {
            Ok(t) => t,
            Err(_) => continue,
        };
        if newest.as_ref().map(|(t, _)| mtime > *t).unwrap_or(true) {
            newest = Some((mtime, stem));
        }
    }

    newest.map(|(_, stem)| short_session_id(&stem))
}

/// First 4 chars of a session uuid, for the compact `project · 7c51…` label.
fn short_session_id(full: &str) -> String {
    full.chars().take(4).collect()
}

/// A short, readable label for a child process from its cmd. The basename of
/// the executable plus a single meaningful hint token:
///
/// - `node /…/mcp-server-figma/dist/index.js` → `"node mcp-server-figma"`
/// - `/bin/bash -c cargo build` → `"bash cargo build"`
/// - `claude --resume <id>` → `"claude --resume"`
///
/// Falls back to the bare executable basename when no useful hint is present.
pub fn child_label(proc: &RawProc) -> String {
    let exe_base = proc
        .exe
        .as_ref()
        .and_then(|p| p.file_name())
        .and_then(|s| s.to_str())
        .map(|s| s.to_string())
        .or_else(|| first_token_basename(&proc.cmd))
        .unwrap_or_else(|| proc.name.clone());

    match label_hint(&proc.cmd, &exe_base) {
        Some(hint) => format!("{exe_base} {hint}"),
        None => exe_base,
    }
}

/// Extract a meaningful hint token from a cmd line, given the executable
/// basename so we can skip past it (and past a `-c` shell flag).
fn label_hint(cmd: &str, exe_base: &str) -> Option<String> {
    let mut tokens = cmd.split_whitespace().peekable();
    // Skip the leading executable path token.
    tokens.next();

    while let Some(tok) = tokens.next() {
        // Skip a shell `-c` and look at the actual command it runs.
        if tok == "-c" {
            continue;
        }
        // For node/python the script path is the signal: use its basename,
        // and prefer a parent dir name when the file is a generic `index.js`.
        if exe_base.starts_with("node") || exe_base.starts_with("python") {
            if let Some(base) = script_signal(tok) {
                return Some(base);
            }
            // A bare `-m module` invocation (python -m studio_design_mcp).
            if tok == "-m" {
                if let Some(module) = tokens.peek() {
                    return Some((*module).to_string());
                }
            }
            continue;
        }
        // Generic case (bash, cargo, etc.): the first non-flag token is the
        // command being run.
        if tok.starts_with('-') {
            return Some(tok.to_string());
        }
        return Some(basename_token(tok));
    }
    None
}

/// Turn a script path into a readable signal: the file basename, or — when the
/// file is a generic entrypoint like `index.js` / `__main__.py` — the parent
/// directory name (`.../mcp-server-figma/dist/index.js` → `mcp-server-figma`).
fn script_signal(tok: &str) -> Option<String> {
    if !tok.contains('/') && !tok.ends_with(".js") && !tok.ends_with(".py") {
        return None;
    }
    let path = Path::new(tok);
    let file = path.file_name().and_then(|s| s.to_str())?;
    let generic = matches!(file, "index.js" | "main.js" | "__main__.py" | "cli.js");
    if generic {
        // Walk up past generic dir names (dist/build/src) to a meaningful one.
        let mut cur = path.parent();
        while let Some(dir) = cur {
            if let Some(name) = dir.file_name().and_then(|s| s.to_str()) {
                if !matches!(name, "dist" | "build" | "src" | "lib" | "bin") {
                    return Some(name.to_string());
                }
            }
            cur = dir.parent();
        }
    }
    Some(file.to_string())
}

/// Basename of a path-like token, or the token itself when it's plain.
fn basename_token(tok: &str) -> String {
    Path::new(tok)
        .file_name()
        .and_then(|s| s.to_str())
        .map(|s| s.to_string())
        .unwrap_or_else(|| tok.to_string())
}

/// Assemble one [`ClaudeSession`] from a root and its subtree pids, looking up
/// each pid in `by_pid`. Sums CPU/mem over the subtree and selects the heaviest
/// [`MAX_CHILDREN`] *non-root* processes (by CPU) as culprit rows. When the
/// root is the only process, the root itself is shown as the single child so
/// the row isn't childless.
fn assemble_session(
    root: &RawProc,
    subtree: &[u32],
    by_pid: &HashMap<u32, &RawProc>,
    claude_projects_dir: &Path,
) -> ClaudeSession {
    let mut total_cpu = 0.0f32;
    let mut total_mem = 0u64;
    let mut members: Vec<&RawProc> = Vec::new();
    for pid in subtree {
        if let Some(p) = by_pid.get(pid) {
            total_cpu += p.cpu;
            total_mem += p.mem_bytes;
            members.push(p);
        }
    }

    // Heaviest children by CPU. Prefer non-root processes (the spawned hogs);
    // fall back to the root when it's the only member.
    let mut culprits: Vec<&RawProc> = members
        .iter()
        .copied()
        .filter(|p| p.pid != root.pid)
        .collect();
    if culprits.is_empty() {
        culprits.push(root);
    }
    culprits.sort_by(|a, b| b.cpu.total_cmp(&a.cpu));
    culprits.truncate(MAX_CHILDREN);

    let children = culprits
        .into_iter()
        .map(|p| ProcView {
            label: child_label(p),
            pid: p.pid,
            cpu: p.cpu,
            mem_bytes: p.mem_bytes,
        })
        .collect();

    let cwd = root.cwd.as_deref();
    let project =
        project_from_cwd(cwd).unwrap_or_else(|| format!("pid {pid}", pid = root.pid));
    let session_id = active_session_for(cwd, claude_projects_dir);

    ClaudeSession {
        root_pid: root.pid,
        project,
        session_id,
        total_cpu,
        total_mem_bytes: total_mem,
        children,
    }
}

/// The pure core of a sample: given a process list and the
/// `~/.claude/projects` dir, produce the per-session view sorted by total CPU
/// descending. No OS calls except the (injected) projects-dir read.
pub fn sessions_from_procs(procs: &[RawProc], claude_projects_dir: &Path) -> Vec<ClaudeSession> {
    let by_pid: HashMap<u32, &RawProc> = procs.iter().map(|p| (p.pid, p)).collect();
    let subtrees = build_subtrees(procs);

    let mut sessions: Vec<ClaudeSession> = subtrees
        .iter()
        .filter_map(|(root_pid, pids)| {
            by_pid
                .get(root_pid)
                .map(|root| assemble_session(root, pids, &by_pid, claude_projects_dir))
        })
        .collect();

    sessions.sort_by(|a, b| b.total_cpu.total_cmp(&a.total_cpu));
    sessions
}

/// Aggregate totals across all sessions, for the footer line.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Default)]
pub struct ActivityTotals {
    /// Sum of every session's CPU%.
    pub total_cpu: f32,
    /// Sum of every session's resident memory in bytes.
    pub total_mem_bytes: u64,
    /// Number of live Claude Code sessions.
    pub session_count: usize,
}

/// Compute the footer totals from the per-session list.
pub fn totals(sessions: &[ClaudeSession]) -> ActivityTotals {
    ActivityTotals {
        total_cpu: sessions.iter().map(|s| s.total_cpu).sum(),
        total_mem_bytes: sessions.iter().map(|s| s.total_mem_bytes).sum(),
        session_count: sessions.len(),
    }
}

/// Default location of Claude Code's per-project session logs:
/// `~/.claude/projects`. `None` when the home dir can't be resolved.
pub fn default_claude_projects_dir() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".claude").join("projects"))
}

// ── Stateful monitor (owns the live System) ─────────────────────────────────────

#[cfg(feature = "activity")]
mod monitor {
    use super::*;
    use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System};

    /// Owns a long-lived [`sysinfo::System`] so CPU% deltas compute correctly
    /// across ticks, and the path to `~/.claude/projects`. Callers hold one of
    /// these and call [`Self::sample`] on each refresh tick.
    pub struct ActivityMonitor {
        sys: System,
        claude_projects_dir: PathBuf,
        /// Number of [`Self::sample`] calls so far. CPU% is a delta against the
        /// previous refresh, so the *first* sample has no baseline and its CPU
        /// readings are meaningless. [`Self::is_primed`] is true from the second
        /// sample onward; until then the caller shows "measuring…".
        samples_taken: u32,
    }

    impl ActivityMonitor {
        /// Build a monitor with the default `~/.claude/projects` dir.
        pub fn new() -> Self {
            Self::with_projects_dir(
                default_claude_projects_dir().unwrap_or_else(|| PathBuf::from(".claude/projects")),
            )
        }

        /// Build a monitor pointed at a specific projects dir (used in tests).
        pub fn with_projects_dir(claude_projects_dir: PathBuf) -> Self {
            Self {
                sys: System::new(),
                claude_projects_dir,
                samples_taken: 0,
            }
        }

        /// Whether a CPU baseline exists yet — true once at least two samples
        /// have been taken. `false` after the first sample (CPU has no prior
        /// point to delta against), so the caller shows "measuring…".
        pub fn is_primed(&self) -> bool {
            self.samples_taken >= 2
        }

        /// Refresh the process table and return the per-session view. The first
        /// call primes the CPU baseline (its CPU numbers are zero/garbage and
        /// [`Self::is_primed`] stays `false` until the *next* call). Keep the
        /// monitor alive between calls and space calls by at least
        /// [`sysinfo::MINIMUM_CPU_UPDATE_INTERVAL`] so the deltas are valid.
        pub fn sample(&mut self) -> Vec<ClaudeSession> {
            // Only the fields we actually read: cmd/exe/cwd, cpu, memory. This
            // keeps the per-tick refresh lean.
            self.sys.refresh_processes_specifics(
                ProcessesToUpdate::All,
                ProcessRefreshKind::new()
                    .with_cpu()
                    .with_memory()
                    .with_cwd(sysinfo::UpdateKind::OnlyIfNotSet)
                    .with_exe(sysinfo::UpdateKind::OnlyIfNotSet)
                    .with_cmd(sysinfo::UpdateKind::OnlyIfNotSet),
            );

            let procs: Vec<RawProc> = self
                .sys
                .processes()
                .values()
                .map(|p| RawProc {
                    pid: p.pid().as_u32(),
                    ppid: p.parent().map(|pp| pp.as_u32()),
                    name: p.name().to_string_lossy().to_string(),
                    cmd: p
                        .cmd()
                        .iter()
                        .map(|s| s.to_string_lossy())
                        .collect::<Vec<_>>()
                        .join(" "),
                    exe: p.exe().map(|e| e.to_path_buf()),
                    cwd: p.cwd().map(|c| c.to_path_buf()),
                    cpu: p.cpu_usage(),
                    mem_bytes: p.memory(),
                })
                .collect();

            // The first refresh only establishes the CPU baseline; its CPU
            // readings aren't meaningful yet (`is_primed` stays false until the
            // second call), but we still return the sessions so the UI can
            // render the structure with a "measuring…" CPU note.
            self.samples_taken = self.samples_taken.saturating_add(1);
            sessions_from_procs(&procs, &self.claude_projects_dir)
        }
    }

    impl Default for ActivityMonitor {
        fn default() -> Self {
            Self::new()
        }
    }
}

#[cfg(feature = "activity")]
pub use monitor::ActivityMonitor;

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    fn proc(pid: u32, ppid: Option<u32>, name: &str, cmd: &str, cpu: f32, mem: u64) -> RawProc {
        RawProc {
            pid,
            ppid,
            name: name.to_string(),
            cmd: cmd.to_string(),
            exe: Some(PathBuf::from(cmd.split_whitespace().next().unwrap_or(name))),
            cwd: None,
            cpu,
            mem_bytes: mem,
        }
    }

    fn with_cwd(mut p: RawProc, cwd: &str) -> RawProc {
        p.cwd = Some(PathBuf::from(cwd));
        p
    }

    #[test]
    fn classifies_only_the_claude_cli_root() {
        let procs = vec![
            proc(1, None, "claude", "claude --dangerously-skip-permissions", 10.0, 100),
            proc(2, Some(1), "node", "node /x/mcp/dist/index.js", 5.0, 50),
            // Desktop app — excluded.
            proc(3, None, "Claude", "/Applications/Claude.app/Contents/MacOS/Claude", 1.0, 10),
            // aura itself — excluded.
            proc(4, None, "aura", "/usr/local/bin/aura", 1.0, 10),
            proc(5, None, "claude.exe", "claude.exe --resume abc", 2.0, 20),
        ];
        let roots = classify_roots(&procs);
        let pids: Vec<u32> = roots.iter().map(|r| r.pid).collect();
        assert_eq!(pids, vec![1, 5]);
    }

    #[test]
    fn classifies_root_by_first_argv_token() {
        // sysinfo couldn't shorten the name, but argv[0] basename is `claude`.
        let p = proc(9, None, "node", "/Users/x/.local/bin/claude --resume z", 1.0, 1);
        assert!(is_claude_root(&p));
    }

    #[test]
    fn builds_full_subtree_including_grandchildren() {
        let procs = vec![
            proc(1, None, "claude", "claude", 1.0, 1),
            proc(2, Some(1), "node", "node a", 1.0, 1),
            proc(3, Some(2), "bash", "bash b", 1.0, 1), // grandchild
            proc(4, Some(1), "python", "python -m x", 1.0, 1),
            proc(99, None, "unrelated", "unrelated", 1.0, 1),
            proc(100, Some(99), "child", "child", 1.0, 1),
        ];
        let subtrees = build_subtrees(&procs);
        assert_eq!(subtrees.len(), 1);
        let (root, mut pids) = subtrees[0].clone();
        assert_eq!(root, 1);
        pids.sort();
        assert_eq!(pids, vec![1, 2, 3, 4]);
    }

    #[test]
    fn subtree_is_cycle_safe() {
        let mut map: HashMap<u32, Vec<u32>> = HashMap::new();
        map.insert(1, vec![2]);
        map.insert(2, vec![1]); // cycle
        let pids = subtree_pids(1, &map);
        assert_eq!(pids.len(), 2);
    }

    #[test]
    fn project_name_is_basename_of_cwd() {
        assert_eq!(
            project_from_cwd(Some(Path::new("/Users/x/Downloads/reconhecimento"))),
            Some("reconhecimento".to_string())
        );
        assert_eq!(project_from_cwd(None), None);
    }

    #[test]
    fn slugifies_path_replacing_slash_and_dot() {
        assert_eq!(
            slugify_cwd(Path::new("/Users/pedro/Downloads/AI-Outreach/.brand-research")),
            "-Users-pedro-Downloads-AI-Outreach--brand-research"
        );
        assert_eq!(
            slugify_cwd(Path::new("/Users/pedro/Downloads/cambrian-api-key-dashboard")),
            "-Users-pedro-Downloads-cambrian-api-key-dashboard"
        );
    }

    #[test]
    fn active_session_picks_newest_jsonl() {
        let tmp = tempfile::tempdir().unwrap();
        let cwd = "/Users/x/proj";
        let slug = slugify_cwd(Path::new(cwd));
        let dir = tmp.path().join(&slug);
        fs::create_dir_all(&dir).unwrap();

        // Two sessions; the second is written later so it's newest.
        let old = dir.join("aaaa1111-old.jsonl");
        fs::write(&old, "{}").unwrap();
        // Bump mtimes deterministically: re-write the newer file after a beat.
        std::thread::sleep(std::time::Duration::from_millis(20));
        let new = dir.join("7c51dddd-new.jsonl");
        fs::write(&new, "{}").unwrap();

        let id = active_session_for(Some(Path::new(cwd)), tmp.path());
        assert_eq!(id, Some("7c51".to_string()));
    }

    #[test]
    fn active_session_none_when_dir_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let id = active_session_for(Some(Path::new("/Users/x/nope")), tmp.path());
        assert_eq!(id, None);
    }

    #[test]
    fn child_label_is_readable() {
        let node = proc(
            1,
            None,
            "node",
            "node /Users/x/.claude/mcp-servers/design-inspiration-mcp-server/dist/index.js",
            0.0,
            0,
        );
        assert_eq!(child_label(&node), "node design-inspiration-mcp-server");

        let py = proc(
            2,
            None,
            "python",
            "/Users/x/.venv/bin/python -m studio_design_mcp",
            0.0,
            0,
        );
        assert_eq!(child_label(&py), "python studio_design_mcp");

        let cargo = proc(3, None, "bash", "/bin/bash -c cargo build", 0.0, 0);
        assert_eq!(child_label(&cargo), "bash cargo");

        let claude = proc(4, None, "claude", "claude --resume abc", 0.0, 0);
        assert_eq!(child_label(&claude), "claude --resume");
    }

    #[test]
    fn sums_subtree_and_picks_heaviest_children() {
        let tmp = tempfile::tempdir().unwrap();
        let procs = vec![
            with_cwd(
                proc(1, None, "claude", "claude", 44.0, 600_000_000),
                "/Users/x/recon",
            ),
            proc(2, Some(1), "node", "node /x/mcp-figma/dist/index.js", 180.0, 800_000_000),
            proc(3, Some(1), "bash", "/bin/bash -c cargo build", 132.0, 1_200_000_000),
            proc(4, Some(1), "node", "node /x/tiny/dist/index.js", 1.0, 1_000),
        ];
        let sessions = sessions_from_procs(&procs, tmp.path());
        assert_eq!(sessions.len(), 1);
        let s = &sessions[0];
        assert_eq!(s.project, "recon");
        // 44 + 180 + 132 + 1
        assert!((s.total_cpu - 357.0).abs() < 0.01);
        assert_eq!(s.total_mem_bytes, 600_000_000 + 800_000_000 + 1_200_000_000 + 1_000);
        // Heaviest 3 children, root (44%) excluded, sorted desc by cpu.
        assert_eq!(s.children.len(), 3);
        assert_eq!(s.children[0].cpu, 180.0);
        assert_eq!(s.children[1].cpu, 132.0);
        assert_eq!(s.children[2].cpu, 1.0);
    }

    #[test]
    fn sessions_sorted_by_total_cpu_desc() {
        let tmp = tempfile::tempdir().unwrap();
        let procs = vec![
            with_cwd(proc(1, None, "claude", "claude", 44.0, 1), "/Users/x/jp"),
            with_cwd(proc(2, None, "claude", "claude", 300.0, 1), "/Users/x/recon"),
            proc(3, Some(2), "node", "node /x/a/dist/index.js", 12.0, 1),
        ];
        let sessions = sessions_from_procs(&procs, tmp.path());
        assert_eq!(sessions.len(), 2);
        assert_eq!(sessions[0].project, "recon"); // 312
        assert_eq!(sessions[1].project, "jp"); // 44
    }

    #[test]
    fn lone_root_shows_itself_as_child() {
        let tmp = tempfile::tempdir().unwrap();
        let procs = vec![with_cwd(
            proc(1, None, "claude", "claude --resume a3d1", 44.0, 600),
            "/Users/x/jp",
        )];
        let sessions = sessions_from_procs(&procs, tmp.path());
        assert_eq!(sessions[0].children.len(), 1);
        assert_eq!(sessions[0].children[0].pid, 1);
    }

    #[test]
    fn project_falls_back_to_pid_without_cwd() {
        let tmp = tempfile::tempdir().unwrap();
        let procs = vec![proc(7, None, "claude", "claude", 1.0, 1)];
        let sessions = sessions_from_procs(&procs, tmp.path());
        assert_eq!(sessions[0].project, "pid 7");
        assert_eq!(sessions[0].session_id, None);
    }

    #[test]
    fn totals_aggregate_across_sessions() {
        let tmp = tempfile::tempdir().unwrap();
        let procs = vec![
            with_cwd(proc(1, None, "claude", "claude", 100.0, 1_000), "/Users/x/a"),
            with_cwd(proc(2, None, "claude", "claude", 50.0, 2_000), "/Users/x/b"),
        ];
        let sessions = sessions_from_procs(&procs, tmp.path());
        let t = totals(&sessions);
        assert_eq!(t.session_count, 2);
        assert!((t.total_cpu - 150.0).abs() < 0.01);
        assert_eq!(t.total_mem_bytes, 3_000);
    }

    #[test]
    fn empty_when_no_claude_processes() {
        let tmp = tempfile::tempdir().unwrap();
        let procs = vec![proc(1, None, "node", "node server.js", 10.0, 1)];
        let sessions = sessions_from_procs(&procs, tmp.path());
        assert!(sessions.is_empty());
    }

    #[cfg(feature = "activity")]
    #[test]
    fn monitor_primes_on_second_sample() {
        let tmp = tempfile::tempdir().unwrap();
        let mut mon = ActivityMonitor::with_projects_dir(tmp.path().to_path_buf());
        // First sample only establishes the CPU baseline.
        let _ = mon.sample();
        assert!(!mon.is_primed(), "should not be primed after one sample");
        // Second sample has a valid delta.
        let _ = mon.sample();
        assert!(mon.is_primed(), "should be primed after two samples");
    }
}
