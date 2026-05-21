# Agents

Each agent profile has a **brand color** used to tint its icon in the profile
pill row. The brand color is also reserved for *per-agent accenting* when that
profile is the active one — active pills, progress-bar fills, the tab underline
— though that wiring is not yet built (it is the next step after the
luminance-fallback below lands).

Source: `crates/aura/src/app.rs:25-27` and `app.rs:848-862`.

## Canonical brand colors

| Agent kind            | Hex       | Why                                              | Used in       |
| --------------------- | --------- | ------------------------------------------------ | ------------- |
| `AgentKind::ClaudeCode` | `#d97757` | Anthropic's published Claude orange.            | `app.rs:854`  |
| `AgentKind::Codex`      | `#ffffff` | OpenAI's pure-white mark — **needs fallback**.  | `app.rs:855`  |

## The luminance fallback rule

Pure white (`#ffffff`) over `COLOR_BG` (`#0e0e10`) creates a max-contrast blob
that reads as "missing color" rather than a brand mark. Any brand color whose
**relative luminance exceeds `0.85`** must be replaced with a neutral light
grey before it hits the renderer.

**Fallback color**: `#b8b8c0` — sits between `COLOR_TEXT` (`#e6e6ee`) and
`COLOR_TEXT_DIM` (`#8a8a9a`), preserving the "white-ish" reading without
torching the eye. This is a **new token**; add it to `tokens.md` if/when you
wire the helper below.

### Luminance formula

Use the WCAG relative-luminance formula on the linearized sRGB channels:

```
L = 0.2126 * R_lin + 0.7152 * G_lin + 0.0722 * B_lin
```

Where each `_lin` channel is the gamma-decoded `[0.0, 1.0]` value. A simpler
approximation that is good enough for this single threshold check is the
straight average of normalized channels (`(R + G + B) / 3 / 255 > 0.85`); the
proper formula is preferred because it correctly rejects e.g. yellow but admits
saturated-but-dim brand greens.

## Helper signature

Add to `crates/aura/src/app.rs` (or a new `crates/aura/src/theme.rs` once the
file grows past comfort):

```rust
/// Returns the accent color to use for an agent kind. Pure-white-ish brand
/// colors are substituted with a neutral light grey (`#b8b8c0`) so they read
/// against `COLOR_BG` instead of washing out.
fn agent_accent(kind: AgentKind) -> u32 {
    let brand = match kind {
        AgentKind::ClaudeCode => COLOR_CLAUDE, // 0xd97757
        AgentKind::Codex      => COLOR_OPENAI, // 0xffffff
    };
    if relative_luminance(brand) > 0.85 {
        0xb8b8c0
    } else {
        brand
    }
}

fn relative_luminance(rgb: u32) -> f32 {
    let r = ((rgb >> 16) & 0xff) as f32 / 255.0;
    let g = ((rgb >> 8) & 0xff) as f32 / 255.0;
    let b = (rgb & 0xff) as f32 / 255.0;
    let lin = |c: f32| if c <= 0.03928 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) };
    0.2126 * lin(r) + 0.7152 * lin(g) + 0.0722 * lin(b)
}
```

The current `agent_icon` (`app.rs:852-862`) reads `COLOR_CLAUDE` / `COLOR_OPENAI`
directly. Once `agent_accent` lands, call it from `agent_icon` and from the
active-pill / progress-bar renderers when those switch to per-agent theming.

## Per-agent accenting (planned, not built)

When agent `A` is the active profile:

| Surface                  | Today           | Proposed (per-agent)            |
| ------------------------ | --------------- | ------------------------------- |
| Active profile pill bg   | `COLOR_ACCENT_DIM` | `agent_accent(A.kind)` at 25% mix into `COLOR_SURFACE` |
| Active period pill bg    | `COLOR_ACCENT`     | `agent_accent(A.kind)`          |
| Active tab underline     | `COLOR_ACCENT`     | `agent_accent(A.kind)`          |
| Progress-bar fill        | `COLOR_ACCENT`     | `agent_accent(A.kind)`          |
| Daily chart bars         | `COLOR_ACCENT`     | `agent_accent(A.kind)`          |
| Plugin panel title       | `COLOR_ACCENT`     | stays `COLOR_ACCENT` (plugins are agent-agnostic) |

The `COLOR_ACCENT` violet remains the **fallback accent** for any UI where no
agent is selected (e.g. error state, no profiles configured).

## Adding a new agent kind

When you add a variant to `AgentKind`:

1. Add a `COLOR_<NAME>` constant near `app.rs:26`.
2. Add the icon SVG to `crates/aura/icons/<name>.svg`.
3. Extend the match in `agent_icon` (`app.rs:853`).
4. Update the **canonical brand colors** table above.
5. If the brand color trips the `> 0.85` luminance threshold, no extra work —
   `agent_accent` handles it. Note it in the table anyway.
