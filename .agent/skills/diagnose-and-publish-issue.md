---
title: Diagnose a platform issue and publish it to GitHub
status: current
version: 0.1.0
last_updated: 2026-05-23
last_verified: 2026-05-23
source_refs:
  - README.md
  - install.sh
  - scripts/install.ps1
  - justfile
  - .agent/context/stack.md
  - .github/ISSUE_TEMPLATE/bug_report.yml
  - .github/ISSUE_TEMPLATE/config.yml
owner: "@rfluid"
tags: [skill, support, triage, github]
---

# Diagnose a platform issue and publish it to GitHub

Use this skill when a user reports an Aura bug or misbehavior on a
specific OS / desktop environment and asks the agent to investigate
and file an issue against `Rfluid/aura`. The skill covers: clarifying
the report, gathering the right artifacts per platform, optionally
reproducing locally with `cargo`, and creating a deduplicated GitHub
issue via `gh`.

The remote is **`git@github.com:Rfluid/aura.git`** — when calling `gh`
use `--repo Rfluid/aura` so it works from any clone.

## When to invoke

- The user is on macOS, Linux, or Windows and reports a problem with
  installation, autostart, the tray icon, the modal, or an agent
  data-source integration.
- The user explicitly asks for "diagnosis + open an issue", "report
  this upstream", or similar.
- Do **not** invoke for: questions about how to use Aura (point at the
  README), generic Rust help, or requests that already have a clear
  fix the agent can land in a PR.

## Step 1 — Clarify the report (ask these before touching the system)

Ask the questions below in one batch. Skip ones the user already
answered. Do not start running commands until the answers are in or
the user explicitly says "just diagnose".

### Codebase access

1. **Is the Aura source checked out locally?** If so, what path? If
   not, do you want me to `git clone git@github.com:Rfluid/aura.git`
   into a working directory so I can build and reproduce? (Without a
   checkout, diagnosis is limited to inspecting the installed
   artifacts, logs, and configs.)
2. **If a clone exists, is `cargo` (Rust ≥ 1.80) available?** If yes,
   we can `cargo build --release --workspace` and `cargo run -p aura`
   from a terminal to capture stderr that the autostart service
   swallows. If no, ask whether `rustup` may be installed.

### Environment

3. **Operating system + version.** Examples: macOS 14.5 (Tahoe),
   Ubuntu 24.04, Fedora 40, Windows 11 23H2.
4. **Linux only — desktop environment + session type.** Plasma 6 on
   Wayland, GNOME 45 on Wayland, sway, etc. (`echo
   $XDG_CURRENT_DESKTOP`, `echo $XDG_SESSION_TYPE`).
5. **Aura version.** `aura --version` (or `~/.local/bin/aura
   --version`, `/Applications/Aura.app/Contents/MacOS/aura --version`,
   `%LOCALAPPDATA%\Programs\Aura\aura.exe --version`).
6. **How was Aura installed?** `install.sh` from GitHub Releases,
   `./install.sh` from source, `just install`, `cargo build` +
   manual, `install.ps1`, or other.

### Symptom

7. **One-line summary of the bug.** ("Tray icon never appears on
   GNOME 45 / Wayland", "Modal opens off-screen on macOS 14",
   "`install.ps1` aborts with `unsigned binary` warning", etc.)
8. **Exact reproduction steps.** From a clean state if possible.
9. **Expected vs. observed.** What did you think would happen, and
   what actually happened?
10. **Was it ever working?** If yes, what changed (Aura update, OS
    update, DE update, GPU driver change)? `git log` against the
    affected file is often informative.
11. **Already-tried workarounds.** Reinstall, restart, `just
    uninstall && just install`, switching X11 ↔ Wayland, etc.

### Permission to file

12. **Are you authorized to file this issue against
    `Rfluid/aura`?** The agent will run `gh issue create --repo
    Rfluid/aura` — confirm before publishing.

## Step 2 — Verify environment & gather artifacts

Run only the commands relevant to the user's platform. Capture output
into a scratch buffer for the eventual issue body; do **not** paste
home-directory paths containing the user's username verbatim if the
user prefers them redacted (ask).

### All platforms

- `aura --version` — confirms the version on disk.
- `cat <config_path>/config.toml` (path per platform table below) —
  capture *structure*, redact agent names/paths if requested.
