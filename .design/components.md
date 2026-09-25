# Components

Visual primitives, in render order. Each section names the renderer, lists
tokens it consumes, and enumerates state variants. State variants the code does
not yet handle are marked **proposed**.

## Header

**Renderer**: `AuraView::render_header` — `app.rs:224-296`.

Top bar holding the **profile pill row** on the left and the **action cluster**
(settings cog, "Aura ⟳" title/refresh) on the right.

- Bg: `COLOR_SURFACE`
- Border-bottom: `border_1` / `COLOR_BORDER`
- Padding: `px_4 py_3`
- Layout: `flex_row items_center justify_between`

**Proposed**: replace the static "Aura ⟳" text with `Aura {spinner?}`, where
the spinner appears whenever a refresh is in flight (see `loading.md`).

### Profile pill

**Renderer**: inline in `render_header`, `app.rs:243-265`.

A small clickable pill containing the agent icon + agent name.

| State      | Bg                  | Text                | Notes                                  |
| ---------- | ------------------- | ------------------- | -------------------------------------- |
| Idle       | `COLOR_SURFACE_HI`  | `COLOR_TEXT_DIM`    | Default for non-selected profiles.     |
| Active     | `COLOR_ACCENT_DIM`  | `COLOR_TEXT`        | Selected profile.                      |
| Loading    | (idle)              | (idle)              | **Proposed**: dim to 60% opacity while a profile switch is in flight. |
| Error      | (idle)              | (idle)              | **Proposed**: red dot prefix when the last fetch for this profile failed. |

- Padding: `px_2 py_1`
- Radius: `rounded_md`
- Type: `text_xs`
- Internal gap: `gap_1p5` (icon ↔ name)
- Icon: 14px SVG tinted with `agent_accent(kind)` (see `agents.md`)

### Action cluster

**Renderer**: inline, `app.rs:269-294`.

- Internal gap: `gap_3`
- Settings cog `⚙`: `COLOR_TEXT_DIM`, opens config via `xdg-open`/`$EDITOR`.
- Title `Aura ⟳`: `COLOR_ACCENT`. Clickable; triggers `refresh()`.

## Sponsor nudge

**Renderer**: `AuraView::render_sponsor_nudge` — `app.rs:1722`; dismiss
handler `AuraView::dismiss_sponsor_nudge` — `app.rs:912`.

One-time card between the header and the selector row, shown a week after the
first run until the user closes it with its × (gating:
`aura_core::sponsor`). Warm but calm: a faint accent *wash*, not a filled
banner, so it reads as information rather than an alert. Every color is
derived from theme tokens with `Theme::blend`, so custom `theme.toml` palettes
(light ones included) stay coherent — no literals.

- Strip: `px_4 py_2`, border-bottom `border_1` / `COLOR_BORDER`, `flex_shrink_0`
- Card: `flex_col`, padding `px_3 py_3`, gap `gap_2`, radius `rounded_md`,
  `border_1`
- Header row (`items_center`, `gap_2`): 14px `heart.svg` in `COLOR_ACCENT` ·
  title `text_sm` / `COLOR_TEXT`, `flex_1` (lexicon `sponsor_nudge_title`) ·
  trailing `icon_button` with `close.svg` (20×20 hit area, 14px icon)
- Body: `text_xs` / `COLOR_TEXT_DIM` (lexicon `sponsor_nudge`)
- Actions row: `flex_wrap`, `gap_2`, `mt_1`. Buttons are `px_2 py_1`,
  `rounded_md`, `text_xs`, icon 12px with `gap_1p5`
- Weight: regular (the app uses no font weights — see `tokens.md`); the
  title's emphasis comes from `text_sm` + `COLOR_TEXT` against the dim body.

| Element          | Bg / border (rest)                                                        | Text / icon                                             | Hover                                                                   |
| ---------------- | ------------------------------------------------------------------------- | ------------------------------------------------------- | ----------------------------------------------------------------------- |
| Card             | bg `blend(ACCENT, BG, 0.9)`, border `blend(ACCENT, BG, 0.7)`              | —                                                       | —                                                                       |
| × (dismiss)      | none                                                                      | icon `COLOR_TEXT_DIM`                                   | bg `blend(ACCENT, BG, 0.8)`                                             |
| Sponsor on GitHub| bg `blend(ACCENT, BG, 0.15)`                                              | `on_accent_text(ACCENT)`, leading `github.svg` same     | bg `COLOR_ACCENT` (brightens — never a downgrade)                       |
| Pix (BRL)        | none, border `blend(ACCENT, BG, 0.55)`                                    | `COLOR_TEXT`, trailing `arrow_up_right.svg` `TEXT_DIM`  | bg `blend(ACCENT, BG, 0.8)`, border `blend(ACCENT, BG, 0.3)`            |

