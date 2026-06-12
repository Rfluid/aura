---
title: Plugin authoring
status: stable
version: 0.2.0
last_updated: 2026-06-12
source_refs:
  - crates/aura-core/src/plugin/mod.rs
  - crates/aura-core/src/plugin/runner.rs
  - crates/aura-core/src/plugin/discovery.rs
  - plugins/hello/
owner: "@rfluid"
tags: [plugins, docs, how-to]
---

# Plugin authoring

Aura plugins are stand-alone executables. The host invokes them with a
period flag, reads JSON from stdout, and renders a panel in the modal.
There is no Rust-only ABI — write a plugin in any language that can
print JSON.

This guide covers:

1. The wire contract (CLI + JSON schema)
2. Building the reference plugin (`aura-plugin-hello`)
3. Installing your plugin into Aura

For the broader plugin-system rationale, see
[`plugin-system.md`](plugin-system.md).

## Wire contract

### Invocation

```bash
<your-binary> --period <all|7d|30d>
```

- The host always passes `--period`. Honour it if your data has a time
  dimension; otherwise mark the section as `uses_period = false` (see
  below) and the modal will hide the period pill row.
- If **no** section in your panel sets `uses_period = true`, the host
  reuses your previous output when the user switches periods instead of
  re-invoking the binary (a manual refresh still re-runs everything).
  Don't rely on being called once per period change.
- Stdout must be a single UTF-8 JSON object. Stderr is captured and
  surfaced as the error message on non-zero exit.
- The host enforces a **500 ms** budget per invocation. Cache or
  pre-aggregate anything that would push past that.
- Exit `0` for success (panel rendered), non-zero for failure (panel
  shows your stderr as the error message).

### JSON schema

The full type is `aura_core::plugin::PluginPanel`. Minimum required
fields:

```json
{
  "title": "My Plugin",
  "sections": [
    {
      "id": "overview",
      "label": "Overview",
      "type": "lines",
      "lines": [
        { "label": "Status", "value": "Running", "highlight": true }
      ]
    }
  ]
}
```

**Section content variants** (tagged by `type`):

| `type`     | Extra fields                | Renders as                         |
| ---------- | --------------------------- | ---------------------------------- |
| `lines`    | `lines: PluginLine[]`       | Key/value rows                     |
| `table`    | `headers`, `rows`           | Tabular with header row            |
| `text`     | `text: string`              | Preformatted text block            |
| `controls` | `controls: PluginControl[]` | Interactive button rows (see below)|

**`PluginLine`** fields:

| Field       | Type             | Default | Notes                                                    |
| ----------- | ---------------- | ------- | -------------------------------------------------------- |
| `label`     | string           | —       | Left column                                              |
| `value`     | string           | —       | Right column                                             |
| `highlight` | bool             | `false` | Bold + accent color                                      |
| `progress`  | number \| null   | `null`  | 0.0–1.0; draws a fill bar under the value                |

**`PluginRow`** fields (used in `table` sections):

| Field       | Type             | Default | Notes                                                    |
| ----------- | ---------------- | ------- | -------------------------------------------------------- |
| `cells`     | string[]         | —       | One cell per header                                      |
| `highlight` | bool             | `false` | Highlight the row                                        |
| `progress`  | number \| null   | `null`  | 0.0–1.0; trailing "Impact" bar                           |

**`PluginSection`** fields:

| Field         | Type    | Default | Notes                                          |
| ------------- | ------- | ------- | ---------------------------------------------- |
| `id`          | string  | —       | Stable identifier (preserved on tab switch)    |
| `label`       | string  | —       | Tab label                                      |
| `uses_period` | bool    | `true`  | Set `false` to hide the period pill row        |

### Interactive controls (aura ≥ 0.1.26)

A `controls` section makes a panel interactive. Each control is a row
with a label, an optional dim `hint` line, and a set of pill buttons:

```json
{
  "id": "agents",
  "label": "Agents",
  "uses_period": false,
  "type": "controls",
  "controls": [
    {
      "label": "Peh",
      "hint": "hooks: Stop, Notification",
      "buttons": [
        { "id": "agent:Peh:tags", "label": "tags", "active": true },
        { "id": "agent:Peh:off",  "label": "Off" },
        { "id": "hooks:Peh:remove", "label": "Remove", "danger": true }
      ]
    }
  ]
}
```

- `active: true` renders the pill in the accent color — use it to show
  the current selection of a mutually-exclusive group.
- `danger: true` renders the label in the error color — use it for
  destructive actions.
- `buttons` may be empty; the row then just shows label + hint.

When the user clicks a button, the host re-invokes your binary as:

```bash
<your-binary> action <id> --period <all|7d|30d>
```

Perform the operation, then print the **full refreshed panel JSON** on
stdout exactly as for a normal invocation (print `{"title": ...,
"error": "..."}` to surface a failure; the next refresh recovers).

Two differences from panel refreshes:

- The budget is **180 s**, not 500 ms — an action may legitimately block
  on user interaction, e.g. opening a `zenity` / `kdialog` file picker.
