---
title: Antigravity agent
status: implemented
version: 1.0.0
last_updated: 2026-09-16
last_verified: 2026-09-16
source_refs:
    - crates/aura-core/src/quota/antigravity.rs
    - crates/aura-core/src/reader/antigravity.rs
    - crates/aura-core/src/config.rs
    - crates/aura-core/src/config_schema.rs
    - crates/aura-core/src/quota/mod.rs
    - crates/aura-core/src/reader/mod.rs
    - crates/aura-core/src/theme.rs
    - crates/aura/src/app.rs
    - crates/aura/src/tray_status.rs
    - crates/aura/src/cli/quota.rs
owner: "@rfluid"
tags: [agents, quota, antigravity, design]
---

# Antigravity agent

## Problem

Aura monitors Claude Code, Codex, and Gemini. Google's Antigravity CLI
(`agy`) is a fourth agent in daily use on the same machine, with its own
rate-limit windows that no other tool surfaces. A user running `agy`
alongside `claude` has no way to see how much of the Antigravity weekly
or 5-hour limit is left without dropping into the CLI and typing
`/usage`.

Adding Antigravity as a first-class `AgentKind` puts those windows in the
tray ring, the Quota tab, the Forecast tab, and `aura quota`.

## What Antigravity exposes

Investigated against `agy` 1.2.4 on Linux, 2026-09-16.

### Data directory

`~/.gemini/antigravity-cli/`, created on first `agy` run. Distinct from
the Gemini CLI subtree Aura already reads (`~/.gemini/tmp/<project>/chats/`),
so the two agents never collide despite sharing a parent.

Relevant contents:

| Path | Contents |
| --- | --- |
| `conversation_summaries.db` | SQLite. One row per conversation: id, title, preview, `step_count`, `last_modified_time`, `last_user_input_time`, `workspace_uris`, `app_data_dir`, `source`, `agent_name`. Covers both CLI and IDE conversations. |
| `conversations/<uuid>.db` | SQLite. `steps`, `gen_metadata`, `executor_metadata` — all **schema-less protobuf blobs**. |
| `history.jsonl` | Prompt history: `display`, `timestamp`, `workspace`, `type`. No usage data. |
| `settings.json`, `cache/`, `log/` | Config, caches, verbose gRPC logs. No usage data. |

The IDE (`~/.gemini/antigravity/conversations/<uuid>.pb`) uses an older
flat-protobuf format for the same trajectories.

### Quota — available, exact, free

```bash
agy -p "/usage" --output-format json
```

Returns:

```json
{
  "conversation_id": "",
  "status": "SUCCESS",
  "response": "Gemini Models\tWeekly Limit Remaining\t99%\t2026-09-23T22:57:05Z\n…",
  "num_turns": 0,
  "usage": { "input_tokens": 0, "output_tokens": 0, "total_tokens": 0 },
  "command": {
    "name": "usage",
    "data": {
      "description": "Within each group, models share a weekly limit and a 5-hour limit…",
      "groups": [
        {
          "name": "Gemini Models",
          "description": "Models within this group: Gemini Flash, Gemini Pro",
          "buckets": [
            { "id": "gemini-weekly", "name": "Weekly Limit Remaining", "window": "weekly",
              "remaining_fraction": 0.9999147057533264, "reset_time": "2026-09-23T22:57:05Z" },
            { "id": "gemini-5h",     "name": "Five Hour Limit Remaining", "window": "5h",
              "remaining_fraction": 0.9994884729385376, "reset_time": "2026-09-17T03:57:05Z" }
          ]
        },
        { "name": "Claude and GPT models", "buckets": [ { "id": "3p-weekly", … }, { "id": "3p-5h", … } ] }
      ]
    }
  }
}
```

Measured properties:

- **Free.** `num_turns: 0`, `usage.total_tokens: 0`. The slash command is
  handled locally against a cached backend reading; it does not start an
  LLM turn.
- **Non-draining.** Eight back-to-back invocations moved
  `remaining_fraction` only between `0.9999` and `1.0` — rounding noise on
  an idle account, not per-call consumption.
- **Live, not local.** These are backend numbers, the same ones the IDE
  shows. Treated as `QuotaSource::Api`.
- **~3 s wall time.** The binary is a 207 MB Go executable; startup
  dominates.
