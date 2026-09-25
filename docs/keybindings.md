---
title: Keybindings
status: current
version: 0.2.0
last_updated: 2026-09-25
last_verified: 2026-09-25
source_refs:
  - crates/aura-core/src/keymap/mod.rs
  - crates/aura-core/src/keymap/file.rs
  - crates/aura-ui/src/keys.rs
  - crates/aura-ui/src/hints.rs
  - crates/aura-core/src/plugin/keys.rs
  - crates/aura-ui/src/app.rs
  - crates/aura-ui/src/lib.rs
  - crates/aura-cli/src/keys.rs
owner: "@rfluid"
tags: [keybindings, configuration, docs]
---

# Keybindings

The Aura modal can be driven entirely from the keyboard. The default keys are
vim-style: `j`/`k` scroll, `h`/`l` switch sections, `q` closes the window and
`?` shows every shortcut. Arrow keys, Page Up/Down, Home/End and Tab work too.

Press `?` in the modal to see the live keymap, including anything you've
changed.

## Defaults

### Scroll

| Keys | Action | What it does |
|---|---|---|
| `j` · `down` | `scroll_down` | Scroll down one line |
| `k` · `up` | `scroll_up` | Scroll up one line |
| `ctrl-d` | `half_page_down` | Scroll down half a page |
| `ctrl-u` | `half_page_up` | Scroll up half a page |
| `ctrl-f` · `pagedown` | `page_down` | Scroll down a page |
| `ctrl-b` · `pageup` | `page_up` | Scroll up a page |
| `g g` · `home` | `scroll_top` | Jump to the top |
| `G` · `end` | `scroll_bottom` | Jump to the bottom |

While the help overlay is open, the scroll keys scroll the help list instead of
the body.

### Navigate

| Keys | Action | What it does |
|---|---|---|
| `l` · `tab` · `g t` | `next_section` | Next section tab (wraps around) |
| `h` · `shift-tab` · `g T` | `prev_section` | Previous section tab |
| `1` … `9` | `section_1` … `section_9` | Go to section N |
| `L` · `]` | `next_profile` | Next agent pill, or next plugin in plugin mode |
| `H` · `[` | `prev_profile` | Previous agent / plugin |
| `m` | `toggle_mode` | Switch between agents and plugins |
| `p` | `next_period` | Next period (all → 7d → 30d), when the section uses one |
| `P` | `prev_period` | Previous period |

### Commands

| Keys | Action | What it does |
|---|---|---|
| `r` · `f5` | `refresh` | Refresh (also reloads config, theme and keybindings) |
| `,` · `secondary-,` | `toggle_settings` | Open / close the settings panel |
| `.` | `toggle_more` | Open / close the more menu |
| `?` | `toggle_help` | Show / hide the shortcut list |
| `esc` (in an overlay) | `close_overlay` | Close the open menu, panel or help |
| `q` · `esc` | `dismiss` | Close the window |
| `e` | `open_config` | Edit `config.toml` |
| `t` | `open_theme` | Edit `theme.toml` |
| `u` | `open_update` | Open the update instructions (when an update is shown) |
| `U` | `dismiss_update` | Hide the update button (when shown) |
| `f` | `hint_mode` | Label a plugin's buttons so you can press them by key. See [Hint mode](#hint-mode) |
| — | `open_keybindings` | Edit `keybindings.toml` (unbound by default) |
| — | `quit` | Quit Aura, tray icon included (unbound by default) |

Escape works in layers. First it clears any selected text. If nothing is
selected, it closes the open overlay. If no overlay is open, it disarms a
plugin button waiting for its confirming press. Otherwise it closes the
window. `q` closes the window even when an overlay is open, but it also
disarms an armed button first.

### Hint mode

Plugin panels can have buttons (`controls` sections). To press one without
the mouse, press `f`. Every button on the tab gets a short label, such as `a`,
`s` or `d`. Type a label to press that button.

- Labels use the letters `asdfjklghqwertyuiopzxcvbnm`, home row first. Up to
  26 buttons get one letter each. Beyond that, every label is two letters.
- Buttons that no longer match what you've typed fade out. `backspace`
  deletes the last letter you typed. A letter that matches no label does
  nothing.