| Click target      | Opens                          | Then                                   |
| ----------------- | ------------------------------ | -------------------------------------- |
| Sponsor on GitHub | `sponsor::SPONSOR_URL`         | card stays up                          |
| Pix (BRL)         | `sponsor::PIX_URL`             | card stays up                          |
| ×                 | nothing                        | `sponsor_nudge_done = true` (persisted)|

Notes:

- The sponsor buttons deliberately leave the card up: someone who paid via Pix
  may still want to set up a GitHub sponsorship, or vice versa. Only the ×
  retires it.

- The Pix label is plain text, not the 🇧🇷 flag: regional-indicator flags
  need a color-emoji font plus ligature shaping, which the monospace stack
  does not guarantee, so the flag could render as two boxed letters.
- No `cursor_pointer`: no other click target in the app sets one, so the card
  follows suit.
- The body does not quote a spend figure: Aura tracks tokens, not cost.

## Period row

**Renderer**: `AuraView::render_period_row` — `app.rs:298-336`.

Three period pills: All time / Last 7 days / Last 30 days.

- Bg: inherits `COLOR_BG`
- Border-bottom: `border_1` / `COLOR_BORDER`
- Padding: `px_4 py_2`
- Pill internal padding: `px_3 py_1`
- Pill radius: `rounded_md`, type: `text_xs`

| State  | Bg              | Text             |
| ------ | --------------- | ---------------- |
| Idle   | `COLOR_SURFACE` | `COLOR_TEXT_DIM` |
| Active | `COLOR_ACCENT`  | `#ffffff`        |

## Tab row

**Renderer**: `AuraView::render_tab_row` — `app.rs:338-375`.

Three tabs: Quota / Summary / Models. Active state is an underline, not a fill.

- Bg: `COLOR_SURFACE`
- Border-bottom: `border_1` / `COLOR_BORDER`
- Padding: `px_4 py_2`, internal gap: `gap_4`
- Type: `text_sm`

| State  | Text            | Underline                          |
| ------ | --------------- | ---------------------------------- |
| Idle   | `COLOR_TEXT_DIM`| none                               |
| Active | `COLOR_TEXT`    | `border_b_2` / `COLOR_ACCENT`      |

## Stat card

**Renderer**: `stat_card(label, value)` — `app.rs:648-671`.

Two-line card: dim label on top, primary-text value below. Used in the Summary
grid (`app.rs:638-644`).

- Bg: `COLOR_SURFACE`, border `COLOR_BORDER`, radius `rounded_md`
- Padding: `px_3 py_2`, internal gap: `gap_1`
- Label: `text_xs` / `COLOR_TEXT_DIM`
- Value: `text_sm` (root default) / `COLOR_TEXT`
- Layout: `flex_1` — fills its 2-col row equally.

| State    | Notes                                                       |
| -------- | ----------------------------------------------------------- |
| Idle     | Default rendering.                                          |
| Empty    | Value is `"—"` (em-dash). Same colors — no special styling. |
| Loading  | **Proposed**: skeleton — replace value with a 60%-width `COLOR_SURFACE_HI` block. |
| Error    | Stat-cards do not render on error; the body switches to the global error banner (`app.rs:378-388`). |

## Quota window card

**Renderer**: `render_quota_window` — `app.rs:493-560`.

Card showing one subscription window (e.g. "5h window", "weekly limit").
Stacks: title row → progress bar + percent label → "Resets ..." caption.

- Bg: `COLOR_SURFACE`, border `COLOR_BORDER`, radius `rounded_md`
- Padding: `px_3 py_3`, vertical gap: `gap_2`
- Title: `COLOR_TEXT` (default size)
- Reset caption: `text_xs` / `COLOR_TEXT_DIM`

### Progress bar (8px)

- Track: `h(8) flex_1 bg(COLOR_SURFACE_HI) rounded_md`
- Fill:  `h(8) w(relative(fraction)) bg(COLOR_ACCENT) rounded_md`
- Trailing label: `text_xs` / `COLOR_TEXT` — either `{pct}% used`, or
  `{tokens} tokens` when percent is unknown (fallback source).

| State        | Trailing label    | Notes                                                |
| ------------ | ----------------- | ---------------------------------------------------- |
| API-backed   | `"{n}% used"`     | `quota.source == Api`, real percentage available.    |
| Fallback     | `"{n} tokens"`    | `quota.source == Fallback`; bar shows 0% (unknown limit). Bottom-of-section note explains. |
| No data      | `"—"`             | Renders the empty-state callout instead of the card (`app.rs:454-470`). |
| Loading      | (no card)         | Body falls through to `render_loading` (`app.rs:438`). |

## Model row

**Renderer**: `render_model_row` — `app.rs:702-749`.

Per-model breakdown card: model name + `pct% · token count`, then a thin (4px)
progress bar.

- Same surface/border/radius/padding as the quota card but `py_2`.
- Progress fill: `COLOR_ACCENT`, track: `COLOR_SURFACE_HI`, height `px(4.0)`.
- Min visible width: clamped at 2% so tiny models still register (`app.rs:703`).

