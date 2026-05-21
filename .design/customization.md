# Customization

**Status**: not built yet. This file specifies the schema and behavior so the
implementer can land it without re-deciding the shape.

The goal: let users override **any** token from `tokens.md` and any agent's
accent color from `agents.md`, without recompiling. Defaults stay hard-coded as
`const COLOR_*` in `app.rs`; the theme file is a strict override layer on top.

## File location

```
~/.config/aura/theme.toml
```

Same parent directory as the existing `config.toml` (see README → Configuration).
Aura already reads `AppConfig` from `~/.config/aura/config.toml`; the theme
file is a sibling, loaded by an analogous `Theme::load(&theme_path)` returning
`Theme::default()` when the file is missing.

## Schema

All keys are optional. Missing keys fall through to the defaults baked into
`app.rs:16-27`. Colors are hex strings with a leading `#` (3- or 6-digit). Any
unknown key triggers a startup warning logged via `tracing::warn!` but does not
fail the load — forward compatibility matters more than strictness here.

```toml
# ~/.config/aura/theme.toml — every key is optional.

[colors]
# Neutrals
bg          = "#0e0e10"   # COLOR_BG
surface     = "#1a1a1f"   # COLOR_SURFACE
surface_hi  = "#252530"   # COLOR_SURFACE_HI
border      = "#2d2d36"   # COLOR_BORDER

# Text
text        = "#e6e6ee"   # COLOR_TEXT
text_dim    = "#8a8a9a"   # COLOR_TEXT_DIM

# Accent
accent      = "#8b5cf6"   # COLOR_ACCENT
accent_dim  = "#4c1d95"   # COLOR_ACCENT_DIM

# Status (currently inline literals; promote to tokens)
error       = "#ff6b6b"
on_accent   = "#ffffff"

# Per-agent accent fallback (when brand luminance > 0.85)
agent_fallback = "#b8b8c0"

[typography]
# Override the global monospace stack. Aura still forces monospace metrics;
# this is purely a font-face override.
font_family = "JetBrains Mono"

[spinner]
# See loading.md. Defaults to "braille".
style       = "braille"   # one of: "braille" | "dot"
color       = "#8b5cf6"   # defaults to colors.accent
interval_ms = 80          # frame interval

# Per-agent overrides. Keys match `AgentConfig.name` from config.toml exactly
# (including spaces and parentheses — quote them).
[agents."Claude Code (Personal)"]
accent = "#d97757"

[agents."Claude Code (Enterprise)"]
accent = "#0ea5e9"

[agents."Codex"]
# OpenAI's brand white washes out on the dark surface. The luminance-fallback
# rule from agents.md still applies to per-agent overrides — any color whose
# relative luminance exceeds 0.85 is silently replaced by `colors.agent_fallback`.
accent = "#ffffff"
```

## Precedence

For each rendered surface, the color is resolved as:

```
per-agent override  ->  [colors] override  ->  app.rs default const
                                ^
                       luminance fallback applied here when the resolved color
                       is the per-agent accent and trips > 0.85
```

The luminance fallback (`agents.md`) is enforced **after** the override layer,
not before. This means a user *cannot* opt into a pure-white accent that washes
out the UI even if they explicitly set it. If a power user really wants
unreadable contrast, they can also override `colors.agent_fallback` to the same
white — that takes two deliberate edits, which is the right friction level.

## Hot reload

Tie `theme.toml` into the same reload-on-refresh path that already covers
`config.toml` (`AuraView::refresh` at `app.rs:85-138`):

1. Reload `AppConfig` (today).
2. Reload `Theme` from `theme.toml`.
3. Re-run `refresh()`.

This means the existing "Aura ⟳" click already becomes a theme-reload trigger
with no extra UI work.

## Implementation notes

- Add `crates/aura/src/theme.rs` exporting a `Theme` struct mirroring the TOML
  schema, with `Theme::default()` returning the current `COLOR_*` constants.
- Replace direct `COLOR_*` references in `app.rs` with reads from
  `self.theme.colors.bg`, etc. (`u32` values, same `rgb(...)` wrapping).
- For per-agent lookups: `self.theme.agents.get(&agent.name).and_then(|a| a.accent)`
  with the `app.rs:25-27` constants as the final fallback.
- Validate hex strings via a tiny parser; reject 4- and 8-digit (alpha) hex
  with a `tracing::warn!` and fall through to the default. Alpha is not
  supported anywhere in the renderer today.
- Do **not** ship a TOML schema/JSON-schema file in v1; the documented example
  above is the spec. Add machine-readable schema only if the customization
  surface grows past ~20 keys.

## Out of scope (for v1)

- Light theme. The whole design is built around `COLOR_BG = #0e0e10`. A real
  light theme needs a separate pass on `components.md` for contrast pairs.
- Per-tab or per-component overrides (e.g. "make the daily chart bars red").
  If someone wants this badly enough, they can submit a PR — but adding 30+
  knobs preemptively is YAGNI.
- Live editing UI inside Aura. The settings cog opens `config.toml` today; in
  v1 of theming it should open `theme.toml` instead when shift-clicked, or
  both side-by-side when the user holds modifier keys. That's a design TODO,
  not a v1 requirement.
