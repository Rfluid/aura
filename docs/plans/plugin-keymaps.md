---
title: Plugin keymaps
status: implemented
version: 0.4.0
last_updated: 2026-09-25
last_verified: 2026-09-25
source_refs:
    - crates/aura-core/src/plugin/mod.rs
    - crates/aura-core/src/keymap/mod.rs
    - crates/aura-core/src/config.rs
    - crates/aura-core/src/config_schema.rs
    - crates/aura-ui/src/keys.rs
    - crates/aura-ui/src/hints.rs
    - crates/aura-ui/src/app.rs
    - crates/aura-cli/src/lib.rs
owner: "@rfluid"
tags: [plugins, keybindings, design]
---

# Plugin keymaps

## Problem

Plugins can show buttons (`controls` sections), and a click calls the plugin
back with `<cmd> action <id>`. From the keyboard, the only way to press a
button is [hint mode](../keybindings.md#hint-mode) (`f`). Hint mode works for
every plugin, but labels depend on where a button sits on the tab, so users
can't learn them. Actions a plugin wants on a fixed key, like "toggle mute",
have no way to get one.

## Scope

- A plugin declares keys on each section of its panel JSON. Each key maps to
  an action id and is active only while that section is on screen.
- Every plugin key sits behind a **leader** keystroke, `space` by default,
  set in `config.toml`. `space s` runs the plugin's `s` key.
- Pressing a plugin key does exactly what clicking the button with that id
  does, including the `confirm` two-press rule.
- Aura shows the keys: a badge on the matching button, a group in the `?`
  help overlay, and a strip listing the keys after the leader is pressed.

## Non-goals

- Plugins can't bind keys without the leader, and they can't override Aura's
  own shortcuts.
- No new way to call the plugin. Keys use the existing `action <id>` call.
- Aura doesn't remap plugin keys. `keybindings.toml` has no plugin table. A
  plugin that wants its keys remappable offers that itself, e.g. a settings
  section or its own config file, and emits the user's choice in `keys`.
- The leader does nothing outside plugin mode. Agent tabs get no leader strip.
- No keys declared in the sidecar TOML. Button ids change at runtime (e.g.
  `agent:Peh:off`) and a panel is re-sent after every action, so keys belong
  in the panel next to the buttons.

## Design

### Wire format

Each section gets an optional `keys` list. Old plugins leave it out and see
no change. A key a plugin wants on every section is repeated in each one;
the plugin builds its sections in code, so this costs it one helper call.

```json
{
  "title": "Audio Hooks",
  "sections": [
    {
      "id": "agents", "label": "Agents", "type": "controls",
      "keys": [
        { "keys": "s", "action": "mute:toggle", "label": "Toggle sound" }
      ],
      "controls": [ ... ]
    },
    {
      "id": "profiles", "label": "Profiles", "type": "controls",
      "keys": [
        { "keys": "s", "action": "mute:toggle", "label": "Toggle sound" },
        { "keys": "d", "action": "profile:rm:work", "label": "Delete work",
          "confirm": "Delete work?" }
      ],
      "controls": [ ... ]
    }
  ]
}
```

Keys work on every section type, not only `controls`. A `lines` or `table`
section can have keys for actions that have no button.

`PluginKey` fields:

| Field     | Type           | Default | Notes |
| --------- | -------------- | ------- | ----- |
| `keys`    | string         | —       | Keystrokes after the leader, in `keybindings.toml` syntax (`s`, `ctrl-x`, `d d`) |
| `action`  | string         | —       | Action id sent back as `action <id>`, the same as a button `id` |
| `label`   | string         | —       | Shown in the help overlay and the leader strip |
| `confirm` | string \| null | `null`  | Two-press confirm when no button carries it (see below) |

Rust side, in `aura-core/src/plugin/mod.rs`:

```rust
pub struct PluginSection {
    pub id: String,
    pub label: String,
    #[serde(default = "default_true")]
    pub uses_period: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub keys: Vec<PluginKey>,
    #[serde(flatten)]
    pub content: PluginContent,
}
```

### Leader config

```toml
# config.toml
[keybindings]
enabled = true
plugin_leader = "space"   # any keystroke(s); "none" turns plugin keys off
```

- `KeybindingsConfig.plugin_leader: String`, default `"space"`. Add a
  `config_schema` field so `aura config set keybindings.plugin_leader ctrl-p`
  works and is validated.
- The value is parsed with `keymap::parse_keys`. If it's invalid, Aura warns
  and falls back to `space`.
- Why `space`: no default binding uses it, so a plugin key can't collide with
  an Aura shortcut. It's also the leader vim and helix users already expect.
- If the user binds the leader itself in `[global]`, report it in
  `aura keys validate` and `aura doctor`. GPUI still waits for a second key,
  so both keep working, but the global binding fires only after the timeout.
  This is the same case as the existing "one binding is the start of a longer
  one" warning.

### Resolving keys (aura-core, no UI code)

`aura_core::plugin::keys::resolve(section, leader) ->
(Vec<PluginBinding>, Vec<PluginKeyWarning>)`:

- Joins the leader and `keys` into one sequence (`space s`) and makes it
  canonical with the keymap's parser. `S` and `shift-s` are treated as the
  same key.
- Warnings, which never fail the panel:
  - invalid keystroke → skipped
  - empty `action` → skipped
  - the same keys twice → the later one wins
  - one key is the start of a longer one (`d` and `d d`) → `d` fires after a
    short wait

Keeping this in core means `aura plugin run` can print the same warnings, and
it can be tested without GPUI.

### GPUI wiring (aura-ui)

- One action carries the id:

  ```rust
  #[derive(Clone, PartialEq, gpui::Action)]
  #[action(namespace = aura, no_json)]
  pub struct PluginKeyAction { pub action_id: SharedString }
  ```

- A new key context `plugin`. The root adds it next to `Aura` when all of
  these hold:
  - the modal is in plugin mode,
  - the panel has loaded without an error,
  - no overlay is open,
  - no action or refresh is running,
  - hint mode is off.
- `keys::install` takes the resolved plugin bindings too. It installs global,
  then plugin, then overlay, keeping overlay last.
- The bindings depend on the active plugin, section and panel. A single
  `sync_plugin_keys()` runs after a refresh, `apply_action_result`,
  `set_plugin`, `set_plugin_section` and `set_mode`. It reinstalls only when
  the resolved set has changed. `install` already clears and rebinds on every
  open and refresh, so this costs little.
- Alternative considered: install every plugin's keys once, with predicates
  like `plugin == p3 && section == s1`. That avoids reinstalls, but a panel
  still changes after each action, so it would need reinstalls anyway.

### Pressing a key

Refactor how a button fires so click, hint mode and plugin keys share one
path:

```rust
fn press_plugin_action(&mut self, id: String, confirm: bool, cx: &mut Context<Self>)
```

- `confirm` is true when a button on screen with this id has `confirm`, or
  when the key entry sets `confirm`.
- First press arms (`armed_action`), second press fires, and any other press
  disarms. This is today's click behavior.
- When the armed action has no button on screen, a one-line strip at the top
  of the plugin body shows the key's `confirm` text so the user knows a
  second press is needed.
- Add a step to Escape's order: clear the selection → disarm → close the
  overlay → dismiss.

### Showing the keys

- **Button badge.** If a key's `action` matches a button id on screen, the
  pill shows a dim key hint after its label (`␣s`).
- **Help overlay.** A "Plugin: \<name\>" group after Commands lists the keys
  of the section on screen, with their labels.
- **Leader strip.** After the leader is pressed, GPUI waits about a second for
  the next key. `cx.observe_pending_input` plus
  `window.pending_input_keystrokes()` tell us when the pending input equals
  the leader. While it does, a strip at the bottom of the body lists each key
  and its label, like a small which-key popup. The strip goes away when the
  sequence completes or times out.

### CLI

- `aura plugin run <name>` prints the resolved keys and any key warnings to
  stderr, next to the JSON.
- `aura keys validate` and `aura doctor` check `plugin_leader`. They don't run
  plugins.

## Implementation steps

1. **Core.** `PluginKey` and `PluginSection.keys` (serde round-trip tests),
   `KeybindingsConfig.plugin_leader` plus its schema field, and
   `plugin::keys::resolve` with warnings and tests. Add the leader collision
   check to the keymap warnings.
2. **UI plumbing.** Add `PluginKeyAction`, the `plugin` context in
   `keys::root_context`, a plugin-bindings parameter on `keys::install`, and
   `sync_plugin_keys()` wired into the five call sites.
3. **Press path.** Add `press_plugin_action` and move the click and hint-mode
   paths onto it. Add the confirm strip for buttons that aren't on screen, and
   the disarm step in Escape's order.
4. **Showing keys.** Button badges, the help overlay group, and the leader
   strip.
5. **CLI and docs.** Warnings in `aura plugin run`, the leader check in
   `aura keys validate` and `aura doctor`, and updates to `keybindings.md`,
   `plugin-authoring.md` and `configuration.md`.
6. **Adoption.** Add a `controls` section with keys to `plugins/hello` as the
   reference. Give audio-hooks keys for mute and for per-agent profile cycling
   (separate repo).

## Decisions

- **Keys are declared on each section**, not once for the panel. Scoping is
  then the section itself, and there's no `section` field to validate.
- **Remapping is the plugin's job.** Aura installs the keys the plugin emits,
  as they are. A plugin that wants remappable keys offers that itself and
  emits the user's choice.
- **The leader does nothing outside plugin mode.**


## Revision (2026-09-25)

After trying it:

- **No key badges on buttons.** The floating panel is where keys are shown.
- **The leader strip became a floating panel** at the bottom right of the
  window. It narrows as keys are typed and shows the strokes left to press.
- **Aura reads the strokes after the leader itself (leader mode).** Only
  the leader is a GPUI binding. GPUI drops a pending sequence after a fixed
  second, which closed the strip too early, and Escape during one replayed
  into `dismiss`. Leader mode waits until the shortcut completes or Escape,
  or for `[keybindings] leader_timeout_ms` when that's set (unset by
  default).
- **The shorter key wins.** When one key starts another, the shorter one
  runs as soon as it's typed. Waiting for a timeout makes no sense when the
  default is to wait forever.

## References

- [Keybindings](../keybindings.md): keymap contexts, hint mode
- [Plugin authoring](../plugin-authoring.md): `controls`, the `action` call