- While the action runs, the focus-loss auto-dismiss is suspended, so a
  dialog your plugin opens can take focus without closing the modal.

Action ids are opaque to the host: pick any encoding you like and parse
it yourself. Test actions headlessly with:

```bash
aura plugin run "My Plugin" --action "agent:Peh:off"
```

### Reporting errors

To show a friendly error in the panel without exiting non-zero:

```json
{ "title": "My Plugin", "error": "Could not reach metrics API" }
```

The host renders the error string in place of the panel body. Use this
for *expected* failure modes (offline, missing config). For
unexpected crashes, exit non-zero and let stderr carry the message.

## Reference plugin

The repository ships a complete example at
[`plugins/hello/`](../plugins/hello). Build it with:

```bash
cargo build -p aura-plugin-hello --release
```

The binary lands at `target/release/aura-plugin-hello`. Run it
directly to see the JSON it emits:

```bash
./target/release/aura-plugin-hello --period 7d
```

The example is built by `cargo build --workspace` but **not** installed
by `install.sh`. Users opt in via `aura plugin add` (below) or by
dropping the binary into the user plugins dir.

## Installing your plugin

There are three ways to register a plugin with Aura. Pick whichever
matches your use case.

### 1. `aura plugin add` (recommended)

Copy a built binary into `~/.config/aura/plugins/`:

```bash
aura plugin add ./target/release/aura-plugin-hello \
    --name "Hello" \
    --color "#22c55e"
```

Supported flags:

| Flag                 | Purpose                                              |
| -------------------- | ---------------------------------------------------- |
| `--as <filename>`    | Override the destination filename                    |
| `--link`             | Symlink instead of copy (Unix). Useful for dev loops |
| `--name <label>`     | Display name in the modal                            |
| `--color <#hex>`     | Accent color override                                |
| `--icon <path>`      | Embedded asset name, abs path, or `~/`-relative path |

Flags map 1:1 to the sidecar TOML keys; they're stored at
`<plugins-dir>/<binary>.toml` and persist across upgrades.

For active development, prefer `--link`: rebuilding the source updates
the live plugin in place without re-running `aura plugin add`.

### 2. Drop the binary in by hand

`~/.config/aura/plugins/` (or your OS equivalent — see below) is
scanned at every modal open. Any executable file in that directory
counts as a plugin.

```bash
mkdir -p ~/.config/aura/plugins
cp ./target/release/aura-plugin-hello ~/.config/aura/plugins/
chmod +x ~/.config/aura/plugins/aura-plugin-hello
```

Optional metadata sidecar (same dir, same basename + `.toml`):

```toml
# ~/.config/aura/plugins/aura-plugin-hello.toml
name  = "Hello"
color = "#22c55e"
icon  = "icons/blocks.svg"
```

Without a sidecar, the display name is derived from the binary's
filename (`aura-plugin-rtk-gains` → "Rtk Gains").

User plugins dir per OS:

| Platform | Path                                                |
| -------- | --------------------------------------------------- |
| Linux    | `~/.config/aura/plugins/`                           |
| macOS    | `~/Library/Application Support/aura/plugins/`       |
| Windows  | `%APPDATA%\aura\plugins\`                           |

### 3. Add to `config.toml` directly

The classic path. Use this when the plugin lives somewhere outside the
user plugins dir (e.g. `/usr/local/bin/...`):

```toml
# ~/.config/aura/config.toml

[[plugins]]
name    = "Hello"
command = "/usr/local/bin/aura-plugin-hello"
color   = "#22c55e"
```

`[[plugins]]` entries in `config.toml` always win over discovered
plugins with the same `name`, so you can also use this to override the
color or icon of a discovered plugin without removing the binary.

## Inspecting and removing

```bash
aura plugin list
# NAME                     SOURCE       COMMAND
# RTK Gains                config       aura-plugin-rtk
# Hello                    discovered   /home/me/.config/aura/plugins/aura-plugin-hello

aura plugin remove "Hello"
# Removed /home/me/.config/aura/plugins/aura-plugin-hello
```

`remove` only deletes plugins in the user plugins dir (the
"discovered" ones). Config-file entries must be removed by editing
`config.toml`.

Other useful subcommands for plugin authors:

```bash
aura plugin dir                    # print the user-plugins directory path
aura plugin list --format json     # machine-readable for scripts
aura plugin run "Hello" --period 7d
# pretty-prints the panel JSON your plugin emitted — no need to install
# and reopen the modal on every change. Combine with `--link` above for
# a tight edit/build/inspect loop.
```

## Checklist

Before publishing a plugin:

- [ ] Stays under the 500 ms budget on a cold cache
- [ ] Emits valid JSON on stdout (test with `jq .`)
- [ ] Honours `--period` if your data has a time axis
- [ ] Returns `{"error": "..."}` for expected failures rather than
      panicking
- [ ] Does not write to Aura's config or any other process's state
- [ ] Ships an `aura-plugin-<name>` binary name so the derived display
      name reads naturally