- A button that asks for confirmation (e.g. **Remove**) arms on the first
  press of its label and fires when you type the label again. Typing a
  different label disarms it.
- `esc` or a click leaves hint mode. So does switching tabs, plugins or
  mode, opening an overlay, or pressing a button.
- While hint mode is on, your other shortcuts are paused, so labels can
  reuse their letters.

`f` does nothing outside a plugin tab that has buttons.

### Plugin keys

A plugin can give its actions their own shortcuts. They all start with the
**leader**, `space` by default, so a plugin's `m` key is pressed as `space m`.
Aura's own shortcuts never start with the leader, so a plugin can't take one
of them over.

- Plugin keys work only while the plugin tab that declares them is on screen.
  Each tab (section) has its own keys.
- Press the leader: a panel floats at the bottom right of the window with
  the tab's keys. As you type, it narrows to the keys that still match and
  shows what's left to press. `backspace` takes back a key. A key that
  matches nothing is ignored.
- The panel waits until you finish a shortcut or press `esc`. To have it
  give up on its own, set `leader_timeout_ms` (below). Each key restarts
  the wait.
- If one key starts another (`d` and `d d`), the shorter one runs as soon
  as it's typed, so the longer one can never be pressed. `aura plugin run`
  warns about this.
- The `?` overlay lists the tab's keys under **Plugin: \<name\>**.
- A key for a button that asks for confirmation arms on the first press and
  fires on the second, like a click. If that button isn't on screen, the
  floating panel shows the prompt. `esc` disarms it.
- Change the leader in `config.toml`, or turn plugin keys off:

  ```toml
  [keybindings]
  plugin_leader = "ctrl-p"      # or "none"
  leader_timeout_ms = 3000      # unset (the default): wait until esc
  ```

  Or run `aura config set keybindings.plugin_leader ctrl-p`.
- Aura doesn't remap plugin keys. If a plugin lets you change its keys, it
  does so in its own settings.
- `aura keys validate` and `aura doctor` warn when the leader collides with a
  `[global]` binding, e.g. a leader of `g` next to `g g`. `aura plugin run
  <name>` prints each tab's keys and any problems with them on stderr.

## Customizing: `keybindings.toml`

Your bindings go in `~/.config/aura/keybindings.toml`, next to `config.toml`.
Aura layers them over the defaults. You can create the file in three ways:

- Run `aura keys init`.
- Run `aura keys edit`, which also opens it in `$EDITOR`.
- Choose **Settings → Keybindings → Edit keybindings.toml** in the modal.

The starter file explains the format and lists every default as a comment.

```toml
# Start from the built-in defaults (true) or from an empty keymap (false).
use_defaults = true

[global]
"ctrl-j" = "scroll_down"     # add a binding
"x"      = "refresh"         # bind another key to an action
"t"      = "none"            # remove a default
"g k"    = "open_keybindings"

# Checked before [global] while a menu, the settings panel or the help is open.
[overlay]
"q" = "close_overlay"        # make q close overlays instead of the window
```

- **Tables are contexts.** `[global]` applies everywhere. `[overlay]` applies
  while an overlay is open and takes precedence over `[global]` for the same
  keys. Global keys that the overlay table doesn't mention keep working.
- **Each entry maps keys to one action.** To give an action more keys, add more
  lines. Run `aura keys actions` to see every action name.
- **`"none"` unbinds.** In `[overlay]` it also blocks the `[global]` binding
  for those keys while an overlay is open.
- **`use_defaults = false`** starts from an empty keymap, so only your file
  applies.

### Keystroke syntax

- Write modifiers joined with `-`, then the key: `ctrl-d`, `alt-shift-x`,
  `cmd-k`. The modifiers are `ctrl`, `alt`, `shift`, `cmd`, `fn` and
  `secondary`. `secondary` means `cmd` on macOS and `ctrl` elsewhere, which
  helps when you share one file across machines.
- A space separates the strokes of a sequence, as in `g g`. After the first
  stroke, Aura waits about a second for the next one.
- A single uppercase letter means shift plus that letter, so `G` and `shift-g`
  are the same binding. Punctuation uses the character it types, e.g. `?`,
  `[`, `,`.
- Named keys are `escape` (or `esc`), `enter` (or `return`), `tab`, `space`,
  `backspace`, `delete`, `insert`, `home`, `end`, `pageup`, `pagedown`, `up`,
  `down`, `left`, `right`, and `f1`–`f24`.