- **Failure mode.** Not logged in surfaces as
  `error getting token source: You are not logged into Antigravity.`
  in the CLI logs and a non-`SUCCESS` status.

### Why not a direct API call, like Claude Code and Codex

Claude Code and Codex both follow the same pattern: read an OAuth token
from a file the agent already wrote (`~/.claude/.credentials.json`,
`~/.codex/auth.json`), refresh it, call a stable HTTPS endpoint. That
pattern was investigated for Antigravity and rejected. Findings, all
verified on 2026-09-16:

**The endpoint exists.** `agy`'s verbose log names it:

```
quota_manager.go:45] doRefreshQuota: starting reload (force=true)
cache.go:135] Cache(retrieveUserQuotaSummary): Singleflight refresh failed:
  Post "https://daily-cloudcode-pa.googleapis.com/v1internal:retrieveUserQuotaSummary"
```

Both `cloudcode-pa.googleapis.com` and the `daily-` staging host serve
`POST /v1internal:retrieveUserQuotaSummary`. The wire schema is
`google/internal/cloud/code/v1internal/quota_summary.proto` —
`QuotaSummaryBucket` with a `fixed32 remaining_fraction`, a
`QuotaResetTime`, and `quota_bucket_key` / `quota_bucket_name` /
`quota_bucket_team`. Internal, unversioned, no public descriptor.

**The credentials are not in a file.** `agy` stores its OAuth token in
the OS keyring via `zalando/go-keyring`
(`ChainedAuth: authenticated via keyring (effective: keyring)`). On Linux
that is a Secret Service item with attributes
`{service: "gemini", username: "antigravity"}`, holding
`{auth_method, id_token, token: {access_token, refresh_token, token_type, expiry}}`.
The binary has a file-storage fallback, but only for hosts where the
keyring times out or no D-Bus session exists — not the normal path. Aura
would need the `keyring` crate and three platform backends, and would be
handling live Google OAuth credentials, which is a security surface the
current file-reading agents do not have.

**The call is license-gated and the gate was not reproducible.** Direct
`POST` with `{}` returns:

```
403 You do not have a valid license of this product.
```

That is with `agy`'s own keyring access token — so a valid bearer token
is not sufficient. `x-goog-user-project`, `client-metadata` (carrying the
`antigravity.env_prod.tier_paid` blob the binary embeds), and
`x-goog-api-client` were each tried and each still 403. A separate token
on the same machine (`~/.gemini/antigravity-acp/acp_token.json`, which
*is* a plain file with a refresh token) authenticates fine against
`v1internal:loadCodeAssist` but comes back
`UNSUPPORTED_CLIENT … please migrate to the Antigravity suite`, i.e. the
license is bound to the CLI's own OAuth client identity plus a handshake
Aura has not reconstructed.

Cost of going direct: reverse-engineered internal proto + a
license/identity handshake + cross-platform keyring credential handling,
all undocumented and free to change in any `agy` release. Cost of the
subprocess: 3 s and a process spawn against `--output-format json`, a
documented public flag. The subprocess wins until Antigravity either
writes quota to disk or publishes the API.

### Token history — not available

`conversations/<uuid>.db` blobs and the IDE `.pb` files contain no
plaintext model names and no field names. Token counts do exist in the
wire data (`agy` embeds
`Interaction_Usage_ModelInvocationTokenCounts`), but recovering them
means guessing undocumented protobuf field numbers that Google can
renumber in any release. Out of scope; revisit only if Antigravity ships
a documented export.

## Scope

**In:**

- `AgentKind::Antigravity`, kebab slug `antigravity`.
- Quota + Forecast parity with the other agents, sourced from `agy`.
- An activity-only reader: sessions, messages, active days, streaks,
  peak hour, first/last dates. Tokens and per-model breakdown stay empty.
- Icon, accent color, detection in `aura setup-config` and the installers,
  docs.

**Out:**

- Per-model token attribution and cost estimation (see above).
- Reading the Antigravity IDE's trajectories.

## Design

### 1. Quota source — `crates/aura-core/src/quota/antigravity.rs`

New `AntigravityQuota { command: PathBuf, data_dir: PathBuf }`.

`snapshot()`:

1. Resolve the binary: the agent's configured `command`, else `agy` from
   `PATH`. Not found → `QuotaSnapshot::unavailable("agy not found on PATH")`.
2. Spawn `agy -p "/usage" --output-format json` with a hard timeout
   (10 s) and `cwd` set to the data dir's parent so no workspace trust
   prompt is triggered.
3. Parse `command.data.groups[].buckets[]` with `serde`.
4. Map each bucket to a `QuotaWindow`:
   - `label` — `"<group short name> <window>"`, e.g. `"Gemini · 5h"`,
     `"Gemini · week"`, `"Claude/GPT · 5h"`, `"Claude/GPT · week"`.
   - `used_percentage` — `(1.0 - remaining_fraction) * 100.0`.
   - `used_tokens` — `None`; Antigravity reports cost-weighted fractions,
     not tokens.
   - `resets_at` — `reset_time`.
   - `length_minutes` — `300` for `"5h"`, `10080` for `"weekly"`. This is
     what lets the Forecast tab project the window.
5. `subscription_type` — `None` (not exposed by `/usage`).
6. Any spawn, timeout, non-`SUCCESS` status, or parse failure →
   `api_failed: true` plus a `note`, so `tray_status::summarize_sticky`
   holds the last good reading instead of degrading the icon.

**Window order** is `[Gemini · 5h, Gemini · week, Claude/GPT · 5h, Claude/GPT · week]`,
not the order `/usage` prints them. This preserves the repo-wide
convention that position 0 is the session window and position 1 is the
week, so the `tray_progress_source` / `tray_color_source` defaults
(`0` and `1`) keep meaning the same thing on this agent as on every other.

**Caching.** The tray poll floor is 30 s (`TrayConfig::refresh_interval`)
and the modal refreshes on open. A 3 s subprocess on each is tolerable
because quota fetches already run on `background_executor`, but the
snapshot is cached with a short TTL (~20 s) so a modal open right after
a tray tick reuses the reading rather than re-spawning a 207 MB binary.

### 2. Reader — `crates/aura-core/src/reader/antigravity.rs`

`AntigravityReader { config_path }` over `conversation_summaries.db`.

- Opens the DB **read-only** (`file:…?mode=ro`) so a running `agy` is never
  disturbed, and copies nothing. Not `immutable=1` by default, contrary to
  the original sketch: the DB is in WAL mode, and `immutable=1` reads the
  main file alone, so it would silently miss every commit `agy` had not yet
  checkpointed. `immutable=1` is the fallback for when the `-shm` file
  cannot be created.
- One row → one session. `step_count` → messages. `last_user_input_time`
  (falling back to `last_modified_time` when it is the zero timestamp —
  older rows have `0001-01-01`) → the session's date and hour.
- Period filtering by that date; `AllTime` reads every row.
- Produces `UsageSnapshot` with `total_sessions`, `total_messages`,
  `active_days`, `total_days`, `streaks`, `peak_hour`, `daily_activity`,
  `first_session_date`, `last_session_date`. Token fields and `per_model`
  stay at their defaults, and `favorite_model` is `None`.
- Optionally filters on `app_data_dir` so a profile can scope to CLI-only
  conversations; default is every row.

**Decided:** `diesel` (sqlite backend, no default features) plus
`libsqlite3-sys/bundled`. `history.jsonl` was rejected on measurement — on
the reference machine it held 4 prompt lines against the DB's 13
conversations, carries no `step_count`, and has a `conversationId` on only
some entries. The "new C dependency" objection turned out to be weaker than
the plan assumed: `cc` is already in `Cargo.lock` via seven deps in the gpui
tree, and all six release targets build on native runners, so the bundled
amalgamation needs no cross-compiler.

`agy` is Go/gorm, so both timestamp columns are declared `Text` and
normalised by hand — diesel's chrono deserializer expects
`%Y-%m-%d %H:%M:%S%.f` and these carry a `+00:00` offset.

The UI consequence: the Models tab renders empty and the Overview's token
counters read zero for this agent. Both need an explicit "not reported by
this agent" state rather than a bare `0`, mirroring how Gemini's
percentage-less quota windows already suppress their progress bar.

### 3. Config

- `AgentKind::Antigravity` in `config.rs`; `resolved_config_path()`
  default `~/.gemini/antigravity-cli`.
