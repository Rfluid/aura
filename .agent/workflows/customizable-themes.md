---
title: Customizable themes — implementation plan
status: draft
version: 0.1.0
last_updated: 2026-05-22
last_verified: 2026-05-22
source_refs:
  - .design/customization.md
  - .design/tokens.md
  - .design/agents.md
  - crates/aura/src/app.rs
  - crates/aura-core/src/config.rs
owner: "@rfluid"
tags: [theming, customization, plan]
---

# Customizable themes — implementation plan

Maps `.design/customization.md` onto concrete code changes. Spec is the
source of truth for schema / precedence; this file is the work order.

## Scope

Land `~/.config/aura/theme.toml` as an override layer on top of the
hard-coded `COLOR_*` constants in `crates/aura/src/app.rs:19-31`.
Defaults preserved; missing file → current look. Per-agent overrides
reuse the existing `AgentConfig.color` plumbing (already implemented at
`crates/aura-core/src/config.rs:29`), so this phase focuses on global
tokens and the file load + hot-reload wiring.

## Step 1 — `Theme` data model (`crates/aura-core/src/theme.rs`)

New module, parallel to `config.rs`. Why core, not the binary: keeps it
unit-testable without GPUI and lets us reuse `parse_hex_color` from
`config.rs:293`.

```rust
pub struct Theme {
    pub colors: ThemeColors,
    pub typography: ThemeTypography,
    pub spinner: ThemeSpinner,
    pub agents: HashMap<String, AgentTheme>,
}

pub struct ThemeColors {
    pub bg: u32, surface: u32, surface_hi: u32, border: u32,
    pub text: u32, text_dim: u32,
    pub accent: u32, accent_dim: u32,
    pub error: u32, on_accent: u32,
    pub warning: u32,           // covers COLOR_WARNING (used in render_fallback_warning)
    pub agent_fallback: u32,
}
```

- `Theme::default()` returns the current `app.rs:19-31` constants
  verbatim — single source of truth moves here.
- `Theme::load(path)` reads TOML, applies overrides field-by-field on
  top of `default()`, returns `Theme::default()` if the file is missing.
- Hex parsing: reuse `parse_hex_color`; warn-and-skip on 4/8-digit
  (alpha) hex via `tracing::warn!`.
- Unknown TOML keys: deserialize with `#[serde(deny_unknown_fields)]`
  **off** — collect leftover keys with `toml::Value` and `tracing::warn!`
  them, per the spec's "forward compatibility matters more than
  strictness".
- Export from `aura-core/src/lib.rs`.

Unit tests in the new module: defaults round-trip, partial override
leaves other keys at default, bad hex falls through with a default
value (no panic), per-agent map parsing handles quoted keys with
spaces.

## Step 2 — Wire `Theme` into `AuraView`

`crates/aura/src/app.rs`:

1. Add `theme: Theme` field to `AuraView` (after `config_path`),
   default-loaded in `new`.
2. Add a `theme_path: PathBuf` field (sibling of `config_path` —
   `~/.config/aura/theme.toml`).
3. Delete the `const COLOR_*` block at lines 19-31, plus
   `COLOR_AGENT_FALLBACK` at 1877. Replace every call-site (≈103
   references — `Grep -c` confirms this) with `self.theme.colors.bg`
   etc.
4. Hoist the inline `0xff6b6b` (error text, lines 902 / 962) and
   `0xffffff` (on-accent text in `render_mode_toggle` line 766) into
   `theme.colors.error` / `theme.colors.on_accent`. These are TODOs
   already flagged in `.design/tokens.md:36-44`.
5. Free helpers `agent_accent`, `plugin_accent`, `on_accent_text`,
   `relative_luminance`, `blend` currently read consts directly. Either
   convert to methods on `Theme` or pass `&Theme` through. Methods on
   `Theme` are cleaner — `theme.agent_accent(agent)` reads the
   per-agent override first, then `agent_kind_default_color`, then
   luminance-fallback to `theme.colors.agent_fallback`.

Precedence locked exactly per spec:
`theme.agents[name].accent` → `AgentConfig.color` →
`theme.colors.accent` (plugin surfaces) / per-kind brand color (agent
surfaces) → luminance fallback last. The two color sources (theme file
vs config file `[[agents]] color`) overlap; spec implies theme file
wins — call this out in a doc comment so future readers don't trip on
it.

## Step 3 — Hot reload through the existing refresh path

`do_refresh` at `app.rs:274` already reloads `AppConfig`. Mirror the
pattern:

```rust
let theme = Theme::load(&theme_path).unwrap_or_else(|e| {
    tracing::warn!("theme reload failed: {e}");
    Theme::default()
});
```

Add `theme: Option<Theme>` to `RefreshResult`, apply it in
`apply_refresh_result` alongside config. The refresh button at
`app.rs:623` becomes the theme reload trigger for free, per spec
§"Hot reload".

## Step 4 — Modal "Themes" item

`app.rs:1705-1712` currently shows a "coming soon" error. Repurpose to
open `theme.toml` in the user's editor — mirror `open_config`
(`app.rs:356-386`). Create the file with `Theme::default()` serialized
as TOML on first click so the editor doesn't open a blank buffer. Keep
the modal item label "Themes" and icon (`sliders.svg`).

## Step 5 — Docs

- Add a "Themes" section to `README.md` after "Configuration" with the
  `theme.toml` example and the precedence rule, linking
  `.design/customization.md` for the full reference.
- Update `docs/configuration.md` similarly.
- Mark `.design/customization.md` status as "implemented" (top
  frontmatter), referencing the new `theme.rs`.
- `docs/ui-design.md:91-97` ("Color / theming") — note that the default
  surface stack is now overridable.

## Step 6 — Tests

- `aura-core`: unit tests above for `Theme`.
- `aura`: a smoke test that constructs `AuraView` with a non-default
  theme and verifies a render-relevant getter returns the override
  (e.g. `view.theme.colors.bg == 0x123456`). Full render snapshotting
  is out of scope — GPUI doesn't have a headless renderer.

## Explicit non-goals (per spec)

- Light theme — would require recomputing every contrast pair in
  `.design/components.md`.
- Per-tab / per-component overrides.
- In-app theme editor UI.
- TOML JSON schema file.

## Risk / gotchas

- Loose semantics on shared overlap: a user setting both
  `[[agents]] color = "..."` in `config.toml` and
  `[agents."Name"] accent = "..."` in `theme.toml` needs a deterministic
  winner. Spec says theme wins — verify with a test, and document in
  the `Theme::agent_accent` doc comment.
- `COLOR_WARNING` (`#e0a96d`, `app.rs:27`) isn't in
  `.design/customization.md`'s `[colors]` table. Add it there as part
  of this change so the spec stays canonical.
- The OAuth-token error message uses `0xff6b6b` directly via
  `render_quota` / `render_plugin_body` paths — the grep at step 2.4
  needs to cover both `render_body` (line 902) and
  `render_plugin_body` (line 962). Easy to miss one.

## Suggested PR shape

One PR, four commits:

1. `aura-core::theme` module + tests.
2. Wire into `AuraView`, delete consts, hoist literals.
3. Refresh path + modal action.
4. Docs.

Roughly 400–600 LOC net change, mostly mechanical const → field
substitutions.