### Warnings

A mistake in `keybindings.toml` never costs you your shortcuts. Aura skips the
bad entry, keeps everything else, and reports the problem in three places:

- `aura keys validate`, which exits 1 when there are problems (and also takes
  `--format json`).
- `aura doctor`.
- In the modal, a warning chip in the header shows the problem count. Click it
  or press `?` to see the full list at the top of the help overlay. Each warning
  is also printed once to stderr.

The checks:

| Problem | Example | What happens |
|---|---|---|
| File is not valid TOML | missing `]` | The whole file is ignored and the defaults apply |
| Unknown table | `[globl]` | The table is ignored; Aura suggests `global` |
| Unknown action | `"y" = "scrol_down"` | The entry is skipped; Aura suggests `scroll_down` |
| Invalid keystroke | `"hyper-x"`, `"ctrl-"`, `"f99"` | The entry is skipped |
| Value is not a string | `"z" = 3` | The entry is skipped |
| Same keystroke twice in one table | `"G"` and `"shift-g"` | The later one wins |
| Unbinding a key that has no binding | `"F9" = "none"` | No effect |
| One binding is the start of a longer one | `"g"` alongside `"g g"` | `g` still works, but only after a short wait |
| Binding a key text selection uses | `ctrl-c`, `ctrl-a`, `shift-up` | The binding works, but copy, select-all or selection extension on that key stops working |
| `use_defaults` is not a boolean | `use_defaults = "yes"` | The defaults are kept |

## Turning shortcuts off

Shortcuts are on by default. To turn them all off:

```toml
# config.toml
[keybindings]
enabled = false
```

You can also run `aura config set keybindings.enabled false`. With shortcuts
off, the modal is mouse-only, except that Escape still clears a selection or
closes the window.

## When changes apply

Aura reads `keybindings.toml` and `[keybindings] enabled` again every time the
window opens, and on every refresh (`r`, or the refresh button). You never need
to restart.

## CLI

`aura keys` works like `aura config`. Its read commands take
`--format text|json`. Its write commands edit `keybindings.toml` in place
without touching your comments, layout, or entries they can't parse. Every
write reports any warning it introduces.