- `ls -la <state_path>` — to confirm whether `state.json` exists.

| Platform | Config path                                      | State path                  |
| -------- | ------------------------------------------------ | --------------------------- |
| Linux    | `~/.config/aura/config.toml`                     | `~/.local/share/aura/`      |
| macOS    | `~/Library/Application Support/aura/config.toml` | `~/Library/Application Support/aura/` |
| Windows  | `%APPDATA%\aura\config.toml`                     | `%APPDATA%\aura\`           |

### Linux

- `systemctl --user status aura` — autostart service health.
- `journalctl --user -u aura -n 200 --no-pager` — service logs.
- `pgrep -a aura` — is it running? what cmdline?
- For tray icon issues: confirm StatusNotifierItem support.
  - Plasma: `qdbus6 org.kde.StatusNotifierWatcher 2>/dev/null` or
    `gdbus call --session --dest org.kde.StatusNotifierWatcher
    --object-path /StatusNotifierWatcher --method
    org.freedesktop.DBus.Properties.GetAll
    org.kde.StatusNotifierWatcher`.
  - GNOME: check the **AppIndicator** extension is installed and
    enabled (`gnome-extensions list --enabled | grep -i appindicator`).
- For Wayland modal placement issues: confirm KWin rules file
  (`~/.config/kwinrulesrc`) — `install.sh` writes a keepalive rule;
  capture it.
- For systemd autostart not firing: `loginctl show-user "$USER" |
  grep Linger`.
- Stop the service and run `aura` directly from a terminal so stderr
  is captured: `systemctl --user stop aura && ~/.local/bin/aura`.

### macOS

- `launchctl print "gui/$(id -u)/com.aura.agent-usage" | head -50`
  — LaunchAgent status; non-zero `last exit code` is the smoking gun.
- `log show --predicate 'process == "aura"' --info --last 30m` —
  unified log entries.
- `xattr /Applications/Aura.app` — quarantine flag still attached?
- `/Applications/Aura.app/Contents/MacOS/aura` — run directly to see
  stderr.
- Keychain hint: if the user mentions "Keychain read failed", confirm
  Claude Code has been launched at least once to populate the
  `Claude Code-credentials` keychain item.

### Windows

- PowerShell: `Get-Process aura -ErrorAction SilentlyContinue | Format-Table Id,
  ProcessName, StartTime`.
- `Get-EventLog -LogName Application -Source 'aura' -Newest 50` (may
  be empty — Aura logs to stderr, not Event Log).
- Startup folder shortcut: `Test-Path "$([Environment]::GetFolderPath('Startup'))\Aura.lnk"`.
- Run interactively: `& "$env:LOCALAPPDATA\Programs\Aura\aura.exe"` —
  captures stderr in the current shell.
- Credential Manager: `cmdkey /list | Select-String "Claude Code-credentials"`.

## Step 3 — Reproduce locally (if a clone + cargo are available)

1. `cd` into the clone.
2. `cargo --version && rustc --version` — confirm ≥ 1.80.
3. `cargo build --release --workspace` — first reproducibility check.
   A build failure that matches the user's report is already an
   answer.
4. `cargo run -p aura 2>&1 | tee /tmp/aura-repro.log` — run from a
   terminal, drive the bug, capture stderr. This is the single
   highest-signal artifact for any tray / modal / panic issue.
5. `just lint` and `just test` — only if the symptom plausibly maps to
   a regression a unit/integration test would catch.
6. If the user is on a different OS than the agent, say so explicitly
   in the issue: "Repro attempted on `<your OS>`; the user is on
   `<their OS>`."

## Step 4 — Classify and dedupe

Before filing, check whether the issue is already known:

```
gh issue list --repo Rfluid/aura --state all --search "<keywords>" --limit 20
gh issue list --repo Rfluid/aura --state all --label "<platform>" --limit 20
```

Keywords should be the most specific terms from the symptom (e.g.,
`KWin keepalive`, `appindicator`, `quarantine Gatekeeper`,
`Credential Manager`). If a matching issue exists:

- If still open: comment on it with the new repro/environment via `gh
  issue comment <number> --repo Rfluid/aura --body "…"` instead of
  filing a new one.
- If closed but the bug appears to have regressed: reopen with `gh
  issue reopen` and add a comment — do not file a duplicate.

Pick labels from the existing label set (`gh label list --repo
Rfluid/aura`). Likely candidates:

- Platform: `linux`, `macos`, `windows`.
- Area: `tray`, `modal`, `installer`, `autostart`, `plugin`,
  `agent-source` (Claude Code / Codex / Gemini integrations).
- Type: `bug`, `regression`, `compatibility`.

If a needed label doesn't exist, **don't create it** — leave a note
in the issue body and let the maintainer label.

## Step 5 — Compose the issue

The canonical structure is the issue form at
`.github/ISSUE_TEMPLATE/bug_report.yml`. When filing via `gh issue
create` we cannot trigger the form, so we mirror the form's headings
in plain markdown — same field labels, same order — so the rendered
issue looks identical to one a human would file from the web UI.

Fill every section. If something genuinely does not apply write
`n/a` rather than deleting the heading.

```markdown
## Summary
<1–2 sentence description, leading with the symptom.>

