//! Config normalization — the one place that knows how older `config.toml`
//! layouts map onto the current one.
//!
//! Aura's config is a user-edited file that long outlives any single release,
//! so reorganizing a section can't mean "everyone's settings silently revert
//! to defaults". This module keeps a registry of [`Migration`]s, each a small
//! declarative list of [`Step`]s over the *raw* TOML table, applied before
//! serde ever sees it.
//!
//! Declarative rather than a hand-written `fn(&mut Table)` per migration
//! because the same list answers two different questions:
//!
//! 1. "Rewrite this file into the current shape" — [`normalize`], used by
//!    [`crate::config::AppConfig::load`] (in memory, every launch) and by
//!    `aura config migrate` (written back to disk).
//! 2. "Where did this key go?" — [`resolve_key`], which chases a moved key
//!    through every later migration so `aura config get display.anchor` still
//!    answers, and `config_schema` can accept legacy key names.
//!
//! Adding a migration is adding one `Migration` to [`migrations`]. Nothing
//! else needs to change: the CLI, the alias map, and the report all read off
//! the same list.
//!
//! Migrations must be idempotent — [`normalize`] runs on every load, and a
//! `MoveKey` whose source is absent is a no-op.

use anyhow::{Context, Result};
use std::path::Path;
use toml::{Table, Value};

// ── Registry ─────────────────────────────────────────────────────────────────

/// One mechanical rewrite applied to a raw config table.
#[derive(Debug, Clone, Copy)]
pub struct Migration {
    /// Stable identifier, ordered by prefix. Never reused or renamed — it is
    /// what a report names when explaining what changed.
    pub id: &'static str,
    /// One line describing the reorganization, shown by `aura config migrate`.
    pub summary: &'static str,
    /// Applied in order. Idempotent: each is a no-op when its source is gone.
    pub steps: &'static [Step],
}

/// A single key-level edit. Both variants are no-ops when the source key is
/// absent, which is what makes re-running a migration safe.
#[derive(Debug, Clone, Copy)]
pub enum Step {
    /// Move a dotted key to another dotted key, creating the destination
    /// table if needed. Skipped when the destination already holds a value —
    /// a config that was hand-edited into the new shape wins over the old key.
    MoveKey {
        from: &'static str,
        to: &'static str,
    },
    /// Delete a key that no longer means anything.
    DropKey {
        key: &'static str,
        /// Why it went away, surfaced in the migration report.
        why: &'static str,
    },
}

/// Every migration, oldest first. Append here; never reorder or edit a
/// shipped entry — a user's config may be at any point in this history.
pub fn migrations() -> &'static [Migration] {
    &[Migration {
        id: "0001-split-display",
        summary: "Split [display] into [window] (placement and size), [tray] \
                  (the indicator) and [content] (what the modal renders).",
        steps: &[
            // Placement, size, chrome — what [display] is now narrowed to.
            Step::MoveKey {
                from: "display.anchor",
                to: "window.anchor",
            },
            Step::MoveKey {
                from: "display.linux_backend",
                to: "window.linux_backend",
            },
            Step::MoveKey {
                from: "display.show_in_app_switcher",
                to: "window.show_in_app_switcher",
            },
            Step::MoveKey {
                from: "display.dismiss_on_focus_loss",
                to: "window.dismiss_on_focus_loss",
            },
            // `window_chrome` loses its prefix now the section supplies it.
            Step::MoveKey {
                from: "display.window_chrome",
                to: "window.chrome",
            },
            Step::MoveKey {
                from: "display.auto_resize",
                to: "window.auto_resize",
            },
            Step::MoveKey {
                from: "display.max_height",
                to: "window.max_height",
            },
            // The tray indicator, which never touched the modal.
            Step::MoveKey {
                from: "display.tray_status",
                to: "tray.indicator",
            },
            Step::MoveKey {
                from: "display.tray_progress",
                to: "tray.progress",
            },
            Step::MoveKey {
                from: "display.tray_color",
                to: "tray.color",
            },
            Step::MoveKey {
                from: "display.tray_pulse",
                to: "tray.pulse",
            },
            Step::MoveKey {
                from: "display.tray_status_interval_secs",
                to: "tray.refresh_secs",
            },
            // What the modal renders, as opposed to where it sits.
            Step::MoveKey {
                from: "display.default_period",
                to: "content.default_period",
            },
            Step::MoveKey {
                from: "display.plugin_order",
                to: "content.plugin_order",
            },
            Step::MoveKey {
                from: "display.goblin_mode",
                to: "content.goblin_mode",
            },
        ],
    }]
}

