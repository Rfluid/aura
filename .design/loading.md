# Loading

Every action that triggers a fetch must surface a spinner so the user knows
the widget is doing work. Without it, Aura looks frozen — `refresh()` is
synchronous today (`app.rs:85-138`) and can block on disk I/O or an HTTP call
to the Anthropic quota endpoint.

## When to show the spinner

Show it in the **header**, next to the "Aura" wordmark (`app.rs:283-291`),
whenever any of these is in flight:

| Trigger                  | Code path                                                        |
| ------------------------ | ---------------------------------------------------------------- |
| Initial load             | `AuraView::new` → `refresh()` (`app.rs:80`)                       |
| Manual refresh           | "Aura ⟳" click (`app.rs:288`)                                     |
| Profile switch           | `set_profile` → `refresh()` (`app.rs:180`)                        |
| Period switch            | `set_period` → `refresh()` (`app.rs:188`)                         |

The settings cog (which `xdg-open`s the config file) does **not** trigger a
spinner — it does not fetch.

The Quota/Summary/Models tab switch (`set_tab`, `app.rs:193-198`) also does
not trigger a fetch; it just re-renders cached `snapshot` / `quota` data. No
spinner.

## Where to show it

Replace the trailing `⟳` glyph in the header title with the spinner frame
whenever a fetch is in flight; otherwise keep `⟳` as the static
"click to refresh" affordance.

```
idle:    Aura ⟳
loading: Aura ⠋   (frame cycles)
```

Same color as the title (`COLOR_ACCENT`), same text size as the title (root
default `text_sm`). Vertical alignment: identical to the static `⟳` it
replaces — the spinner glyph is single-cell-monospace by construction (see
options below), so swapping them in place is geometrically clean.

Additionally, in the body:

- `render_loading` (`app.rs:426-435`) should append the same spinner frame
  after `"Loading"` (dropping the static ellipsis): `Loading ⠋`. Color:
  `COLOR_TEXT_DIM`.
- Plugin panel titles (`app.rs:807-813`) get a trailing spinner *only* when
  that specific plugin's `run()` is pending. Today plugins run synchronously
  inside `refresh()`; once they go async, wire this.

## Spinner options

Two candidates. Both are single-character, monospace-clean, and battle-tested
in terminal tooling.

### A) Braille (12-frame, recommended)

```rust
const SPINNER_BRAILLE: &[char] = &[
    '⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏',
    '⠋', '⠙', // intentionally repeat to keep cadence
];
// Actually use 10 unique frames + 2 repeats, or just 10 — both read fine.
// The canonical "dots" spinner from `cli-spinners` is:
const SPINNER_DOTS: &[char] = &[
    '⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏',
];
```

- Frame interval: **80ms** (12.5 fps). This matches the `cli-spinners`
  `dots` preset that ripgrep, cargo, and most modern Rust CLIs use, so it
  feels native to the terminal-style aesthetic.
- Glyph: Unicode braille pattern, fully covered by the system monospace stack
  and ships in any GPUI text run without a custom font.
- Memory: cycle index is a single `u8` on `AuraView` (`spinner_frame: u8`),
  advanced by a GPUI timer (`cx.spawn` + `Timer::after(Duration::from_millis(80))`)
  while a fetch is in flight.

### B) Rotating dot (4-frame, simpler)

```rust
const SPINNER_DOT: &[char] = &['◐', '◓', '◑', '◒'];
```

- Frame interval: **120ms** (~8.3 fps). Slower because there are fewer frames;
  any faster and it looks twitchy.
- Glyph: half-circle quadrants. Reads as a single dot spinning. Slightly more
  "playful" than braille; less terminal-native.
- Same wiring as braille — just a different frame array and interval.

## Recommendation

**Use braille (option A).** Reasons:

1. It matches the Zed-ish / terminal aesthetic that drives the rest of the
   design (see `README.md` → philosophy).
2. The 80ms cadence is the most-tested loading cadence on Earth; users have
   learned to read it as "the tool is working, not stuck".
3. 12 frames give enough variation that a 3+ second fetch still feels alive.
4. The dot quadrants render with a slight horizontal wobble in some monospace
   fonts because they are wider than a typical ASCII cell — braille avoids
   that.

## State on `AuraView`

Proposed fields to add:

```rust
pub struct AuraView {
    // ...existing fields...
    loading: bool,
    spinner_frame: u8,
}
```

- `loading = true` on entry to `refresh()`, `false` on exit.
- A GPUI task started in `new()` ticks `spinner_frame = (spinner_frame + 1) % FRAMES.len() as u8`
  every 80ms **only while `loading == true`**, then `cx.notify()` to repaint.
- When `loading == false`, the timer either parks (preferred — wake on
  `loading` transition) or runs without notifying. Don't burn frames repainting
  the same `⟳` glyph.

## Async refresh prerequisite

The current `refresh()` is fully synchronous, so today the spinner would never
actually render — the function returns before GPUI gets a chance to paint a
"loading" frame. Before wiring the spinner, move the snapshot + quota fetches
onto a background task with `cx.spawn(...)` and write results back into
`AuraView` via `cx.update`. Without that, the spinner is decorative at best.

This is a prerequisite, not a separate feature — schedule the two together.

## Proposed token

Add to `tokens.md` when this lands:

| Name             | Hex            | Semantic role                                  |
| ---------------- | -------------- | ---------------------------------------------- |
| `COLOR_SPINNER`  | `COLOR_ACCENT` | Spinner glyph in header title (alias today).   |

Aliasing to `COLOR_ACCENT` keeps the surface count low. Promote to a distinct
hex only if the spinner needs to differ from the accent (it shouldn't).
