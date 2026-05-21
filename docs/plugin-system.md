---
title: Plugin system
status: draft
version: 0.1.0
last_updated: 2026-05-21
last_verified: 2026-05-21
source_refs: []
owner: "@rfluid"
tags: [plugins, docs]
---

# Plugin system

## Overview

Plugins extend Aura with custom metrics panels displayed in the modal beneath the core usage stats. Any developer can author a plugin. Aura ships with one built-in plugin: **RTK Gains**.

## Plugin contract

A plugin is a program that:

1. Reads from whatever data source it owns (files, env vars, APIs)
2. Returns a structured panel payload when invoked by Aura
3. Has no write access to Aura's state or config

Aura calls plugins at modal open time and caches results for the modal's lifetime.

## Interface (subprocess + JSON IPC)

_Note: the loading strategy (dynamic library vs. subprocess) is a pending decision. This section describes the subprocess approach, which is the most portable and avoids ABI concerns._

Aura spawns the plugin binary with no arguments and reads a single JSON object from stdout:

```json
{
  "title": "RTK Gains",
  "lines": [
    { "label": "Tokens saved today",  "value": "1,247,832", "highlight": true },
    { "label": "Savings rate",         "value": "61%" },
    { "label": "Commands intercepted", "value": "342" }
  ],
  "error": null
}
```

If `error` is non-null, Aura shows the plugin panel with an error state and the error string.

Exit code: `0` on success, non-zero on fatal failure (panel hidden entirely).

Timeout: 500ms. Plugins that exceed this are shown in an error state.

## Plugin configuration

```toml
# ~/.config/aura/config.toml

[[plugins]]
name = "RTK Gains"
command = "aura-plugin-rtk"     # binary on $PATH, or absolute path

[[plugins]]
name = "My Custom Plugin"
command = "/usr/local/bin/my-aura-plugin"
```

## Built-in plugins

### RTK Gains

Source: `plugins/rtk-gains/`

Reads RTK's gain log (location: `~/.local/share/rtk/gains.json` — pending confirmation from RTK authors). Reports:
- Tokens saved today
- Tokens saved this month  
- Overall savings rate (%)
- Number of commands intercepted

## Authoring a plugin

1. Create a binary (any language) that writes the JSON panel payload to stdout
2. Handle the 500ms timeout — do not make network calls unless they're fast
3. Ship it on `$PATH` or document the absolute path for config
4. Register it in `~/.config/aura/config.toml` under `[[plugins]]`

A minimal plugin example (shell script):

```bash
#!/usr/bin/env bash
echo '{
  "title": "My Plugin",
  "lines": [
    { "label": "Status", "value": "OK" }
  ],
  "error": null
}'
```

## Future: plugin registry

A future version of Aura may ship a plugin registry (`aura plugin install <name>`) for discovering and installing community plugins. Out of scope for v0.1.
