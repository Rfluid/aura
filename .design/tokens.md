# Tokens

Canonical design tokens for Aura. These mirror the `const COLOR_*` declarations
in `crates/aura/src/app.rs:16-23` and the GPUI utilities used throughout the
render code. Do not invent new tokens without adding them here first.

## Colors

All colors are stored as `u32` literals (RGB, no alpha) and wrapped at the
call-site with `gpui::rgb(...)`. Source of truth: `crates/aura/src/app.rs:16-27`.

### Neutrals (surface stack)

| Name               | Hex       | Semantic role                                            | Used in                                                                                   |
| ------------------ | --------- | -------------------------------------------------------- | ----------------------------------------------------------------------------------------- |
| `COLOR_BG`         | `#0e0e10` | App background. Lowest layer, fills the window.          | `app.rs:209` (root), `app.rs:804` (plugin card)                                            |
| `COLOR_SURFACE`    | `#1a1a1f` | Card / header / tab-bar surface. One step above bg.      | `app.rs:234` (header), `app.rs:353` (tab row), `app.rs:415` (plugin section), stat-cards   |
| `COLOR_SURFACE_HI` | `#252530` | Elevated surface: inactive pills, progress-bar troughs.  | `app.rs:258` (inactive profile pill), `app.rs:537`, `app.rs:739`, `app.rs:771` (bar bg)    |
| `COLOR_BORDER`     | `#2d2d36` | Hairline separators and card outlines.                   | `app.rs:233`, `app.rs:312`, `app.rs:352`, `app.rs:414`, `app.rs:465`, `app.rs:520`, etc.   |

### Text

| Name             | Hex       | Semantic role                                       | Used in                                                                              |
| ---------------- | --------- | --------------------------------------------------- | ------------------------------------------------------------------------------------ |
| `COLOR_TEXT`     | `#e6e6ee` | Primary text. Numbers, model names, active labels.  | `app.rs:210` (root), `app.rs:523`, `app.rs:668`, `app.rs:721`                         |
| `COLOR_TEXT_DIM` | `#8a8a9a` | Secondary text. Labels, captions, inactive tabs.    | `app.rs:258`, `app.rs:277`, `app.rs:327`, `app.rs:367`, `app.rs:433`, `app.rs:449`, etc. |

### Accent

| Name                | Hex       | Semantic role                                                          | Used in                                                                            |
| ------------------- | --------- | ---------------------------------------------------------------------- | ---------------------------------------------------------------------------------- |
| `COLOR_ACCENT`      | `#8b5cf6` | Active state, progress fill, focus, "Aura" wordmark, plugin titles.    | `app.rs:286` (title), `app.rs:324` (active period), `app.rs:365` (tab underline), `app.rs:543`, `app.rs:745`, `app.rs:771`, `app.rs:811` |
| `COLOR_ACCENT_DIM` | `#4c1d95` | Muted accent for active *profile* pills (less shouty than `ACCENT`).   | `app.rs:255` (active profile pill bg)                                              |

### Status (unscoped, inline literals today)

These are used as raw `rgb(0x...)` literals in the renderer. **Proposed**: hoist
to `COLOR_ERROR` and `COLOR_ON_ACCENT` in a follow-up. Until then, document the
literal so we don't accidentally drift it.

| Name (proposed)    | Hex       | Semantic role                                | Used in                          |
| ------------------ | --------- | -------------------------------------------- | -------------------------------- |
| `COLOR_ERROR`      | `#ff6b6b` | Error text (body error banner, plugin error) | `app.rs:386`, `app.rs:819`       |
| `COLOR_ON_ACCENT`  | `#ffffff` | Text drawn on top of `COLOR_ACCENT` fill.    | `app.rs:324` (active period pill)|

### Per-agent brand (see `agents.md` for the full rule)

| Name           | Hex       | Semantic role                          | Used in        |
| -------------- | --------- | -------------------------------------- | -------------- |
| `COLOR_CLAUDE` | `#d97757` | Claude Code brand orange.              | `app.rs:854`   |
| `COLOR_OPENAI` | `#ffffff` | OpenAI / Codex brand white (problem!). | `app.rs:855`   |

> Pure white over `#0e0e10` washes out and reads as "missing color". See
> `agents.md` for the luminance-fallback rule that replaces it with `#b8b8c0`.

## Typography

Aura uses GPUI's Tailwind-shaped sizing utilities. Source: `app.rs:211-212`.

