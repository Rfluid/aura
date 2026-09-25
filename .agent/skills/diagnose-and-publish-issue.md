---
title: Diagnose an Aura issue and publish it to GitHub when warranted
status: current
version: 0.2.0
last_updated: 2026-09-16
last_verified: 2026-09-16
source_refs:
  - README.md
  - CONTRIBUTING.md
  - install.sh
  - uninstall.sh
  - scripts/install.ps1
  - scripts/uninstall.ps1
  - justfile
  - docs/cli.md
  - docs/configuration.md
  - docs/platform-tray-icon.md
  - docs/troubleshooting/modal-stretches-on-resize-kde.md
  - docs/plugin-system.md
  - docs/plugin-authoring.md
  - .agent/context/stack.md
  - .github/ISSUE_TEMPLATE/bug_report.yml
  - .github/ISSUE_TEMPLATE/config.yml
owner: "@rfluid"
tags: [skill, support, triage, github, diagnostics]
---

# Diagnose an Aura issue and publish it to GitHub when warranted

Use this skill when a user reports Aura misbehavior, asks whether it is a real
bug, or wants the agent to diagnose and create a GitHub issue for `Rfluid/aura`.
The skill's first job is triage: decide whether the report is an actionable
Aura issue, documented expected behavior, a local setup/config problem, a known
platform limitation, a duplicate, or a documentation gap. File or update a
GitHub issue only after that decision is evidence-backed.

The GitHub repo is `Rfluid/aura`. When using `gh`, pass `--repo Rfluid/aura` so
commands work from any clone.

## Invocation Boundaries

Use this skill for:

- Installation, update, uninstall, autostart, tray icon, modal/window behavior,
  configuration, theme, usage/quota data, plugin loading, or agent-source
  reports.
- Requests like "diagnose this", "is this a bug?", "create an issue", "report
  this upstream", or "turn this into a GitHub issue".
- Support reports where OS, desktop environment, or install method materially
  affects the answer.

Do not use this skill for:

- General Rust development unrelated to Aura support.
- A code change the agent can simply implement in the working tree instead of
  filing as support.
- Security-sensitive reports. Direct the user to the private advisory link in
  `.github/ISSUE_TEMPLATE/config.yml`; do not publish details publicly.

## Core Rule

Answer the user's actual situation before filing anything. A good result may be
"this is expected on native Wayland; use XWayland or a KWin rule", "this is a
duplicate; add your logs to #123", "this is a docs gap; open a docs issue", or
"this is an actionable bug; I filed it here".

Never create a public issue until:

- Documentation and repo behavior have been checked.
- Existing open and closed issues have been searched.
- The report has enough environment, reproduction, expected/actual behavior,
  and logs to be actionable.
- The user explicitly approves publishing with the final title/body summary.

## Source Map

Read only the sources relevant to the reported area, plus the issue template
before composing a public issue.

| Report area | Sources to check |
| --- | --- |
| Install/update/uninstall | `README.md` installation/update sections, `install.sh`, `uninstall.sh`, `scripts/install.ps1`, `scripts/uninstall.ps1`, `justfile` |
| Tray icon/click behavior | `README.md` "Making the tray icon always visible" and "Compatibility", `docs/platform-tray-icon.md`, `crates/aura-ui/src/tray.rs`, `crates/aura-ui/src/tray_status.rs`, `crates/aura-ui/src/lib.rs` |
| Modal placement/resize/focus | `README.md` "Modal placement on Wayland", `docs/configuration.md` modal/backend sections, `docs/platform-tray-icon.md`, `docs/troubleshooting/modal-stretches-on-resize-kde.md`, `crates/aura-ui/src/app.rs`, `crates/aura-ui/src/placement.rs`, `crates/aura-ui/src/platform.rs` |
| Config/theme/state | `docs/configuration.md`, `docs/cli.md`, `crates/aura-core/src/config*.rs`, `crates/aura-core/src/state.rs`, `.agent/skills/add-or-change-config.md` |
| Usage/quota/agent data | `README.md`, `docs/cli.md`, `.agent/context/stack.md`, relevant `crates/aura-core` and `crates/aura-ui/src/cli` sources |
| Plugins | `README.md` plugin section, `docs/plugin-system.md`, `docs/plugin-authoring.md`, `plugins/*/README.md`, relevant plugin/core sources |
| Issue publishing | `.github/ISSUE_TEMPLATE/bug_report.yml`, `.github/ISSUE_TEMPLATE/config.yml`, `CONTRIBUTING.md` |

