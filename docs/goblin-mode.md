---
title: Goblin Mode (easter egg)
status: design
version: 0.1.0
last_updated: 2026-05-27
last_verified: 2026-05-27
source_refs:
  - crates/aura-core/src/config.rs
  - crates/aura/src/app.rs
  - crates/aura/src/format.rs
  - docs/forecast-tab.md
owner: "@rfluid"
tags: [easter-egg, ui, lexicon, design]
---

# Goblin Mode (easter egg)

## Problem

Every user-facing string in the modal — tab labels, empty states,
loading copy, status badges, projection blurbs — is hard-coded inline
in `crates/aura/src/app.rs` and (for tokens / costs) `format.rs`.
Adding a single alt-tone variant (let alone iterating on it) means
hunting through ~76 KB of view code.

We want a config-gated alternate persona — codename **Goblin Mode** —
that swaps the polite default copy for unhinged, slop-laced,
profanity-flecked variants. The trigger is a single boolean in
`config.toml`; flipping it back restores the normal copy without a
restart.

This is also the right moment to centralize UI copy: even without the
easter egg, having one place to read every string the modal renders
makes future localization / theming / a11y review trivial.

## Scope

Replace **the user-facing copy** of the modal — not the layout, not
the data, not the colors. Everything Goblin Mode touches is a string
the user reads.

In scope:

- **Tab labels**: `Quota`, `Summary`, `Models`, `Plugins`,
  `Forecast` (added by [[forecast-tab]] — see
  [Dependency](#dependency-forecast-tab-first)).
- **Status badges & enum-mapped copy**: `ForecastStatus`
  (`Ok` / `Watch` / `Overshoot` / `Insufficient`), `Subscription:`,
  `Resets <when>`, period pills (`all` / `7d` / `30d`).
- **Empty / loading / error states**: `Loading…`, `No quota data
  available.`, `No plugins configured`, `No plugin selected`,
  `warming up — check back in a few minutes` (forecast),
  generic error banner copy.
- **More-menu entries**: `Open config file`, `Themes`, future
  Settings/Refresh labels.
- **Forecast-tab subtext**: `Projected at reset`, `Will hit 100%
  at HH:MM`.

Out of scope for v1:

- **Plugin-supplied strings.** Plugins ship their own copy via JSON
  IPC; we don't rewrite their output. If a plugin wants in, it can
  read the active persona from a future env var (`AURA_PERSONA`)
  itself.
- **Agent/profile names, plugin display names.** User-authored config
  values are sacred.
- **Numbers, dates, paths, error details.** Goblin Mode wraps the
  prose around them; it does not mangle data.
- **Localization.** The lexicon is structurally ready for it, but v1
  ships English only — both personas.
- **Sound effects, animations, ASCII art.** Tempting, but out of
  scope — text-only.

## Non-goals

- Hiding the easter egg. It's a documented config field with a clear
  name. We don't want users flipping it on by accident, but we also
  don't want a "find the secret" treasure hunt — that ages badly and
  invites support tickets we can't reproduce.
- Toning the persona down per-string. A persona is a whole vibe; if a
  given line is too much for a user, they turn the persona off.
- Multiple personas in v1. Architecture supports it (see
  [Lexicon shape](#lexicon-shape)); shipping it means writing a
  second variant lexicon plus a config enum. Defer.

## Design

### Trigger / config

Add one field under `[display]` in `crates/aura-core/src/config.rs`:

```toml
[display]
# Swap the modal's UI copy for an aggressive / unhinged variant.
# Default false. Toggling reloads on the next refresh — no restart.
goblin_mode = false
```

```rust
// crates/aura-core/src/config.rs — DisplayConfig
#[serde(default)]
pub goblin_mode: bool,
```

`Default` returns `false`. Hot-reload piggy-backs on the existing
config refresh path (same code path as `dismiss_on_focus_loss`,
`window_chrome`, etc.) — no new infrastructure.

**Why a bool, not an enum.** v1 ships one alt persona. When a second
lands, migrate to `persona: PersonaKind` with `#[serde(default)]` and
a `From<bool>` shim so old configs keep working for one release
cycle, then drop the bool.

### Lexicon shape

New module `crates/aura-core/src/lexicon.rs`:

```rust
pub struct Lexicon {
    // Tabs
    pub tab_quota: &'static str,
    pub tab_summary: &'static str,
    pub tab_models: &'static str,
    pub tab_plugins: &'static str,
    pub tab_forecast: &'static str,

    // Forecast status badges
    pub forecast_ok: &'static str,
    pub forecast_watch: &'static str,
    pub forecast_overshoot: &'static str,
    pub forecast_insufficient: &'static str,
    pub forecast_projected_at_reset: &'static str,
    pub forecast_will_hit_100_fmt: fn(time: &str) -> String,
    pub forecast_warming_up: &'static str,

    // Empty / loading / error
    pub loading: &'static str,
    pub no_quota_data: &'static str,
    pub no_plugins_configured: &'static str,
    pub no_plugin_selected: &'static str,
    pub error_banner_prefix: &'static str,

    // Quota row chrome
    pub subscription_fmt: fn(sub: &str) -> String,
    pub resets_fmt: fn(when: &str) -> String,

    // More menu
    pub menu_open_config: &'static str,
    pub menu_themes: &'static str,

    // Period pills
    pub period_all: &'static str,
    pub period_7d: &'static str,
    pub period_30d: &'static str,
}

pub const POLITE: Lexicon = Lexicon { /* current copy, verbatim */ };
pub const GOBLIN: Lexicon = Lexicon { /* see drafts below */ };

pub fn pick(goblin_mode: bool) -> &'static Lexicon {
    if goblin_mode { &GOBLIN } else { &POLITE }
}
```

A few entries are `fn` rather than `&'static str` so persona-specific
formatting (different word order, extra glyphs, capitalization quirks)
stays inside the lexicon instead of leaking into view code.

**Why one flat struct instead of a trait + two impls.** A struct is
trivially `const` and can live in `aura-core` with zero runtime cost.
A trait would force `dyn Lexicon` (slower) or generic plumbing
through the view layer (uglier). The struct is also the
nearly-mechanical refactor: every site that used a string literal
becomes `lex.<field>`.

### Persona drafts

Voice rules for `GOBLIN`:

- **Tonal target**: gremlin energy. Tired, sarcastic, mildly hostile,
  occasionally affectionate toward the user. Think a friend roasting
  your coding habits.
- **Profanity**: allowed but rationed. `damn`, `hell`, `crap`, `bs`,
  `shit` are fair game; harder slurs and anything targeting protected
  classes are not. Never aimed _at the user_.
- **Length**: don't blow up the layout. Goblin copy must fit the same
  visual budget as the polite copy (tab labels stay one word; status
  badges stay short).
- **Data sanctity**: numbers, dates, paths render exactly as they do
  in `POLITE`. Goblin Mode never makes the user re-derive a value.

Working drafts (final pass during impl; treat these as direction, not
spec):

| Field | Polite | Goblin |
|---|---|---|
| `tab_quota` | Quota | Damage |
| `tab_summary` | Summary | The Bill |
| `tab_models` | Models | Slop Vendors |
| `tab_plugins` | Plugins | Hangers-on |
| `tab_forecast` | Forecast | Doom |
| `forecast_ok` | OK | Fine, whatever |
| `forecast_watch` | Watch | Bruh |
| `forecast_overshoot` | Overshoot | Cooked |
| `forecast_insufficient` | Insufficient | Idk yet |
| `forecast_projected_at_reset` | Projected at reset | What you'll burn |
| `forecast_will_hit_100_fmt(t)` | Will hit 100% at {t} | Goose is cooked at {t} |
| `forecast_warming_up` | warming up — check back in a few minutes | give it a minute, jeez |
| `loading` | Loading… | hold on damn |
| `no_quota_data` | No quota data available. | Nothing. Empty. Dry. |
| `no_plugins_configured` | No plugins configured | No hangers-on |
| `no_plugin_selected` | No plugin selected | Pick one, coward |
| `subscription_fmt(s)` | Subscription: {s} | Paying for: {s} |
| `resets_fmt(w)` | Resets {w} | Wipes {w} |
| `menu_open_config` | Open config file | Crack open the config |
| `menu_themes` | Themes | Paint job |
| `period_all` | all | the whole damn time |
| `period_7d` | 7d | last week |
| `period_30d` | 30d | this month-ish |

Period pills are tight on width; if `the whole damn time` overflows
in practice, fall back to `forever` / `week` / `month` — verify in
the browser-equivalent render pass during impl.

### Wiring sites

Sites that today use a literal need to read from the active lexicon
instead. Cataloged from `app.rs` / `format.rs` at time of writing
(line numbers will drift — re-grep at impl time):

1. `AgentSection::label()` — `app.rs:45` — return `lex.tab_*`. After
   forecast lands, the new `AgentSection::Forecast` arm joins the
   same match.
2. Mode pill `("Plugins", …)` — `app.rs:934` — read `lex.tab_plugins`.
3. Loading copy — `app.rs:1218` — `lex.loading`.
4. `Subscription: {sub}` — `app.rs:1241` — `lex.subscription_fmt`.
5. `No quota data available.` — `app.rs:1261` — `lex.no_quota_data`.
6. `Resets {label}` — `app.rs:1405` — `lex.resets_fmt`.
7. `No plugins configured` — `app.rs:879` — `lex.no_plugins_configured`.
8. `No plugin selected` — `app.rs:1153` — `lex.no_plugin_selected`.
9. `Open config file` — `app.rs:1968` — `lex.menu_open_config`.
10. `Themes` — `app.rs:1990` — `lex.menu_themes`.
11. Forecast strings — added by [[forecast-tab]]; wire to lexicon at
    creation time, not in a follow-up pass (see
    [Dependency](#dependency-forecast-tab-first)).

`AuraView` reads the active lexicon once per render via
`lexicon::pick(self.config.display.goblin_mode)`. No new state field —
the config is already on `self`.

### Hot reload

The existing refresh-button / hot-reload path already re-parses
`config.toml` (see `app.rs:295` `do_refresh()` and the
`apply_refresh_result()` callsite). `goblin_mode` rides that bus for
free — the next render picks up `&GOBLIN` instead of `&POLITE`. No
window recreate, no restart.

### Safety rails

- **Test fixtures stay polite.** Snapshot / golden tests assert
  against `POLITE` only. Goblin's strings have a smaller, dedicated
  test (the per-field length budget; see below).
- **Length budget test.** Unit test in `lexicon.rs`: every
  `GOBLIN` entry whose `POLITE` counterpart is ≤ N chars must also be
  ≤ N + 8 (or whatever the layout tolerates — tune during impl).
  Stops drift where Goblin copy quietly breaks the tab row.
- **Profanity boundary test.** Tiny deny-list (slurs, protected-class
  targeting) checked against every `GOBLIN` string at compile time
  via a `const fn` or a `#[test]`. Cheap, catches accidents during
  PRs.
- **Screenshot-safe default.** `goblin_mode = false` is the only
  documented default; the README / docs screenshots always show
  `POLITE`.

## Dependency: Forecast tab first

[[forecast-tab]] (`docs/forecast-tab.md`) adds:

- `AgentSection::Forecast` (new enum variant + `label()` arm).
- `ForecastStatus { Ok, Watch, Overshoot, Insufficient }`.
- Subtext strings: `Projected at reset`, `Will hit 100% at HH:MM`,
  `warming up — check back in a few minutes`.

All of those are baked into the Goblin lexicon above. **Forecast
ships first**, with strings hard-coded in the natural way; Goblin
Mode is the immediate follow-up PR that:

1. Lands `lexicon.rs` and the `display.goblin_mode` config field.
2. Refactors every catalogued site (including the brand-new Forecast
   sites) to read from the lexicon.
3. Ships `POLITE` (verbatim current copy) + `GOBLIN` (drafts above).

Doing it in this order means the Forecast PR isn't held up reviewing
two unrelated concerns at once, and the Goblin PR is a single
mechanical refactor + one new module — easy to review, easy to revert.

After Goblin lands, the project rule becomes: **new user-facing
string ⇒ new `Lexicon` field, in both personas, same PR.** Add a
short note to `docs/ui-design.md` so contributors see it.

## Rollout

### Phase 0 — Forecast tab lands

Tracked separately in [[forecast-tab]]. Goblin work does not start
until Forecast is in `main`.

### Phase 1 — Lexicon scaffolding

- Add `lexicon.rs` with the `Lexicon` struct and `POLITE` constant
  matching today's copy verbatim (no behavior change).
- Add `display.goblin_mode: bool` to `DisplayConfig` with
  `#[serde(default)]`.
- Refactor every site in the [Wiring sites](#wiring-sites) catalog
  to read from `lexicon::pick(...)`. Diff should be 1:1 — same
  rendered strings, no visual change with `goblin_mode = false`.
- Tests: confirm existing snapshot / golden tests still pass
  unchanged.

### Phase 2 — Goblin persona

- Add `GOBLIN` constant with drafts from the table above
  (finalize wording during PR review).
- Length-budget test.
- Profanity-boundary test.
- Docs: append a short `## Goblin Mode` section to
  `docs/configuration.md` documenting the flag, with a one-line
  warning about tone.

### Phase 3 — Field & polish

- Internal dogfood for a week. Adjust copy where Goblin lines
  overflow, fall flat, or punch down instead of sideways.
- Decide whether v2 needs a second persona (`persona: PersonaKind`)
  or whether one alt is enough. Captured in
  [Open questions](#open-questions).

## Open questions

- **Naming.** "Goblin Mode" is the working codename. Alternatives
  considered: `feral_mode`, `cursed_mode`, `slop_mode`. Codename and
  config-field name don't have to match — e.g. ship as `goblin_mode`
  in config but refer to the persona as "Feral" in UI/docs. Defer
  until Phase 2 PR.
- **Plugin opt-in.** Should plugins receive `AURA_PERSONA=goblin` so
  their JSON output can match the host vibe? Cheap to add; defer
  until a plugin actually asks.
- **Per-tab opt-out.** Could imagine `goblin_mode_except = ["Quota"]`
  for users who want chaos everywhere _except_ the screen they
  screenshot for receipts. Almost certainly YAGNI — the global
  toggle is the receipts.
- **Tray-icon copy.** The tray tooltip / menu items are platform
  code (`crates/aura/src/tray.rs`, `platform.rs`). v1 scope is the
  modal only; tray comes after if Phase 3 dogfood shows the contrast
  is jarring.
- **Theme coupling.** Should Goblin Mode also nudge accent colors
  (e.g. force a punchier accent)? Out of scope — theme is its own
  knob. Users can stack them.

## Tracking

Roadmap entry: add under "Backlog / under consideration" once Forecast
is in flight — single bullet, link to this doc.