| Command | What it does |
|---|---|
| `aura keys path` | Print the `keybindings.toml` path |
| `aura keys list [--context global\|overlay]` | Print the effective keymap, where each binding came from, and warnings |
| `aura keys describe` | List every action with its current keys. `*` marks actions you changed. Alias: `aura keys actions` |
| `aura keys describe <action>` | Explain one action: its default keys, its current keys, and how to bind or restore it |
| `aura keys describe <keys>` · `aura keys get <keys> [--context …]` | Show what a keystroke does and where that binding comes from. In the overlay context, a key the overlay doesn't bind shows its `[global]` binding |
| `aura keys set <keys> <action> [--context …]` | Bind the keystroke and save. Any other spelling of the same keystroke is replaced (`G` replaces `shift-g`) |
| `aura keys unbind <keys> [--context …]` | Remove the keystroke's binding, even if it's a default (writes `"none"`) |
| `aura keys reset <keys>` · `--action <name>` · `--all [--context …]` | Remove your overrides so the defaults apply again: for one keystroke, for one action, or for the whole file |
| `aura keys wizard [--context …]` | Step through every action, one prompt each: Enter keeps its keys, `j, ctrl-n` replaces them, `none` unbinds it, `default` restores it, and `stop` ends the wizard |
| `aura keys merge <file\|-> [--prefer theirs\|ours] [--check]` | Merge another keymap into yours. See [Merging](#merging) |
| `aura keys export` | Print the effective keymap as a complete file: `use_defaults = false`, with every binding written out |
| `aura keys init [--force] [--full]` | Write a starter file with every default as a comment. `--full` writes every default as a live binding instead, with `use_defaults = false` |
| `aura keys document [--force]` | Rewrite the file in the generated layout: your entries in order, a description on each, and the defaults as a commented reference |
| `aura keys validate` | Report problems; exit 1 if there are any |
| `aura keys edit` | Open the file in `$EDITOR`, creating it first if missing, then report any warnings |

`aura keybindings` is an alias for `aura keys`.

### Examples

```console
$ aura keys set ctrl-j scroll_down
set [global] "ctrl-j" = "scroll_down"

$ aura keys set r toggle_help
set [global] "r" = "toggle_help"   (was refresh)

$ aura keys set x scrol_down
Error: unknown action `scrol_down` (did you mean `scroll_down`?) …

$ aura keys get j --context overlay
[overlay] j → scroll_down   (default, from [global])

$ aura keys reset --action toggle_help
Removed 1 entry from …/keybindings.toml; the defaults apply there again.
```

When the wizard changes an action's keys, it keeps the change as small as
possible:

- A default key the action loses becomes `"none"`.
- Any other key it loses has its entry deleted, so that key's own default, if
  it has one, applies again. The wizard notes when this happens.
- A key taken from another action is noted too.

### Merging

`merge` compares entries by keystroke, so `G` and `shift-g` count as the same
entry.

| Case | What happens |
|---|---|
| New keystroke | Added |
| Same action in both files | Left alone |
| Different action | The incoming file wins (`--prefer theirs`, the default), or yours is kept and the conflict is reported (`--prefer ours`) |
| Broken entry in the incoming file | Listed and never copied |
| `use_defaults` differs | Follows the same `--prefer` rule |

`--check` prints the plan and exits 1 when the merge would change anything.
`-` reads the incoming file from stdin. `--format json` prints the report.

### `document` and your comments

`document` rebuilds the whole file, so it can't keep your comments. It keeps
every valid entry. If any entry can't be carried over, it lists them and stops
unless you pass `--force`. Every other write command keeps the file as you
wrote it.

## How it works

- `aura-core/src/keymap/mod.rs` has no UI code. It holds the action catalogue, the
  defaults table, the keystroke parser, the merge of defaults and user file,
  and every warning check. The CLI and the modal share it. It reads the file
  with `toml_edit`, which keeps file order, so warnings list entries top to
  bottom and "the later one wins" means the entry lower in the file.
- `aura-core/src/keymap/file.rs` (`KeymapFile`) is the editing side. It makes
  format-preserving edits through `toml_edit`. The CLI commands and their
  tests all build on its operations: `bind`, `remove`, `clear`,
  `set_action_keys`, `restore_action`, `merge` and `document`.
- `aura/src/keys.rs` defines one GPUI action per `KeyAction`. The mapping is an
  exhaustive `match`, so an unwired action won't compile. It also installs the
  resolved bindings:
  - `[global]` uses key context `Aura`, and `[overlay]` uses `overlay`.
  - The modal's root element carries both contexts. Because the two sit on the
    same element, GPUI can't break ties by depth and uses insertion order
    instead, so overlay bindings are installed last.
  - An unbind becomes GPUI's `NoAction`, which also masks a lower context.
- The root element keeps a `FocusHandle` focused for the window's lifetime,
  because GPUI dispatches key bindings from the focused element. No other
  element in the modal is focusable, and a click anywhere puts focus back on
  the root.
- Hint mode (`aura-ui/src/hints.rs`) removes the `Aura` context from the root
  element while it runs, so no binding matches. Typed letters then reach the
  root's `key_down` listener, which matches them against the labels. Labels
  follow the order buttons render in, and all labels have the same length,
  so none is a prefix of another.
- Plugin keys (`aura-core/src/plugin/keys.rs`) are resolved per section:
  the leader goes in front, spellings are made canonical, and duplicates or
  keys that start other keys are reported. Only the leader becomes a GPUI
  binding, in a third context, `plugin`, between global and overlay. The
  root carries `plugin` only while the section on screen has keys and no
  overlay, hint mode, refresh or action is running. The leader opens leader
  mode, which works like hint mode: the root drops its keymap contexts and
  its `key_down` listener matches the typed strokes against the section's
  keys with GPUI's own `KeyBinding::match_keystrokes`. Aura reads the
  strokes itself rather than binding whole sequences because GPUI forgets a
  pending sequence after a fixed second, and Escape during one would
  replay into `dismiss`. The leader is reinstalled whenever the plugin,
  section, mode or panel changes.
- When shortcuts are on, Escape is an ordinary binding. The fallback Escape
  observer in `main.rs` only runs when they're off.
