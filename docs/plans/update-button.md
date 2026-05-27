---
title: Update button
status: design
version: 0.1.0
last_updated: 2026-05-27
last_verified: 2026-05-27
source_refs:
    - crates/aura/src/app.rs
    - crates/aura/src/platform.rs
    - crates/aura/src/runtime.rs
    - crates/aura-core/src/config.rs
    - crates/aura-core/Cargo.toml
    - crates/aura/Cargo.toml
    - install.sh
    - justfile
    - scripts/install.ps1
    - README.md
owner: "@rfluid"
tags: [updates, ui, releases, header, design]
---

# Update button

## Problem

Aura ships from GitHub releases (Linux/macOS tarballs, Windows zip) and
the installer scripts at `install.sh` / `scripts/install.ps1`. There is
no in-app signal that a newer version exists: users either re-run the
installer on a hunch or notice the version drift accidentally on the
GitHub releases page. The **More** modal already has a "Check updates"
row, but it just opens `…/aura/releases` in a browser — it doesn't tell
the user whether an update is _actually_ available and it doesn't tell
them which version they're on right now.

Reference: Zed's `crates/title_bar/src/update_version.rs` +
`crates/ui/src/components/collab/update_button.rs` render a small
title-bar button when an update is available and provide a dismiss
"x". We want the same _UX_ — a visible nudge — without the auto-
downloader. Aura's update path stays "open the install script in a
shell"; the button just _surfaces the offer_.

## Scope

1. **Background release check on app open** — fetch the latest GitHub
   release tag, compare against `env!("CARGO_PKG_VERSION")`, surface
   the result. Must be non-blocking: if GitHub is slow / offline /
   rate-limiting us, the modal still opens at normal speed and nothing
   else stalls.
2. **Header update button** — visible only when a newer release exists,
   not dismissed, and the user hasn't disabled all update prompts. Two
   parts: a clickable label ("Update available · v0.1.18 →") and an "x"
   dismiss affordance.