// ── Report ───────────────────────────────────────────────────────────────────

/// What one step actually did to a given config.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Change {
    /// `from` was carried over to `to`.
    Moved {
        migration: &'static str,
        from: &'static str,
        to: &'static str,
    },
    /// `from` was discarded because `to` already had a value.
    Superseded {
        migration: &'static str,
        from: &'static str,
        to: &'static str,
    },
    /// `key` was deleted.
    Dropped {
        migration: &'static str,
        key: &'static str,
        why: &'static str,
    },
    /// A section emptied by the migration was removed.
    SectionRemoved {
        migration: &'static str,
        section: &'static str,
    },
    /// A key left behind in a migrated-away section that Aura has no
    /// descriptor for, so the migration does not know where to put it. It is
    /// left alone in the table, but a rewrite through the commented renderer
    /// re-serializes from the struct and will not carry it over.
    Unrecognized { section: &'static str, key: String },
}

impl std::fmt::Display for Change {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Change::Moved { from, to, .. } => write!(f, "{from} → {to}"),
            Change::Superseded { from, to, .. } => {
                write!(f, "{from} dropped ({to} is already set)")
            }
            Change::Dropped { key, why, .. } => write!(f, "{key} removed — {why}"),
            Change::SectionRemoved { section, .. } => write!(f, "[{section}] removed (now empty)"),
            Change::Unrecognized { section, key } => {
                write!(f, "[{section}] {key} — not a key Aura knows")
            }
        }
    }
}

/// Everything [`normalize`] did to one config table.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MigrationReport {
    pub changes: Vec<Change>,
}

impl MigrationReport {
    /// True when the config was already in the current shape.
    pub fn is_empty(&self) -> bool {
        self.changes.is_empty()
    }

    /// True when nothing was actually rewritten. An `Unrecognized` entry is a
    /// note, not an edit, so a report holding only those means the config is
    /// already current.
    pub fn is_noop(&self) -> bool {
        !self
            .changes
            .iter()
            .any(|c| !matches!(c, Change::Unrecognized { .. }))
    }
}

// ── Apply ────────────────────────────────────────────────────────────────────

/// Rewrite `table` into the current config shape, running every migration in
/// order. Idempotent — running it on an already-current config reports
/// nothing and changes nothing.
pub fn normalize(table: &mut Table) -> MigrationReport {
    let mut report = MigrationReport::default();
    for migration in migrations() {
        for step in migration.steps {
            match *step {
                Step::MoveKey { from, to } => move_key(table, migration.id, from, to, &mut report),
                Step::DropKey { key, why } => {
                    if take(table, key).is_some() {
                        report.changes.push(Change::Dropped {
                            migration: migration.id,
                            key,
                            why,
                        });
                    }
                }
            }
        }
        prune_emptied_sections(table, migration, &mut report);
    }
    report
}

/// Parse `content` and normalize it in one step.
pub fn normalize_str(content: &str) -> Result<(Table, MigrationReport)> {
    let mut table: Table = toml::from_str(content).context("parse config as TOML")?;
    let report = normalize(&mut table);
    Ok((table, report))
}

/// Report what [`normalize`] *would* do to the config at `path`, without
/// writing anything. A missing file reports no changes.
pub fn check_file(path: &Path) -> Result<MigrationReport> {
    if !path.exists() {
        return Ok(MigrationReport::default());
    }
    let content =
        std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    let (_, report) = normalize_str(&content)?;
    Ok(report)
}

fn move_key(
    table: &mut Table,
    migration: &'static str,
    from: &'static str,
    to: &'static str,
    report: &mut MigrationReport,
) {
    let Some(value) = take(table, from) else {
        return;
    };
    if get(table, to).is_some() {
        // Someone already wrote the new key by hand; the stale one loses.
        report.changes.push(Change::Superseded {
            migration,
            from,
            to,
        });
        return;
    }
    put(table, to, value);
    report.changes.push(Change::Moved {
        migration,
        from,
        to,
    });
}

/// Remove sections this migration emptied, and note any key it left behind —
/// a key Aura has no descriptor for, which is preserved rather than deleted.
fn prune_emptied_sections(table: &mut Table, migration: &Migration, report: &mut MigrationReport) {
    for section in source_sections(migration) {
        let Some(Value::Table(sub)) = table.get(section) else {
            continue;
        };
        if sub.is_empty() {
            table.remove(section);
            report.changes.push(Change::SectionRemoved {
                migration: migration.id,
                section,
            });
        } else {
            for key in sub.keys() {
                report.changes.push(Change::Unrecognized {
                    section,
                    key: key.clone(),
                });
            }
        }
    }
}