- New optional `AgentConfig::command: Option<String>` — the agent's
  executable, for installs outside `PATH`. Serialized only when set;
  documented in `config_schema.rs` and `docs/configuration.md`. Used by
  the Antigravity quota source today, available to any future
  CLI-backed agent.
- Detection list in `setup-config` gains
  `("Antigravity", AgentKind::Antigravity, ~/.gemini/antigravity-cli)`.
- `config_schema.rs`: `allowed: &["claude-code", "codex", "gemini", "antigravity"]`,
  path summary updated.

### 4. Presentation

- `assets/icons/antigravity.svg` — already on disk, currently untracked;
  `git add` it.
- `assets.rs` icon registration + `app.rs` kind → icon path arm.
- `theme.rs::agent_kind_default_color` — Antigravity brand tint. The
  mark is a monochrome arc; pick a violet (`0x7c5cff`) so it reads
  distinctly against Gemini's `0x4285f4`.

### 5. Match arms to extend

`make_reader` (`reader/mod.rs`), and the three quota dispatch sites:
`app.rs:619`, `tray_status.rs:228`, `cli/quota.rs:33`. All are total
matches, so the compiler enumerates them.

### 6. Docs

`README.md`, `docs/configuration.md` (the new `command` key),
`docs/cli.md`, `.design/agents.md`, `.agent/context/glossary.md`
(the Agent entry lists supported agents), `install.sh`,
`scripts/install.ps1`.

## Testing

- `quota/antigravity.rs`: parse fixtures for the healthy response, a
  not-logged-in response, and malformed JSON. Window ordering and the
  `remaining_fraction` → `used_percentage` inversion get their own
  assertions. Binary spawning is behind a trait or a command path so the
  tests never invoke `agy`.
- `reader/antigravity.rs`: build a temp SQLite DB with known rows; assert
  session/message counts, streaks, peak hour, period filtering, and the
  zero-timestamp fallback.
- `tray_status`: an Antigravity snapshot with `api_failed: true` holds
  the previous reading.
- Manual: `aura quota --format json` against the live CLI.

## Risks

| Risk | Mitigation |
| --- | --- |
| `/usage` JSON shape changes across `agy` releases | Parse defensively — unknown fields ignored, missing `command.data` → `unavailable` with a note naming the version |
| 3 s subprocess on every poll | Background executor + TTL cache + hard timeout |
| `agy` not installed or not logged in | `unavailable` with an actionable note; no panic, no tray degradation |
| New `rusqlite` dependency | Decide reader backing (SQLite vs `history.jsonl`) before implementing |
| Empty Models tab reads as a bug | Tab hidden entirely for agents that report no tokens |

## Resolved questions

1. **SQLite dependency** — `diesel` + `libsqlite3-sys/bundled`, over
   `rusqlite` and over the `history.jsonl` fallback. See §2.
2. **CLI-only or both** — both. No `app_data_dir` filter. On the reference
   machine the split is 1 CLI row against 12 IDE rows going back to
   2025-12; filtering would reduce a user's whole Antigravity history to
   whatever they had typed into `agy` since installing it. The DB is the
   account's, and `agy` itself writes and reads every row in it. The IDE's
   *trajectories* (`~/.gemini/antigravity/conversations/<uuid>.pb`) stay out
   of scope, as planned — those are a different file format entirely.

## Delivered beyond the plan

- **`crates/aura-core/src/bin_path.rs`.** The first build resolved `agy` with
  a bare `Command::new("agy")`, which works from a terminal and fails in the
  tray: the systemd user manager's `PATH` is
  `~/.cargo/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin:…`
  with no `~/.local/bin`, where `agy` installs itself. The plugin runner had
  already solved the same problem with an `augmented_path()` helper; that
  logic is now a shared module, and the quota source resolves the binary to
  an absolute path before spawning. Resolving up front rather than only
  widening the child's `PATH` matters for portability — Unix resolves a bare
  program name against the `PATH` handed to the child, but Windows resolves
  it against the parent's. The child still gets the widened `PATH` so `agy`
  can find its own helpers, and a genuine miss now names where Aura looked
  and what to set. `%LOCALAPPDATA%\agy\bin` is searched on Windows, which is
  where Antigravity's own installer puts the binary and which no generic
  bin-directory list would guess.