3. **More modal "Check updates" gets a version label** — the row already
   exists; append the current Aura version (e.g. "Check updates ·
   v0.1.17") so a user inspecting the modal can confirm the build they
   are on.
4. **Dismissal model** — store the last dismissed version in
   `config.toml`. Re-show the button when a newer version is released
   (i.e. dismissing v0.1.18 does not suppress v0.1.19). Add a
   _"never show update prompts"_ toggle for users who don't want
   nudges at all.
5. **Click-through download flow** — clicking the update label opens
   the browser at the README's updating section. The README documents
   a **two-curl flow** ("uninstall script" piped to bash, then "install
   script" piped to bash) that works on Linux/macOS without `cargo` /
   `just` / any other dev tooling. Windows gets the equivalent two-iex
   block. This means extracting the existing `justfile` `uninstall:` /
   `uninstall-windows:` logic into standalone scripts (`uninstall.sh`,
   `scripts/uninstall.ps1`) that the README can point at.

## Non-goals

- **Auto-download / auto-install.** Aura is a tray indicator; the
  binary it ships is replaced from outside the running process. The
  button only opens the README; the user runs the curl pipes. Zed's
  in-app downloader is explicitly out of scope.
- **Pre-release / nightly channels.** Aura has one channel (the latest
  stable GitHub release). No `release_channel` enum needed.
- **Notifying users who already updated.** Zed posts a "Updated to
  X.Y.Z" toast on first launch after an update; we skip that. The
  README is enough.
- **Telemetry on click / dismiss.** Aura has none today and this
  feature does not introduce any.
- **Update check on a recurring timer while Aura runs.** Aura's tray
  app stays alive across many modal opens, but a check on **app
  startup** is sufficient — users have to re-open the modal to see
  the button anyway, and re-checking every modal open is wasteful.
  Future work: refetch when the modal opens if the last check is older
  than 24h. Tracked under [Future work](#future-work).

## Design

### Pipeline overview

```text
Aura process starts
        │
        ├─ AuraView::new() builds config + state from disk
        │
        ├─ Spawn background fetch (smol task on the background executor):
        │     GET https://api.github.com/repos/Rfluid/aura/releases/latest
        │     Accept: application/vnd.github+json
        │     User-Agent: aura/<version>            ← GH requires UA
        │     timeout = 5s
        │
        ├─ On success: parse `tag_name`, strip leading 'v', compare
        │     against env!("CARGO_PKG_VERSION") via semver::Version.
        │     If latest > current → write the latest tag into a shared
        │     atomic / `Arc<Mutex<Option<UpdateState>>>` consumed by
        │     the view.
        │
        ├─ On failure (timeout, network, 4xx/5xx, malformed JSON):
        │     log to stderr and keep `UpdateState = None`. Never bubble
        │     to the UI — a failed check is invisible by design.
        │
        └─ View next renders. Header button visibility derived from:
              UpdateState.is_some()
                && state.latest_tag != config.update.dismissed_version
                && !config.update.dismiss_all
```

Everything off the foreground executor uses the existing
`cx.background_executor().spawn(...)` pattern (see `app.rs` line 222 in
`do_refresh`). The fetch task takes a weak handle to the view and
updates a field via `this.update(cx, |view, cx| { … cx.notify(); })`
the same way `do_refresh` does.

### Why a background _executor_ task, not a `std::thread::spawn`

`platform::open_url` uses `std::thread::Builder::spawn` because it is
fire-and-forget — there is no UI state to push back. The update check
**does** push state back, so it needs the GPUI weak-handle pattern so
the result lands on the foreground executor at notify time. Mirrors
`do_refresh` exactly.

### Why GitHub's `releases/latest` JSON endpoint, not the HTML page

`api.github.com/repos/Rfluid/aura/releases/latest` returns 32 KB of
JSON with `tag_name` at the top level — easy to parse with
`serde_json` (already a workspace dep). The HTML page is ~150 KB
and would need regex / scraping. Anonymous GitHub API calls are rate-
limited to 60/hour per IP; once-per-app-start is well within that.

If we want to skip parsing entirely, the **redirect endpoint**
`https://github.com/Rfluid/aura/releases/latest` returns a 302 whose
`Location` header points at `.../releases/tag/vX.Y.Z` and we could
sniff the tag out of the URL. Cheaper but quirkier; defer unless the
JSON endpoint becomes problematic.

### Data shape

New module: `crates/aura/src/updater.rs`.

```rust
pub struct UpdateInfo {
    /// The remote tag, with the leading 'v' stripped — e.g. "0.1.18".
    pub latest: semver::Version,
    /// What the user clicks. The release page is fine as a fallback
    /// destination, but we prefer the README anchor (see below).
    pub release_url: String,
}

pub fn current_version() -> semver::Version {
    semver::Version::parse(env!("CARGO_PKG_VERSION")).expect("aura version is valid semver")
}

/// Synchronous network call. Spawned on the background executor; never
/// called from the render thread.
pub fn fetch_latest() -> anyhow::Result<UpdateInfo>;
```

`UpdateInfo` is stored on `AuraView` as
`update: Option<UpdateInfo>`. It is set once on app start by the
background task and never cleared at runtime (a successful dismissal
only writes the version into config — the field stays set so re-
opening the modal in the same session honours the click without a
flicker).

### Config additions

New table in `crates/aura-core/src/config.rs`:

```rust
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct UpdateConfig {
    /// The last release version the user dismissed via the "x" on the
    /// update button. Stored as the bare semver string ("0.1.18"). Any
    /// newer release re-shows the button. `None` means "never
    /// dismissed".
    pub dismissed_version: Option<String>,
    /// Master switch. When true, the update button is never rendered
    /// and the background check is skipped entirely (saves the GH
    /// request). Off by default.
    pub dismiss_all: bool,
}

impl Default for UpdateConfig {
    fn default() -> Self {
        Self { dismissed_version: None, dismiss_all: false }
    }
}

// In AppConfig:
#[serde(default)]
pub update: UpdateConfig,
```

Lives at the top level (not nested under `display`) because the dismiss
state is functional, not display preference, and because Zed's settings
do the same — auto-update lives in its own settings module.

On-disk shape (added to the default `config.toml`):

```toml
[update]
dismissed_version = "0.1.18"   # optional
dismiss_all       = false
```

### Why store in `config.toml` (not `state.json`)

`state.json` is for transient app-managed runtime state (`AppState` only
tracks `active_profile` today). `dismissed_version` is conceptually
similar — it is _state_, not user preference. But the user-facing
control _"dismiss all updates"_ is a true preference, and keeping the
two halves split across two files would surprise users who think they
disabled the prompts in config and then see `dismissed_version` drift
in `state.json`. Putting both in `config.toml` also matches the
intended behaviour: the user can hand-edit either field to mute / re-
enable the button without firing the UI.

### Header button placement

`render_header` in `crates/aura/src/app.rs:760` currently builds an
`actions` flex-row with three icon buttons: refresh, settings, more.

```text
┌── header ────────────────────────────────────────────────────┐
│ [logo] [⠋]                          [↻] [⚙] [⋯]              │   today
│ [logo] [⠋]    [Update · v0.1.18 →][x]  [↻] [⚙] [⋯]            │   with button
└──────────────────────────────────────────────────────────────┘
```

Place the new element **left of the icon group**, inside the same
flex-row, behind a `when(self.show_update_button(), ...)` guard so the
layout collapses cleanly when there is no update.

The component itself is bespoke (Zed's `UpdateButton` lives in
`gpui-zed-ui` and is intertwined with their `ButtonLike` /
`AnnouncementToast` stack — we are not adopting any of that). It is
~30 lines: a rounded bordered container with a label + a small "x"
child. Tooltip on the label reads "Open update instructions"; tooltip
on "x" reads "Hide until next release". Both wired through
`cx.listener` to view methods (`open_update_instructions`,
`dismiss_update`).

The button only paints when:

```rust
fn show_update_button(&self) -> bool {
    let Some(info) = &self.update else { return false; };
    if self.config.update.dismiss_all { return false; }
    match &self.config.update.dismissed_version {
        Some(v) if *v == info.latest.to_string() => false,
        _ => true,
    }
}
```

`dismiss_update` writes the latest version into
`self.config.update.dismissed_version`, calls `AppConfig::save(...)`,
and `cx.notify()`. Saving on dismiss matches how the rest of the modal
treats config writes (`open_config` opens the file; user edits;
re-load on next refresh) — but here the in-memory copy must update
immediately so the button hides on the same paint, so we mutate before
we write.

### More modal version label

Today the "Check updates" row in `render_more_modal`
(`app.rs:2063`) is a static label `"Check updates"`. Replace with:

```rust
let current = env!("CARGO_PKG_VERSION");
let label = format!("Check updates · v{}", current);
```

No state change; just a derived string. Clicking the row keeps its
existing behaviour (`open_url(GITHUB_RELEASES_URL)`) so the modal-side
flow stays "browse all releases", while the **header button** routes
to "do an update now" (the README anchor — see below).

### Download / update click-through

Clicking the header update button opens the browser at:

```
https://github.com/Rfluid/aura/blob/main/README.md#updating
```

The README's `### Updating` section (line 471 today) will be rewritten
to document the **two-curl flow** — the simple path the user asked for:

```bash
# Linux / macOS
curl -fsSL https://raw.githubusercontent.com/Rfluid/aura/main/uninstall.sh | bash
curl -fsSL https://raw.githubusercontent.com/Rfluid/aura/main/install.sh   | bash
```

```powershell
# Windows (PowerShell)
iex (irm https://raw.githubusercontent.com/Rfluid/aura/main/scripts/uninstall.ps1)
iex (irm https://raw.githubusercontent.com/Rfluid/aura/main/scripts/install.ps1)
```

This requires extracting two scripts out of the `justfile` so the
README can curl them:

- `uninstall.sh` (new, repo root) — mirrors the Linux + macOS arms of
  the `justfile uninstall:` recipe (lines 81–128). Detects OS, tears
  down systemd / launchd autostart, kills the process, removes the
  binary, removes the `.desktop` / `Aura.app`. macOS Git Bash falls
  through with the same redirect message `install.sh` uses today.
- `scripts/uninstall.ps1` (new) — mirrors the `justfile
uninstall-windows:` recipe (lines 132–140): stops `aura.exe`, removes
  Start Menu + Startup shortcuts, removes
  `%LOCALAPPDATA%\Programs\Aura`.

The `justfile` recipes become thin wrappers that just invoke the new
scripts so contributors keep one source of truth:

```make
uninstall:
    ./uninstall.sh

uninstall-windows:
    powershell -ExecutionPolicy Bypass -File scripts/uninstall.ps1
```

`install.sh` and `scripts/install.ps1` already work from
`curl | bash` / `iex (irm …)` — see `install.sh:5` "run via `curl |
bash`: downloads the latest GitHub release archive". The uninstall
scripts only need the same shebang + `set -euo pipefail` discipline,
no resolved `ROOT` directory (they operate purely on `~/.local/...` /
`%LOCALAPPDATA%`).

### Why the README anchor, not a dedicated `aura update` URL

We considered shipping a `aura update` CLI subcommand that runs the
uninstall+install pipe. Two reasons we don't:

1. **Trust** — piping `curl | bash` is something a user opts into
   knowingly. Hiding it behind an in-app button surprises users when
   `sudo` prompts (Windows) or password prompts (KWin keepalive rule)
   pop up.
2. **Re-entrancy** — the install scripts kill the running `aura`
   process. If `aura` is the one orchestrating that, we get a half-
   updated system. The button opens a browser; the user opens a
   terminal; the running tray dies cleanly when the install script
   does the `pkill -x aura`.

So the button is exactly a "here are the instructions" hand-off. The
README anchor reads as a checklist; the user pastes the two lines into
a terminal and is done.

### Cross-platform open

`crate::platform::open_url(url)` already handles all three platforms
(see `platform.rs:282–324`): `open` on macOS, `ShellExecute` on
Windows, `xdg-open` elsewhere. No new platform code is needed for the
click-through. The existing `ModalAction::Updates` handler already
exercises this path.

### Required plumbing

| Crate       | Item                                                                                          |
| ----------- | --------------------------------------------------------------------------------------------- |
| `aura-core` | `UpdateConfig` struct + `AppConfig.update` field + default round-trip test (`config.rs:501`). |
| `aura-core` | `semver` workspace dep (currently absent — used by the comparison in `aura`).                 |
| `aura`      | New `crates/aura/src/updater.rs` exposing `fetch_latest()` and `current_version()`.           |
| `aura`      | `AuraView.update: Option<UpdateInfo>` + spawn in `AuraView::new` or just after `do_refresh`.  |
| `aura`      | `render_header` adds the button left of the icon group (`app.rs:790`).                        |
| `aura`      | `render_more_modal` "Check updates" gains the `· vX.Y.Z` suffix (`app.rs:2068`).              |
| `aura`      | View methods `open_update_instructions` and `dismiss_update`; latter writes config to disk.   |
| Repo root   | New `uninstall.sh` (Linux + macOS arms of the existing justfile recipe).                      |
| `scripts/`  | New `scripts/uninstall.ps1` (Windows arm of the existing justfile recipe).                    |
| `justfile`  | `uninstall:` and `uninstall-windows:` recipes become thin wrappers around the new scripts.    |
| `README.md` | `### Updating` section rewritten to show the two-curl / two-iex flow.                         |

### Constants

Add to `app.rs` next to `GITHUB_REPO_URL` / `GITHUB_RELEASES_URL`:

```rust
const UPDATE_INSTRUCTIONS_URL: &str =
    "https://github.com/Rfluid/aura/blob/main/README.md#updating";
const GITHUB_RELEASES_API_URL: &str =
    "https://api.github.com/repos/Rfluid/aura/releases/latest";
```

Both are stable: the README anchor follows GitHub's slug rules
(`#updating` matches `### Updating`); the API URL is GitHub's public
contract.

### Error handling

The fetch task uses `ureq::get(GITHUB_RELEASES_API_URL).timeout(5s)`
and `serde_json` to decode `{ "tag_name": "v0.1.18" }`. Every error
path (timeout, non-200 status, missing field, unparseable semver) is
logged at `eprintln!` parity with `open_url_blocking` and silently
yields `None`. **There is no UI surface for "update check failed"** —
a degraded experience that hides the button is strictly preferable to
a noisy "couldn't check" banner.

Rate-limit headers (`X-RateLimit-Remaining`) are inspected only for the
log line. We don't gate the next call on them — the once-per-launch
cadence is well below the 60/hr anonymous quota and the user can run
Aura indefinitely without hitting it.

### Tests

- `config.rs` — extend `default_config_round_trips_through_toml`
  (line 501) to assert `UpdateConfig::default` deserialises from an
  empty `[update]` block, and that a populated block round-trips.
- `updater.rs` — unit test for a stubbed `parse_release_response`
  helper that takes a `&str` (JSON body) and returns
  `anyhow::Result<UpdateInfo>`. Covers happy path, missing
  `tag_name`, leading-`v` handling, and bad semver. Network is **not**
  mocked — `fetch_latest()` itself is left untested at the unit level
  (manual smoke on first run).
- `app.rs` — `show_update_button()` is pure: add a test that walks
  the four states (`dismiss_all=true`, `dismissed_version` matches,
  `dismissed_version` older, `update=None`) and asserts the bool.
  This is the highest-value test in the feature — it gates _all_ UI
  behaviour.

### Telemetry / privacy

The GitHub fetch sends:

- User-Agent: `aura/<version>` (required by GH; identifies the tool,
  not the user).
- Source IP (unavoidable, anonymous).
- No cookies, no auth headers, no body.

This matches what running `curl https://api.github.com/...` from a
terminal would send. Documented in the README updating section.

The `dismiss_all` toggle is the user-facing kill switch for the
network call. When `dismiss_all=true`, `fetch_latest()` is **not
called at all** — the background task short-circuits at spawn time.

## Implementation steps

Ordered so the repo stays compileable between steps.

1. **Add `semver` to `[workspace.dependencies]`** in the root
   `Cargo.toml` (or just to `aura/Cargo.toml`; root keeps it
   reusable). Add it to `aura/Cargo.toml`'s `[dependencies]`.
2. **`aura-core`: add `UpdateConfig`** + `AppConfig.update` with
   `#[serde(default)]`. Extend the round-trip test. Bump
   `aura-core` patch.
3. **`aura`: add `updater.rs`** exporting `current_version()`,
   `fetch_latest()`, `UpdateInfo`, and `parse_release_response`
   (the tested helper).
4. **`aura`: wire the fetch into `AuraView`.** Field
   `update: Option<UpdateInfo>`. Background-executor spawn on `new()`;
   on success, `this.update(cx, |view, cx| { view.update = Some(info);
cx.notify(); })`. Skip the spawn when `config.update.dismiss_all`.
5. **`aura`: header button.** Render conditionally; wire
   `open_update_instructions(cx)` and `dismiss_update(cx)` methods.
6. **`aura`: more-modal version label.** One-line change in
   `render_more_modal`.
7. **Repo: `uninstall.sh`.** Copy the Linux + macOS branches out of
   `justfile`; keep KWin-rule cleanup intact. Make executable.
8. **Repo: `scripts/uninstall.ps1`.** Copy the Windows branch out of
   `justfile`. PowerShell quoting is finicky — use a `param()`-less
   script so `iex (irm …)` works.
9. **`justfile`: recipes become wrappers.** Two-line bodies.
10. **`README.md`: rewrite `### Updating`.** Two-curl + two-iex blocks
    above the existing systemd-restart / kickstart notes. The notes
    stay (they apply when users build from source).
11. **Manual smoke (per platform).** Confirm: (a) the button appears
    when Aura's `Cargo.toml` version is artificially decremented and
    the binary launched, (b) clicking opens the README anchor in the
    default browser, (c) "x" dismisses and persists across a relaunch,
    (d) editing `dismiss_all = true` in `config.toml` and relaunching
    skips the network call (verify via stderr log line) and never
    renders the button.

## Rollout

Single PR — all pieces are interdependent. The button can be feature-
flagged off via `config.update.dismiss_all = true` if it misbehaves in
the wild, so a separate gate is unnecessary.

Add a `## Update button` entry to the `Roadmap` under `## v0.2 — Codex

- polish`(or`## v0.3 — Plugin ecosystem`, depending on where the
  0.2 cutline lands at merge time).

## Future work

- **Refetch on modal open** when the last successful check is older
  than 24h. Requires a `Instant` field on the in-memory state; no
  config impact. Cheap follow-up.
- **Update button copy variant for Goblin Mode.** Trivial once the
  feature lands — add fields like `lex.update_available` /
  `lex.dismiss_update` to `Lexicon`. Today the strings live inline in
  `app.rs`.
- **Tray-icon overlay badge** when an update is available, so users
  who never open the modal still see the nudge. Needs a per-platform
  tray-icon redraw path and is more invasive than the button.
- **`aura update` CLI** — opt-in subcommand that runs the two-curl
  flow non-interactively. Held off because of the re-entrancy risk
  noted above; would need a "spawn detached, then exec the script"
  trampoline.
- **Recurring fetch on a daily timer** while Aura is running, so
  long-lived tray sessions (Aura is the kind of app a user leaves up
  for weeks) eventually surface releases without a relaunch.

## References

- Zed's title-bar update button:
  `~/development/zed/crates/title_bar/src/update_version.rs` and
  `~/development/zed/crates/ui/src/components/collab/update_button.rs`.
  We borrow the state-machine + dismiss-x affordance; we do **not**
  adopt Zed's auto-downloader (`crates/auto_update/src/auto_update.rs`).
- GitHub releases API:
  `https://docs.github.com/en/rest/releases/releases#get-the-latest-release`.
- Existing cross-platform browser open:
  `crates/aura/src/platform.rs:282`.
- Existing config / state split:
  `crates/aura-core/src/config.rs`, `crates/aura-core/src/state.rs`.