/// Sections this migration takes keys *out* of, in first-seen order.
fn source_sections(migration: &Migration) -> Vec<&'static str> {
    let mut seen: Vec<&'static str> = Vec::new();
    for step in migration.steps {
        let from = match *step {
            Step::MoveKey { from, .. } => from,
            Step::DropKey { key, .. } => key,
        };
        // A step that stays inside its own section (a plain rename) must not
        // mark that section for pruning.
        let stays = matches!(*step, Step::MoveKey { from, to }
            if section_of(from) == section_of(to));
        if stays {
            continue;
        }
        let section = section_of(from);
        if !seen.contains(&section) {
            seen.push(section);
        }
    }
    seen
}

// ── Key aliases ──────────────────────────────────────────────────────────────

/// Where a key that has since moved lives now, chasing the move through every
/// later migration. Returns `None` for a key that never moved (including one
/// that is already current) and for one that was dropped outright.
///
/// This is what lets `aura config get display.tray_color` keep answering after
/// the key became `tray.color`.
pub fn resolve_key(key: &str) -> Option<&'static str> {
    let mut current: Option<&'static str> = None;
    let mut cursor: &str = key;
    for migration in migrations() {
        for step in migration.steps {
            match *step {
                Step::MoveKey { from, to } if from == cursor => {
                    current = Some(to);
                    cursor = to;
                }
                Step::DropKey { key: dropped, .. } if dropped == cursor => {
                    return None;
                }
                _ => {}
            }
        }
    }
    current
}

/// Every legacy key that still resolves, paired with its current name. Used
/// by `aura config describe` to document the aliases in one place.
pub fn aliases() -> Vec<(&'static str, &'static str)> {
    let mut out = Vec::new();
    for migration in migrations() {
        for step in migration.steps {
            let Step::MoveKey { from, .. } = *step else {
                continue;
            };
            // A key moved twice shows up once per hop; only the spelling a
            // user could actually have on disk is an alias worth listing.
            if is_move_destination(from) {
                continue;
            }
            if let Some(to) = resolve_key(from) {
                out.push((from, to));
            }
        }
    }
    out
}

/// True when `key` is itself the destination of some move — an intermediate
/// name that only ever existed between two migrations.
fn is_move_destination(key: &str) -> bool {
    migrations().iter().any(|m| {
        m.steps
            .iter()
            .any(|s| matches!(*s, Step::MoveKey { to, .. } if to == key))
    })
}

// ── Dotted-key helpers ───────────────────────────────────────────────────────

/// Split a dotted key into `(section, leaf)`. A key with no dot is treated as
/// a top-level leaf. Borrows from the input, so a `&'static` key yields
/// `&'static` halves — which is how [`section_of`] stays `'static`.
fn split(key: &str) -> (&str, &str) {
    key.split_once('.').unwrap_or(("", key))
}

/// The table a registry key lives in.
fn section_of(key: &'static str) -> &'static str {
    split(key).0
}

fn get<'a>(table: &'a Table, key: &str) -> Option<&'a Value> {
    let (section, leaf) = split(key);
    if section.is_empty() {
        return table.get(leaf);
    }
    table.get(section)?.as_table()?.get(leaf)
}

fn take(table: &mut Table, key: &str) -> Option<Value> {
    let (section, leaf) = split(key);
    if section.is_empty() {
        return table.remove(leaf);
    }
    table.get_mut(section)?.as_table_mut()?.remove(leaf)
}