Use `aura doctor` early when Aura is runnable. It is local-only and designed as
the first diagnostic surface.

## Triage Workflow

1. Restate the symptom in one sentence and identify the likely area and
   platform. If critical facts are missing, ask only for the minimum needed:
   OS/version, desktop/session on Linux, Aura version, install method,
   reproduction steps, expected/actual behavior, and logs or command output.

2. Check the relevant repo docs and sources from the source map. Prefer current
   repo documentation over memory. If docs describe the behavior as expected or
   unsupported, do not file a bug unless the user's evidence shows the docs are
   wrong, incomplete, or the behavior regressed.

3. Gather platform-specific evidence. Avoid destructive recovery commands unless
   the user explicitly approves them.

4. Classify the result using the decision categories below. Give the user the
   classification and the best next action before asking to publish.

5. Search GitHub issues and labels with `gh` if issue creation or duplicate
   detection is in scope:

   ```sh
   gh issue list --repo Rfluid/aura --state all --search "<specific keywords>" --limit 20
   gh label list --repo Rfluid/aura
   ```

6. If an issue is warranted, compose it using the GitHub template headings,
   preview the title/body/labels for the user, and ask for explicit approval to
   publish. After approval, create or update the issue.

## Decision Categories

Use these categories in the final diagnosis. Pick the strongest one supported
by evidence.

- **Actionable bug:** Aura behavior contradicts documented behavior, expected
  platform behavior, or a supported workflow; the report includes enough
  reproduction detail and diagnostics for maintainers to act.
- **Regression:** A previously working supported behavior broke after a known
  Aura, OS, desktop, dependency, or driver update. Prefer adding last-known-good
  and first-known-bad versions.
- **Known platform limitation:** The behavior follows documented platform
  constraints, such as native Wayland refusing client-side window placement or
  GNOME requiring AppIndicator/SNI support for tray icons. Provide the documented
  workaround instead of filing a bug.
- **Configuration/setup issue:** The problem is caused by missing config,
  invalid config, missing agent credentials, absent plugin binary, disabled
  autostart, missing system dependency, or an unsupported install path. Provide
  commands to fix and verify.
- **Duplicate:** A matching open issue exists, or a closed issue appears to have
  regressed. Comment or reopen only with user approval and new evidence.
- **Documentation gap:** Behavior is expected or supportable but the docs fail
  to help a reasonable user reach the answer. File a docs issue only if the user
  wants it, and label/title it as documentation rather than bug.
- **Insufficient evidence:** The report lacks the minimum diagnostics. Ask for
  the exact missing artifact instead of filing.

## Platform Evidence

Run commands relevant to the user's OS. Redact secrets and personal paths when
needed. Do not paste OAuth tokens, `~/.claude/.credentials.json`, Keychain
items, Credential Manager secrets, or private project names into public issues.

### All Platforms

- `aura --version` or the platform-specific binary path.
- `aura doctor --format text` when the binary runs.
- `aura config path`, `aura config validate`, and `aura config show` when config
  or profile selection may be involved.
- `aura agents list --format text` for agent detection problems.
- `aura plugin list --format text` and `aura plugin run <name>` for plugin
  problems.

Config/state locations:

| Platform | Config/theme path | State path |
| --- | --- | --- |
| Linux | `~/.config/aura/` | `~/.local/share/aura/` |
| macOS | `~/Library/Application Support/aura/` | `~/Library/Application Support/aura/` |
| Windows | `%APPDATA%\aura\` | `%APPDATA%\aura\` |

### Linux

- `echo "$XDG_CURRENT_DESKTOP / $XDG_SESSION_TYPE"`
- `systemctl --user status aura`
- `journalctl --user -u aura -n 200 --no-pager`
- `pgrep -a aura`
- `systemctl --user stop aura && ~/.local/bin/aura` to capture stderr live.
- For tray issues, check SNI/AppIndicator support:
  - KDE/Plasma: `qdbus6 org.kde.StatusNotifierWatcher` or the equivalent
    `gdbus` call.
  - GNOME: `gnome-extensions list --enabled | grep -i appindicator`
- For Wayland placement: confirm whether `window.linux_backend` is `auto`,
  `x11`, or `wayland`; check whether `$DISPLAY` exists; check KWin rules when
  KDE is involved.
- For KDE resize stretching: check Morphing Popups as documented in
  `docs/troubleshooting/modal-stretches-on-resize-kde.md`.

### macOS

- `/Applications/Aura.app/Contents/MacOS/aura --version`
- `launchctl print "gui/$(id -u)/com.aura.agent-usage" | head -50`
- `log show --predicate 'process == "aura"' --info --last 30m`
- `xattr /Applications/Aura.app` for quarantine.
- `/Applications/Aura.app/Contents/MacOS/aura` to capture stderr live.
- If Keychain or Claude Code credentials are mentioned, verify Claude Code has
  been launched at least once; do not expose Keychain contents.

### Windows

Use PowerShell unless the user provides another shell.

- `& "$env:LOCALAPPDATA\Programs\Aura\aura.exe" --version`
- `Get-Process aura -ErrorAction SilentlyContinue | Format-Table Id,ProcessName,StartTime`
- `Test-Path "$([Environment]::GetFolderPath('Startup'))\Aura.lnk"`
- `& "$env:LOCALAPPDATA\Programs\Aura\aura.exe"` to capture stderr live.
- `cmdkey /list | Select-String "Claude Code-credentials"` only to confirm
  presence, not to print secret values.

## Common Non-Issue Answers

Use these as documented decision points, not as shortcuts. Verify the user's
facts first.

- **GNOME tray icon missing:** GNOME needs the AppIndicator/KStatusNotifierItem
  extension. If absent, enabling it is the fix; Aura should keep retrying and
  register after the SNI host appears.
- **Native Wayland modal placement:** Wayland does not allow a normal toplevel
  client to choose its own screen position. Aura defaults to XWayland via
  `window.linux_backend = "auto"` when `$DISPLAY` is present. If the user chose
  `wayland`, placement/anchoring limits are expected; suggest `auto`/`x11` or a
  compositor window rule.
- **KDE modal stretch during resize:** KWin's Morphing Popups effect causes
  this. It is cosmetic and documented; provide the temporary unload command and
  the persistent System Settings/`kwriteconfig` fix.
- **macOS first launch blocked/quarantined:** Release bundles are unsigned and
  may be quarantined. The README documents removing quarantine with `xattr`.
- **Windows Credential Manager warning:** Claude Code credentials may not exist
  until Claude Code has been run once. Confirm presence without printing values.
- **Plugins missing after install:** Core Aura installs no plugins by default.
  Install with `aura plugin add` or place binaries in the user plugin dir.
- **Config edits not taking effect immediately everywhere:** Most config is
  reloaded on tray open and modal refresh; some startup/autostart changes need
  a process restart. Use `aura config validate` before treating this as a bug.

## Remedies

When the classification is not "Actionable bug", provide a concrete remedy.
Separate temporary and permanent remedies when both exist.

- Temporary remedies should be reversible and scoped to diagnosis, such as
  running Aura directly in a terminal, unloading KWin Morphing Popups for the
  session, switching `window.linux_backend` to `auto`, or enabling an extension.
- Permanent remedies should use documented CLI/config/install paths, such as
  `aura config set`, `aura config setup`, `aura plugin add`, reinstall/update
  scripts, persistent KWin settings, or startup/autostart repair.
- Include verification commands, ideally `aura doctor`, `aura config validate`,
  `aura agents list`, or a platform service/status command.
- If the docs are correct but hard to find, suggest the exact doc section to
  link rather than filing a code bug.

## Local Reproduction

If a clone and Rust toolchain are available and local reproduction would add
signal:

```sh
cargo --version
rustc --version
cargo build --release --workspace
cargo run -p aura
```

Run `just lint` and `just test` only when the symptom plausibly maps to a code
regression that those checks can catch. If the user's OS differs from the
agent's OS, state the mismatch in the diagnosis and, if filing, in the issue
body.

## Issue Composition

Mirror `.github/ISSUE_TEMPLATE/bug_report.yml` when filing via `gh issue
create`, because the CLI does not open the web issue form. Fill every heading;
write `n/a` for fields that truly do not apply. Add an "Agent diagnosis" section
above the checklist to record classification, docs checked, duplicate search,
and repro attempt.

Title format:

```text
<area>(<platform>): <one-line symptom>
```

Examples:

- `tray(linux/gnome): icon never appears despite enabled AppIndicator extension`
- `installer(macos): install.sh leaves quarantined app unable to launch`
- `modal(windows): bottom-right placement ignores taskbar work area`
- `docs(linux/kde): document Morphing Popups resize stretch workaround`

Body template:

````markdown
## Summary
<1-2 sentence symptom summary.>

## Platform
<Linux | macOS | Windows>

## OS / kernel version
<exact version>

## Desktop environment & session type (Linux only)
<desktop / session, or n/a>

## Aura version
`<aura --version output>`

## Install method
<release install.sh | release install.ps1 | source ./install.sh | just install | manual | package manager | other>

## Area
<Installer / Autostart / Tray icon / Modal window / Modal content / Agent data source - Claude Code / Agent data source - Codex / Agent data source - Gemini / Plugin system / Configuration / Other>

## Steps to reproduce
1. ...
2. ...
3. ...

## Expected behavior
<what should happen>

## Actual behavior
<what happens instead>

## Logs / command output
```text
<redacted logs>
```

## Was it ever working?
<last known good or n/a>

## Workarounds tried
- <list or none>

## Possible cause / hypothesis
<only evidence-backed code path, stack trace, bisect, or n/a>

## Agent diagnosis
- Classification: <Actionable bug | Regression | Documentation gap>
- Docs checked: <files/sections>
- Duplicate search: <queries and result>
- Repro attempt: <local result or not attempted with reason>

## Pre-submission checklist
- [x] Searched open + closed issues; not a duplicate.
- [x] No secrets in the logs/output above.
- [x] This is a bug report, not a usage question.
````

Labels should come from existing labels only. Good candidates usually include
`bug`, `regression`, `compatibility`, `documentation`, and platform/area labels
such as `linux`, `macos`, `windows`, `tray`, `modal`, `installer`, `autostart`,
`plugin`, or `agent-source`. If a useful label is missing, note it in the body;
do not create labels.

## Publishing

Before creating or commenting on an issue, show the user:

- Classification and why it is publish-worthy.
- Existing issue search result.
- Proposed title.
- Proposed labels.
- Proposed issue body or concise preview.

Ask one clear approval question. After approval:

```sh
gh issue create --repo Rfluid/aura \
  --title "<title>" \
  --label "<comma-separated existing labels>" \
  --body-file <temporary-body-file>
```

If a duplicate exists:

- Open issue: ask approval to comment with the new environment/repro instead of
  filing.
- Closed issue with a credible regression: ask approval to reopen and comment.
- Closed issue with expected behavior: link it and provide the documented remedy.

Return the issue URL or comment URL printed by `gh`.

## Safety

- Do not run destructive commands such as uninstalling, deleting config/state,
  clearing Keychain/Credential Manager entries, or removing plugins without
  explicit user approval.
- Do not include secrets or private local paths in public issues.
- Do not speculate about fixes in the public issue. Keep hypotheses tied to
  stack traces, code paths, docs, or reproduction evidence.
- Do not file an issue just because the user asked if the evidence points to
  documented expected behavior; explain the remedy and offer a docs issue only
  when the docs are insufficient.