## Platform
<Linux | macOS | Windows>

## OS / kernel version
<e.g. Fedora 40, kernel 6.9.4-200.fc40.x86_64>

## Desktop environment & session type (Linux only)
<Output of `echo "$XDG_CURRENT_DESKTOP / $XDG_SESSION_TYPE"`, e.g. `KDE / wayland`. n/a on macOS / Windows.>

## Aura version
`<aura --version output>`

## Install method
<one of: GitHub Releases via install.sh | GitHub Releases via install.ps1 | source via ./install.sh | source via just install | source manually | distro/Homebrew/scoop | other>

## Area
<one of: Installer | Autostart | Tray icon | Modal window | Modal content | Agent data source — Claude Code | Agent data source — Codex | Agent data source — Gemini | Plugin system / RTK Gains | Configuration | Other>

## Steps to reproduce
1. …
2. …
3. …

## Expected behavior
<what should happen>

## Actual behavior
<what actually happens>

## Logs / command output
```text
<paste journalctl / log show / stderr capture here>
```

## Was it ever working?
<Last known good Aura version + what changed; or "no, broken since first install".>

## Workarounds tried
- <list, or "none">

## Possible cause / hypothesis
<Optional — include only with evidence (stack trace, bisect result,
specific code path like `crates/aura/src/app.rs:274`). Leave blank or
write `n/a` if you don't have a grounded guess.>

## Repro attempt (agent)
<"Reproduced on <agent OS>" | "Could not reproduce on <agent OS> — symptom is platform-specific" | "Not attempted (no local clone or no cargo)">

## Pre-submission checklist
- [x] Searched open + closed issues; not a duplicate.
- [x] No secrets in the logs/output above.
- [x] This is a bug report, not a usage question.
```

Notes:

- The `Pre-submission checklist` section is required by the form; in
  the markdown variant we tick the boxes ourselves once we've actually
  done those checks. Don't tick them blind.
- The `Repro attempt (agent)` section is the only one not present in
  the form — it's specifically for agent-filed issues so reviewers
  know who ran what.

Title format: `<area>(<platform>): <one-line symptom>` — same as the
form's `title:` hint, and matches the repo's commit-style convention
(`feat(scope): …`). Examples:

- `tray(linux/gnome): icon never appears without AppIndicator extension`
- `installer(macos): install.sh fails on Tahoe due to gpui dependency`
- `modal(windows): bottom-right placement off by taskbar height`

## Step 6 — Publish

Confirm one more time with the user, then file:

```
gh issue create --repo Rfluid/aura \
  --title "<title from step 5>" \
  --label "<comma-separated existing labels>" \
  --body "$(cat <<'EOF'
<body from step 5>
EOF
)"
```

After creation, return the issue URL printed by `gh` to the user. Do
not close the loop on your side — leave triage / assignment to the
maintainer.

## Things to avoid

- Don't run destructive recovery commands (`just uninstall`, `rm -rf
  ~/.config/aura`) without explicit user approval — the goal is to
  observe state, not reset it.
- Don't include OAuth tokens or anything from `~/.claude/.credentials.json`
  / macOS Keychain / Windows Credential Manager in the issue body.
  Redact agent-profile names if the user prefers.
- Don't speculate about fixes in the issue body. The hypothesis
  section is for code-path evidence, not for guesses.
- Don't file an issue if you couldn't get enough information to make
  it actionable — ask the user for the missing piece first.
- Don't create new labels; let the maintainer attach unknown
  categories.