fn put(table: &mut Table, key: &str, value: Value) {
    let (section, leaf) = split(key);
    if section.is_empty() {
        table.insert(leaf.to_string(), value);
        return;
    }
    let entry = table
        .entry(section.to_string())
        .or_insert_with(|| Value::Table(Table::new()));
    if !entry.is_table() {
        *entry = Value::Table(Table::new());
    }
    if let Some(sub) = entry.as_table_mut() {
        sub.insert(leaf.to_string(), value);
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    const LEGACY: &str = r#"
[[agents]]
name = "Work Claude"
kind = "claude-code"

[display]
default_period = "7d"
anchor = "top"
linux_backend = "wayland"
plugin_order = ["RTK Gains"]
show_in_app_switcher = true
dismiss_on_focus_loss = false
window_chrome = true
auto_resize = false
max_height = 500
goblin_mode = true
tray_status = false
tray_progress = false
tray_color = false
tray_pulse = true
tray_status_interval_secs = 900

[update]
dismiss_all = true
"#;

    #[test]
    fn splits_legacy_display_into_three_sections() {
        let (table, report) = normalize_str(LEGACY).unwrap();
        assert!(!report.is_noop());

        let window = table["window"].as_table().unwrap();
        assert_eq!(window["anchor"].as_str(), Some("top"));
        assert_eq!(window["linux_backend"].as_str(), Some("wayland"));
        assert_eq!(window["chrome"].as_bool(), Some(true));
        assert_eq!(window["auto_resize"].as_bool(), Some(false));
        assert_eq!(window["max_height"].as_integer(), Some(500));
        assert_eq!(window["show_in_app_switcher"].as_bool(), Some(true));
        assert_eq!(window["dismiss_on_focus_loss"].as_bool(), Some(false));

        let tray = table["tray"].as_table().unwrap();
        assert_eq!(tray["indicator"].as_bool(), Some(false));
        assert_eq!(tray["progress"].as_bool(), Some(false));
        assert_eq!(tray["color"].as_bool(), Some(false));
        assert_eq!(tray["pulse"].as_bool(), Some(true));
        assert_eq!(tray["refresh_secs"].as_integer(), Some(900));

        let content = table["content"].as_table().unwrap();
        assert_eq!(content["default_period"].as_str(), Some("7d"));
        assert_eq!(content["goblin_mode"].as_bool(), Some(true));
        assert_eq!(
            content["plugin_order"].as_array().unwrap()[0].as_str(),
            Some("RTK Gains")
        );

        // The emptied section is gone, and nothing else was disturbed.
        assert!(table.get("display").is_none());
        assert_eq!(table["update"]["dismiss_all"].as_bool(), Some(true));
        assert_eq!(table["agents"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn is_idempotent() {
        let (mut table, first) = normalize_str(LEGACY).unwrap();
        assert!(!first.is_noop());
        let second = normalize(&mut table);
        assert!(second.is_empty(), "second pass changed: {second:?}");
    }

    #[test]
    fn current_shape_is_left_alone() {
        let current = r#"
[window]
anchor = "top"

[tray]
indicator = true

[content]
default_period = "all"
"#;
        let (table, report) = normalize_str(current).unwrap();
        assert!(report.is_empty(), "{report:?}");
        assert_eq!(table["window"]["anchor"].as_str(), Some("top"));
    }

    #[test]
    fn hand_edited_new_key_wins_over_the_legacy_one() {
        let mixed = r#"
[display]
anchor = "bottom"

[window]
anchor = "top"
"#;
        let (table, report) = normalize_str(mixed).unwrap();
        assert_eq!(table["window"]["anchor"].as_str(), Some("top"));
        assert!(table.get("display").is_none());
        assert!(report.changes.iter().any(|c| matches!(
            c,
            Change::Superseded {
                from: "display.anchor",
                ..
            }
        )));
    }

    #[test]
    fn unknown_leftover_keys_are_kept_and_reported() {
        let odd = r#"
[display]
anchor = "top"
handwritten_note = "keep me"
"#;
        let (table, report) = normalize_str(odd).unwrap();
        assert_eq!(table["window"]["anchor"].as_str(), Some("top"));
        // The section survives because it still holds something Aura didn't move.
        assert_eq!(
            table["display"]["handwritten_note"].as_str(),
            Some("keep me")
        );
        assert!(report
            .changes
            .iter()
            .any(|c| matches!(c, Change::Unrecognized { key, .. } if key == "handwritten_note")));
    }

    #[test]
    fn empty_config_is_a_noop() {
        let (_, report) = normalize_str("").unwrap();
        assert!(report.is_empty());
    }

    #[test]
    fn resolve_key_follows_moves() {
        assert_eq!(resolve_key("display.tray_color"), Some("tray.color"));
        assert_eq!(resolve_key("display.window_chrome"), Some("window.chrome"));
        assert_eq!(
            resolve_key("display.tray_status_interval_secs"),
            Some("tray.refresh_secs")
        );
        // Already-current keys and unknown keys do not resolve.
        assert_eq!(resolve_key("tray.color"), None);
        assert_eq!(resolve_key("nonsense.key"), None);
    }

    #[test]
    fn aliases_cover_every_moved_key() {
        let aliases = aliases();
        assert_eq!(aliases.len(), 15);
        assert!(aliases.contains(&("display.goblin_mode", "content.goblin_mode")));
    }
}
