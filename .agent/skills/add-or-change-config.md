---
title: Add or change a configuration field
status: current
version: 0.1.0
last_updated: 2026-05-30
last_verified: 2026-05-30
source_refs:
  - crates/aura-core/src/config.rs
  - crates/aura-core/src/config_schema.rs
  - crates/aura/src/runtime.rs
  - crates/aura/src/cli/config.rs
  - docs/configuration.md
owner: "@rfluid"
tags: [skill, config, cli]
---

# Add or change a configuration field

Use this skill when adding a new knob to `config.toml`, changing the type or
allowed values of an existing field, or wiring a config value into the running
app. Read [`docs/configuration.md`](../../docs/configuration.md) first — it
documents the five config layers and the precedence rules this procedure keeps
in sync. This skill is the *how-to-change-it*; that doc is the *what-it-is*.

The cardinal rule: **the typed struct is the source of truth, the field registry
mirrors it, and a test enforces they never drift.** If you add a struct field
without a registry descriptor, `registry_covers_every_field` fails the build —
that's by design, not an obstacle. Work *with* it.

## Where each layer lives

| Layer | File | You touch it when… |
|---|---|---|
| Typed struct | `crates/aura-core/src/config.rs` | always (the field itself) |
| Field registry | `crates/aura-core/src/config_schema.rs` | always (describe + get/set) |
| Runtime mirror | `crates/aura/src/runtime.rs` | the field must reach the tray loop *and* modal |
| Consumer | `crates/aura/src/app.rs`, `main.rs`, … | the field actually does something |
| CLI handler | `crates/aura/src/cli/config.rs` | almost never — it's registry-driven |
| Docs | `docs/configuration.md`, `docs/cli.md`, `README.md` | always |

The CLI (`describe` / `get` / `set` / `wizard` / `init` / `document`) and the
`#`-commented `config.toml` template are **all driven by the registry** — you do
not write per-command code for a new field. Add the descriptor and every surface
updates for free.

## Adding a new scalar field under `[display]` / `[update]`

1. **Add the struct field** in `config.rs` (on `DisplayConfig` or
   `UpdateConfig`). Give it a doc comment — it's the prose your registry
   `description` will quote. Update that struct's `Default` impl. Use
   `#[serde(default)]` on the field (or rely on the struct-level `#[serde(default)]`)
   so old configs without the key still parse. Optional fields are `Option<T>`.

2. **Add a `FieldDescriptor`** to `fields()` in `config_schema.rs`, in
   template-emission order (`display.*` before `update.*`). Fill every field:
   `key` (dotted), `type_label` (`string`/`string?`/`string[]`/`bool`/`u32?`),
   `allowed` (`&[]` for free-form), `default`, `summary` (one line, also the
   inline `#` comment), `description` (full prose), `example`.

3. **Wire `get_value` and `set_value`** (same file) — add a `match` arm for the
   new key in each. Reuse the parse helpers: `parse_enum`, `parse_bool`,
   `parse_opt_u32`, `parse_opt_string`, `parse_list`. Constrained values must go
   through `parse_enum` against the *same* `allowed` slice you put in the
   descriptor.

4. **Run the guards:**
   ```bash
   cargo test -p aura-core config_schema
   ```
   `registry_covers_every_field` proves the struct and registry agree;
   `get_and_set_round_trip_every_descriptor` proves your `example` actually sets;
   `render_commented_round_trips_*` proves the commented template still parses
   back to the same config.

5. **Consume the value.** A field that nothing reads is dead config. If only the
   modal needs it, read `config.display.<field>` in `app.rs`. **If both the tray
   poll loop (`main.rs`) and the modal need it**, mirror it through
   `runtime.rs`: add a `static AtomicBool`/etc., an accessor, and a line in
   `set_from_config`. Reapply any platform state there too (see the macOS
   activation-policy precedent). This is what keeps the background loop from
   drifting against a freshly-reloaded config.

6. **Document it.** Add a row to the field-reference table in
   `docs/configuration.md`, refresh that doc's `last_verified`, and update
   `docs/cli.md` / `README.md` if the surface changed. The doc's
   `registry_covers_every_field` note means the tables should always match
   `config describe`.

## Changing an existing field's allowed values or type

- Update the `allowed` slice (or `type_label`) on the descriptor **and** the
  matching `parse_enum`/parser call in `set_value` — they must list the same
  set, or `set` will accept/reject inconsistently with what `describe` shows.
- If you rename or remove a key, keep deserialization lenient: unrecognised
  values should fall back to a sane default rather than failing the parse (see
  how `anchor` treats the legacy `"auto"`). Never make an old on-disk config
  fail to load.
- Re-run the `config_schema` tests; fix the `example` if it no longer validates.

## Adding a field to a repeatable table (`[[agents]]` / `[[plugins]]`)

These are **not** `get`/`set` targets — they're managed via `aura agents` /
`aura plugin` / `config edit`.

1. Add the field to `AgentConfig` / `PluginConfig` in `config.rs` (`#[serde(default)]`
   for backward compatibility).
2. Add a `SectionField` to `agent_fields()` / `plugin_fields()` in
   `config_schema.rs` so `describe` and the template header document it.
3. Update the round-trip assertions in `render_commented_round_trips_populated`
   to cover the new field.
4. Document it in the relevant table in `docs/configuration.md`.

## Smoke test

```bash
cargo run -p aura -- config describe                 # new field listed?
cargo run -p aura -- config describe <key>           # full prose + current value
cargo run -p aura -- config set <key> <value>        # validation + near-miss keys
cargo run -p aura -- config get <key>
cargo run -p aura -- config init --force             # regenerate; confirm the # comment
cargo run -p aura -- config validate
```

For a runtime-mirrored field, also confirm the reload paths pick it up without a
restart: edit the value, click the tray icon (re-open), and check the behaviour
changed (config is reloaded on every open and on the modal Refresh button — see
the reload-triggers section of the configuration doc).

## Checklist

- [ ] Struct field + doc comment + `Default` updated (`config.rs`)
- [ ] `FieldDescriptor` / `SectionField` added (`config_schema.rs`)
- [ ] `get_value` + `set_value` arms wired (scalars only)
- [ ] `cargo test -p aura-core config_schema` green
- [ ] Value actually consumed (and mirrored via `runtime.rs` if dual-surface)
- [ ] `docs/configuration.md` table + `last_verified` updated; `cli.md` / `README.md` if needed
- [ ] Smoke-tested via the `config` CLI