## Daily chart

**Renderer**: `render_daily_chart` — `app.rs:751-793`.

A column of `flex_1`-wide bars, bottom-aligned, in a 56px-tall row. Each bar
height = `(n / max) * 48`, floored at 2px so empty days still show.

- Bar bg: `COLOR_ACCENT`, radius `rounded_sm` (the only `rounded_sm` in the app)
- Container: same card chrome as stat-card (`COLOR_SURFACE` / `COLOR_BORDER` /
  `rounded_md` / `px_3 py_2`)
- Caption above: `text_xs` / `COLOR_TEXT_DIM` — `"Tokens per day"`

## Plugin section + panel

**Renderer**: `AuraView::render_plugins` — `app.rs:403-421`, panel:
`render_plugin_panel` — `app.rs:797-846`.

Optional bottom section. Hidden entirely when no plugins are configured.

- Section bg: `COLOR_SURFACE`, border-top `COLOR_BORDER`, padding `px_4 py_3`,
  vertical gap `gap_2`.
- Panel bg: `COLOR_BG` (intentionally darker than the section it sits in — the
  panel is a "callout from outside the app", reinforcing the plugin metaphor).
- Panel border: `COLOR_BORDER`, radius `rounded_md`, padding `px_3 py_2`.
- Title: `text_xs` / `COLOR_ACCENT`.
- Key/value rows: `flex_row justify_between`, both sides `text_xs`. Label is
  `COLOR_TEXT_DIM`; value is `COLOR_TEXT`, or `COLOR_ACCENT` when the plugin
  marks the line with `highlight = true`.

| State    | Rendering                                                              |
| -------- | ---------------------------------------------------------------------- |
| Idle     | Title + key/value rows.                                                |
| Loading  | **Proposed**: title with trailing spinner (see `loading.md`), no rows. |
| Error    | Title + single `text_xs` row in `#ff6b6b` containing `panel.error`. (`app.rs:815-821`) |

### Hint labels

**Renderer**: `AuraView::render_plugin_controls` (hint mode, `f`).

While hint mode is on, each button in a `controls` section gets a label
badge before its content, and a status line sits above the rows.

- Badge: `px_1`, `rounded_sm`, bg `COLOR_ACCENT`, text `COLOR_BG`, bold.
  Characters already typed render at 50% opacity before the rest.
- Buttons the typed characters rule out: whole pill at 35% opacity.
- Status line: `text_xs` / `COLOR_TEXT_DIM`.

### Plugin key panel

**Renderer**: `AuraView::render_leader_panel`.

Floating card for plugin shortcuts, drawn over the content at the bottom
right while leader mode is on (the plugin leader, default `space`, was
pressed).

- Position: `absolute`, `right 12px`, `bottom 12px`, width `210px`.
- Card: bg `COLOR_SURFACE`, border `COLOR_BORDER`, `rounded_md`, `p_2`,
  `gap_1`, `shadow_lg`, `text_xs`.
- Header: the leader and the keys typed so far as chips, then `…`;
  border-bottom `COLOR_BORDER`.
- Rows: a chip with the strokes still to press, then the label in
  `COLOR_TEXT_DIM`.
- Chip: `px_1`, `rounded_sm`, bg `COLOR_SURFACE_HI`, text `COLOR_TEXT`.
- Footer: "esc to cancel", `COLOR_TEXT_DIM` at 70% opacity.

| State   | Rendering                                                                 |
| ------- | ------------------------------------------------------------------------- |
| Leader  | Header, one row per key still reachable, footer.                          |
| Confirm | A key armed an action whose button isn't on screen: border and prompt in `COLOR_ERROR`, then "press … again · esc to cancel" in `COLOR_TEXT_DIM`. |

## Loading body

**Renderer**: `render_loading` — `app.rs:426-435`.

Centered "Loading…" string in `COLOR_TEXT_DIM`. Fills the body (`flex_1`).
**Proposed**: replace the trailing ellipsis with the spinner from `loading.md`
so the user sees motion rather than a static frame.

## Error body

**Renderer**: inline in `render_body` — `app.rs:378-388`.

Centered `#ff6b6b` text containing `self.error`. Same layout as loading body.
This is the **only** view that hides the tab content; the period/tab/header
rows still render so the user can switch profiles or refresh out of it.

## Modal (the widget itself)

Aura is not a desktop window in the traditional sense — it is a **modal
anchored near the taskbar**, shown on click of the tray icon and dismissed on
blur (see README). For design purposes, treat the entire `Render` output as
the modal contents. Sizing is governed by the GPUI window config, not by this
doc.

- Outer chrome: `bg(COLOR_BG)` + `text_color(COLOR_TEXT)` + `font_family("monospace")` + `text_sm` (`app.rs:209-212`).
- The modal has no drop shadow (see `tokens.md` — shadows are forbidden).
