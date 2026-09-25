---
title: CLI reference
status: current
version: 0.1.0
last_updated: 2026-09-24
last_verified: 2026-09-24
source_refs: ["crates/aura-cli/src/"]
owner: "@rfluid"
tags: [cli, docs]
---

# CLI reference

`aura` is primarily a tray app. Running the binary with no arguments
launches the tray; every other invocation is headless — it runs the
requested subcommand and exits without spinning up GPUI.

```text
aura [SUBCOMMAND] [OPTIONS]
```

Every read-only subcommand accepts `--format text|json`. `text` is the
default; `json` exists so dashboards, status bars, and `jq` pipelines can
consume aura's data without scraping human output.

## Top-level commands

| Command | Purpose |
|---|---|
| `aura` | Launch the tray (default). |
| `aura config <…>` | Manage `~/.config/aura/config.toml`. |
| `aura state <…>` | Inspect / modify `~/.local/share/aura/state.json`. |
| `aura theme <…>` | Inspect / seed `~/.config/aura/theme.toml`. |
| `aura keys <…>` | List, check and seed `~/.config/aura/keybindings.toml`. Alias: `aura keybindings`. |
| `aura agents list` | Configured profiles + detection status. |
| `aura plugin <…>` | Manage user plugins. Alias: `aura plugins`. |
| `aura usage` | Token-usage snapshot for an agent profile. |
| `aura quota` | Subscription rate-limit windows for an agent profile. |
| `aura doctor` | Resolved paths + detection diagnostics. |
| `aura completions <shell>` | Emit a shell completion script. |
| `aura update [args…]` | Update in place via the install/uninstall scripts. |
| `aura --version` | Print version. |
| `aura --help` | Print help; works on every subcommand too. |

The legacy spelling `aura setup-config` still works — it's a hidden alias
for `aura config setup`, kept so the existing installer scripts
(`install.sh`, `scripts/install.ps1`) don't break.

## Profile resolution

Commands that need to pick an agent (`usage`, `quota`, `plugin run`)
resolve in this order:

1. `--profile <name>` flag (case-insensitive name match).
2. `state.active_profile` — whatever the tray modal last selected.
3. The first entry in `config.agents`.

If none of those resolve, the command exits with an error suggesting
`aura config setup`.

## `aura config`

```text
aura config setup              # detect installed agents, write/update config.toml
aura config path               # print resolved config path
aura config show               # print loaded config (--format text|json)
aura config describe [<key>]   # list every field (type/default/docs), or explain one
                               #   (--format json emits the full schema)
aura config get <key>          # print a single field's current value
aura config set <key> <value>  # validate and set one field (e.g. set window.anchor top)
aura config wizard             # walk every field interactively; blank keeps current
aura config init [--force]     # write a fresh, fully-commented config.toml
aura config document           # rewrite the existing config in place with inline
                               #   docs, keeping every current value
aura config migrate [--check]  # rewrite an older section layout into the current
                               #   one; --check reports and exits 1 if pending
aura config edit               # open in $EDITOR (creates defaults if missing)
aura config validate           # parse-check
```

Keys are dotted paths into the `[window]` / `[tray]` / `[content]` / `[update]` /
`[keybindings]` / `[sponsor]` tables, e.g. `window.anchor`, `window.max_height`, `update.dismiss_all`. `set`
validates the value (rejecting bad enums/booleans and suggesting near-miss
keys); pass `none` to clear an optional field. The on-disk `config.toml` is
written with a `#` comment above each key, so the file documents itself. The
repeatable `[[agents]]` / `[[plugins]]` tables are documented by `describe` but
edited via `aura config edit`, `aura agents`, or `aura plugin`.

