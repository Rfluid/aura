---
title: Add or change a configuration field
status: current
version: 0.2.0
last_updated: 2026-09-16
last_verified: 2026-09-16
source_refs:
  - crates/aura-core/src/config.rs
  - crates/aura-core/src/config_migrate.rs
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
| Migration registry | `crates/aura-core/src/config_migrate.rs` | you move, rename, or remove an existing key |
| Runtime mirror | `crates/aura/src/runtime.rs` | the field must reach the tray loop *and* modal |
| Consumer | `crates/aura/src/app.rs`, `main.rs`, … | the field actually does something |
| CLI handler | `crates/aura/src/cli/config.rs` | almost never — it's registry-driven |
| Docs | `docs/configuration.md`, `docs/cli.md`, `README.md` | always |

The CLI (`describe` / `get` / `set` / `wizard` / `init` / `document`) and the
`#`-commented `config.toml` template are **all driven by the registry** — you do
not write per-command code for a new field. Add the descriptor and every surface
updates for free.

## Adding a new scalar field

1. **Add the struct field** in `config.rs`, on the sub-struct whose job it is
   (`WindowConfig`, `TrayConfig`, `ContentConfig`, `UpdateConfig`). Give it a
   doc comment — it's the prose your registry
   `description` will quote. Update that struct's `Default` impl. Use
   `#[serde(default)]` on the field (or rely on the struct-level `#[serde(default)]`)
   so old configs without the key still parse. Optional fields are `Option<T>`.

2. **Add a `FieldDescriptor`** to `fields()` in `config_schema.rs`, in
   template-emission order (`config_schema::SECTIONS`). Fill every field:
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
   modal needs it, read `config.<section>.<field>` in `app.rs`. **If both the tray
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
- Unrecognised *values* should fall back to a sane default rather than failing
  the parse (see how `anchor` treats the legacy `"auto"`). Never make an old
  on-disk config fail to load.
- Re-run the `config_schema` tests; fix the `example` if it no longer validates.

## Moving, renaming, or removing a key

A user's `config.toml` outlives any single release, so a key that moves needs a
migration or everyone's setting silently reverts to a default. Migrations live
in `config_migrate.rs` and are **declarative** — the same list drives the file
rewrite, the CLI's legacy-key aliases, and the report `doctor` prints. You do
not write procedural migration code.

1. **Append a `Migration`** to `migrations()`. Never reorder or edit a shipped
   entry — a user's config can be at any point in this history. Give it a
   stable `id` (`NNNN-short-slug`) and a one-line `summary`.

2. **List the `Step`s.** `Step::MoveKey { from, to }` for a move or rename,
   `Step::DropKey { key, why }` for a removal. Both are no-ops when the source
   is absent, which is what makes `normalize` idempotent — it runs on every
   load, so it *must* be.

3. **Rename the struct field and the descriptor** as usual (the sections
   above). Nothing else needs touching: `field()`, `get_value` and `set_value`
   all resolve through `config_migrate::resolve_key`, so the old spelling keeps
   answering and the CLI prints a note pointing at the new name.

4. **Add a round-trip test** in `config_migrate.rs` covering a realistic old
   config, and assert `normalize` is idempotent on its own output.

5. **Document the move** in the migration table under "Migrating an older
   config" in `docs/configuration.md`, and grep the tree for the old key —
   `README.md`, `install.sh`, `scripts/install.ps1`, and `docs/` all quote
   config keys in user-facing hints.

Note that a key Aura has no descriptor for is *reported* by `migrate` but not
carried over: `save` re-serializes from the parsed struct. That is pre-existing
`config set` behaviour, not something the migration introduces.

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

After a migration, also check an *old* config still lands correctly:

```bash
XDG_CONFIG_HOME=$(mktemp -d) # then drop an old-layout config.toml in $_/aura/
cargo run -p aura -- config migrate --check   # reports the moves, exits 1
cargo run -p aura -- config migrate           # rewrites; re-run to prove it is idempotent
cargo run -p aura -- doctor                   # surfaces a pending migration
```

For a runtime-mirrored field, also confirm the reload paths pick it up without a
restart: edit the value, click the tray icon (re-open), and check the behaviour
changed (config is reloaded on every open and on the modal Refresh button — see
the reload-triggers section of the configuration doc).

## Checklist

- [ ] Struct field + doc comment + `Default` updated (`config.rs`)
- [ ] `FieldDescriptor` / `SectionField` added (`config_schema.rs`)
- [ ] `Migration` appended (`config_migrate.rs`) if an existing key moved
- [ ] `get_value` + `set_value` arms wired (scalars only)
- [ ] `cargo test -p aura-core config_schema` green
- [ ] Value actually consumed (and mirrored via `runtime.rs` if dual-surface)
- [ ] `docs/configuration.md` table + `last_verified` updated; `cli.md` / `README.md` / installer hints if needed
- [ ] Smoke-tested via the `config` CLI