| Token      | GPUI call    | Size  | Role                                                          | Used in                                                                                            |
| ---------- | ------------ | ----- | ------------------------------------------------------------- | -------------------------------------------------------------------------------------------------- |
| `text_xs`  | `.text_xs()` | 12px  | Labels, captions, pills, percentages, plugin key/value rows.  | `app.rs:253`, `app.rs:322`, `app.rs:448`, `app.rs:468`, `app.rs:485`, `app.rs:549`, `app.rs:662`   |
| `text_sm`  | `.text_sm()` | 14px  | Default body size. Tab labels, card values.                   | `app.rs:212` (root default), `app.rs:360` (tab label)                                              |
| `text_lg`  | `.text_lg()` | 18px  | **Reserved.** Not currently used; available for headlines.    | —                                                                                                  |

**Font family**: `monospace` (system monospace stack), set globally at the root
in `app.rs:211`. Do not override per-element.

**Weight**: GPUI default (regular). Bold is not used; rely on `COLOR_TEXT` vs
`COLOR_TEXT_DIM` for emphasis.

## Spacing

Aura uses GPUI's 4px-step scale (Tailwind-like). The full set is available but
in practice the renderer sticks to these:

| Token       | px  | Role                                                            | Used in                                                                                  |
| ----------- | --- | --------------------------------------------------------------- | ---------------------------------------------------------------------------------------- |
| `gap_1`     | 4   | Card-internal label/value separation.                           | `app.rs:653`, `app.rs:707`                                                               |
| `gap_1p5`   | 6   | Icon-to-text gap inside profile pills.                          | `app.rs:249`                                                                             |
| `gap_2`     | 8   | Pill clusters, progress-bar row, card grids vertical.           | `app.rs:237`, `app.rs:308`, `app.rs:514`, `app.rs:635`, `app.rs:688`, `app.rs:779`       |
| `gap_3`     | 12  | Header action cluster, quota column.                            | `app.rs:273`, `app.rs:442`                                                               |
| `gap_4`     | 16  | Tab row, summary 2-col rows, models column.                     | `app.rs:347`, `app.rs:639`, `app.rs:676`                                                 |
| `px_2`/`py_1` | 8/4  | Pill horizontal padding / pill vertical padding.             | `app.rs:250-251`                                                                         |
| `px_3`/`py_2` | 12/8 | Inner card padding (stat-card, model row, plugin row).       | `app.rs:654-655`, `app.rs:709-710`, `app.rs:802-803`                                     |
| `px_3`/`py_3` | 12/12| Quota window inner padding.                                  | `app.rs:515-516`                                                                         |
| `px_4`/`py_3` | 16/12| Section padding (header, plugin section).                    | `app.rs:230-231`, `app.rs:412-413`, `app.rs:442` (px only), `app.rs:635` (px only)       |

Section padding is always `px_4` horizontally. Card padding is always `px_3`.
This two-step rhythm gives the widget its grid feel — do not break it.

## Radii

| Token       | GPUI call         | Role                                       | Used in                                                                            |
| ----------- | ----------------- | ------------------------------------------ | ---------------------------------------------------------------------------------- |
| `rounded_sm`| `.rounded_sm()`   | Daily chart bars only.                     | `app.rs:772`                                                                       |
| `rounded_md`| `.rounded_md()`   | Everything else with corners: pills, cards, progress bars. | `app.rs:252`, `app.rs:321`, `app.rs:464`, `app.rs:518`, `app.rs:538`, `app.rs:544`, `app.rs:657`, `app.rs:711`, `app.rs:740`, `app.rs:746`, `app.rs:783`, `app.rs:805` |

There is no `rounded_lg` / `rounded_full` in use. Keep it that way.

## Borders

Only two widths are used:

| Width        | Role                                          | Used in                                                                |
| ------------ | --------------------------------------------- | ---------------------------------------------------------------------- |
| `border_1`   | Card outlines + section separators (b/t).     | All cards, `app.rs:232` (header bottom), `app.rs:311`, `app.rs:351`, `app.rs:413` |
| `border_b_2` | Active tab underline.                         | `app.rs:364`                                                            |

Border color is always `COLOR_BORDER` except the active tab underline which is
`COLOR_ACCENT`.

## Shadows

**None.** Aura uses border + surface-elevation contrast to express depth, never
drop shadows. This is intentional: shadows on a near-black bg either disappear
or look like rendering artifacts. If you find yourself wanting a shadow, pick a
lighter surface (`COLOR_SURFACE` → `COLOR_SURFACE_HI`) instead.

## Progress-bar geometry

| Bar           | Height          | Track bg            | Fill bg          | Defined in        |
| ------------- | --------------- | ------------------- | ---------------- | ----------------- |
| Quota window  | `px(8.0)`       | `COLOR_SURFACE_HI`  | `COLOR_ACCENT`   | `app.rs:535-545`  |
| Model row     | `px(4.0)`       | `COLOR_SURFACE_HI`  | `COLOR_ACCENT`   | `app.rs:737-748`  |
| Daily chart   | up to `px(48.0)`, in a `px(56.0)` row | (no track — bars only) | `COLOR_ACCENT`   | `app.rs:760-773`  |