A key from an older layout still resolves: `aura config get display.anchor`
answers with `window.anchor` and prints a note naming the current key.
`aura config migrate` rewrites the file itself — see
[Migrating an older config](configuration.md#migrating-an-older-config).

## `aura state`

```text
aura state path
aura state show                     # --format text|json
aura state set-profile <name>       # validates against config.agents
aura state clear                    # removes active-profile selection
```

## `aura theme`

```text
aura theme path
aura theme edit                     # opens in $EDITOR; seeds defaults if missing
aura theme init [--force]           # writes the bundled defaults to disk
```

## `aura keys`

```text
aura keys path
aura keys list [--context global|overlay] [--format text|json]
aura keys describe [<action>|<keys>] [--format text|json]   # alias: aura keys actions
aura keys get <keys> [--context global|overlay] [--format text|json]
aura keys set <keys> <action> [--context …]     # `none` as the action unbinds
aura keys unbind <keys> [--context …]
aura keys reset <keys> | --action <name> | --all [--context …]
aura keys wizard [--context …]
aura keys merge <file|-> [--prefer theirs|ours] [--check] [--format text|json]
aura keys export                                # complete keymap as TOML on stdout
aura keys init [--force] [--full]               # starter file; --full = every default live
aura keys document [--force]                    # rewrite with inline docs
aura keys validate [--format text|json]         # exit 1 on problems
aura keys edit
```

Write commands edit `keybindings.toml` in place and keep your comments.
See [keybindings.md](keybindings.md#cli) for the full semantics.

## `aura agents`

```text
aura agents list                    # --format text|json
```

Shows each configured profile, its kind (`claude-code` / `codex` /
`gemini` / `antigravity`), the resolved `config_path`, and whether that
directory exists on disk.

## `aura plugin`

```text
aura plugin add <path> [--as NAME] [--link]
                       [--name LABEL] [--color #HEX] [--icon PATH]
aura plugin list                    # --format text|json
aura plugin remove <name>           # alias: rm
aura plugin dir                     # print user-plugins directory
aura plugin run <name> [--period all|7d|30d]
```

`plugin run` spawns the plugin once and prints its JSON payload — useful
for plugin authors who don't want to install + open the modal on every
build.

## `aura usage`

```text
aura usage [--profile <name>] [--period all|7d|30d] [--format text|json]
```

Reads the active profile's local data (Claude Code JSONL, Codex/Gemini
sessions, Antigravity's `conversation_summaries.db`) and prints a snapshot.

Antigravity publishes no token counts, so its snapshot reports
`Tokens: not reported by this agent` instead of a zero; sessions, messages,
active days, streaks and peak hour are all present.

## `aura quota`

```text
aura quota [--profile <name>] [--format text|json]
```

For `claude-code` profiles this hits `/api/oauth/usage` using the
credentials in `~/.claude/.credentials.json` (or Keychain / Credential
Manager). For `codex` / `gemini` it computes windows locally from session
data. For `antigravity` it runs `agy -p "/usage" --output-format json`,
which returns the same backend numbers the Antigravity IDE shows — four
windows (`Gemini · 5h`, `Gemini · week`, `Claude/GPT · 5h`,
`Claude/GPT · week`) with percentages but no token counts, because
Antigravity meters cost-weighted fractions rather than tokens.

The `source` field in the output (`"api"`, `"fallback"`, or
`"unavailable"`) tells you which path produced the numbers.

## `aura doctor`

```text
aura doctor                         # --format text|json
```

Pure local introspection — no network. Prints resolved paths, agent
detection results, plugin discovery, and the theme load result. The
first thing to ask someone to run when something looks wrong.

## `aura completions`

```text
aura completions bash > /etc/bash_completion.d/aura
aura completions zsh > ~/.zfunc/_aura
aura completions fish > ~/.config/fish/completions/aura.fish
aura completions powershell > _aura.ps1
aura completions elvish > aura.elv
```

## `aura update`

Update Aura in place by re-running the uninstall + install scripts.

```text
aura update                        # detect checkout vs. remote automatically
aura update --mode release         # forwarded args, e.g. install channel
aura update --version v1.2.3       # pin a release
```

Inside an Aura source checkout (cwd has `install.sh`, `uninstall.sh`, and
`Cargo.toml`) this runs the local `./uninstall.sh` then `./install.sh`,
rebuilding from source. Anywhere else it streams both scripts from GitHub
(the README's two-curl flow). Any trailing arguments are forwarded verbatim
to both scripts; `uninstall.sh` ignores install-only flags it doesn't
understand.