- **`CREATE_NO_WINDOW` on the `agy` spawn.** `aura` is built with
  `windows_subsystem = "windows"`, so Windows opens a console for every
  console-subsystem child — `agy` is one, and the tray polls every 30 s.
  Without the flag the user gets a CMD window flashing at them twice a
  minute, forever. The plugin runner already had this; the quota source did
  not.

- **A capability split for "this agent has no tokens".**
  `AgentKind::reports_tokens()` answers it synchronously, before any read,
  and drives the tab row; `UsageSnapshot::tokens_unreported` carries the same
  fact on a loaded snapshot and drives the renderers. A test pins the two
  together so they can't drift. The plan asked for an explicit
  "not reported by this agent" state; what shipped is stronger — the **Models
  tab is not offered at all** for such an agent, since both the
  tokens-per-day chart and the per-model bars are token-derived and the page
  would be blank. The selected section is resolved against the visible list
  at render time rather than clamped into state, so switching profiles while
  sitting on Models falls back to Quota instead of desyncing. The Summary
  stat cards and `aura usage --format text` still say "not reported" via a
  `tokens_not_reported` `Lexicon` entry.
- **`aura quota`'s `WINDOW` column widened 14 → 18.** Antigravity's
  `Claude/GPT · week` is the first 17-character label any backend has
  produced, and it pushed every following column out of alignment.
- **Deterministic `peak_hour`** (`reader/claude_code.rs`). `hour_counts` is
  a `HashMap` and `max_by_key` keeps the *last* maximum it sees, so ties
  resolved by hash order and the stat changed between runs on unchanged
  data. Pre-existing and agent-agnostic, but Antigravity's sparse histories
  hit it immediately — the live DB has a four-way tie at three
  conversations each. Ties now resolve to the earliest hour.

## Cross-platform notes

Verified without access to those machines, by reading the sources that decide
the behaviour:

- **SQLite URI form.** Checked against `sqlite3ParseUri` in the bundled
  amalgamation. While parsing the filename it treats exactly three bytes
  specially — `%`, `?`, `#` — so `&` and `=` are literal and must *not* be
  encoded. There is no drive-letter special case, so `file:C:/dir/x.db`
  yields `C:/dir/x.db` unchanged. The one trap is that the authority check
  (`zUri[5]=='/' && zUri[6]=='/'`) runs on the raw string *before*
  percent-decoding, so a UNC path would be rejected as
  `invalid uri authority: server`; the second slash is encoded to sidestep it.
- **Data directory.** `~/.gemini/antigravity-cli` on all three platforms —
  the `agy` binary hardcodes that relative path, with no `AppData` variant.
- **Bundled SQLite on `aarch64-pc-windows-msvc`.** This is the one release
  target that is *not* native: it cross-compiles on an x86_64
  `windows-latest` runner. (An earlier note in this doc claimed all six were
  native; that was wrong.) The runner image does carry
  `Microsoft.VisualStudio.Component.VC.Tools.ARM64`, so `cc` can build the
  amalgamation, and that target is already `experimental: true` /
  `continue-on-error`.
- **macOS sandbox.** `build-macos-app.sh` signs with `--options runtime` and
  no entitlements file, so the bundle uses the hardened runtime but is not
  sandboxed — spawning `agy` is unrestricted.
- **Compilation.** The `#[cfg]`-gated code (`CREATE_NO_WINDOW`, the Windows
  `PATHEXT` candidates, the non-Unix `is_executable`, `agy_install_dirs`)
  type-checks on all six release targets.

## Verified against the live install

`agy` 1.2.4, `~/.gemini/antigravity-cli`, 2026-09-16:

- `aura setup-config` detects Antigravity and writes the profile.
- `aura quota --profile Antigravity` returns all four windows,
  `source: api`, correct reset times, `length_minutes` set.
- `aura usage --profile Antigravity` reports 13 sessions / 1673 messages
  over a 278-day span, and 1 session / 2 messages for `--period 7d`.
- `assets/icons/antigravity.svg` renders to a clean arc silhouette through
  `usvg`/`resvg` 0.45 — the same pipeline `gpui::SvgRenderer` uses, masks
  and blur filters included.
