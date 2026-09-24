//! Editing `keybindings.toml`: the write half of [`super`].
//!
//! [`KeymapFile`] wraps the user's document in `toml_edit`, so a change made
//! from the CLI (`aura keys set`, `wizard`, `merge`, …) touches only the entry
//! it is about. Comments, ordering and entries this module can't parse are
//! left exactly as the user wrote them. [`KeymapFile::document`] is the one
//! operation that rewrites the whole file, and it says what it drops.
//!
//! Every operation compares keystrokes in canonical form, so `G` and
//! `shift-g` are the same entry: binding one replaces the other.

use std::{fs, path::Path};

use anyhow::{Context as _, Result};
use serde::Serialize;
use toml_edit::{value, DocumentMut, Item, Table, TableLike};

use super::{
    parse_action, parse_keys, render_file, BindingContext, KeyAction, Keymap, RenderEntry, DEFAULTS,
};

/// A user keymap file open for editing.
#[derive(Debug, Clone)]
pub struct KeymapFile {
    doc: DocumentMut,
}

/// One entry of the file that parses: a valid keystroke bound to a known
/// action (or `"none"`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FileEntry {
    pub context: BindingContext,
    /// The key exactly as written in the file.
    pub raw: String,
    pub canonical: String,
    pub display: String,
    pub action: Option<KeyAction>,
}

/// What [`KeymapFile::merge`] does when both files bind the same keys to
/// different actions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MergePrefer {
    /// The incoming file wins.
    Theirs,
    /// The existing file wins; the incoming entry is reported as a conflict.
    Ours,
}

/// One entry a merge touched (or declined to).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MergeChange {
    pub context: BindingContext,
    pub keys: String,
    /// Action in the existing file, `None` when it had no entry.
    pub ours: Option<String>,
    /// Action in the incoming file.
    pub theirs: String,
}

/// Outcome of [`KeymapFile::merge`].
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct MergeReport {
    /// New entries.
    pub added: Vec<MergeChange>,
    /// Entries whose action the incoming file replaced.
    pub changed: Vec<MergeChange>,
    /// Entries both files agree on.
    pub unchanged: usize,
    /// Conflicts the existing file kept (`MergePrefer::Ours`).
    pub kept: Vec<MergeChange>,
    /// Problems in the incoming file; its broken entries are not merged.
    pub skipped: Vec<String>,
    /// `use_defaults` as `(ours, theirs)` when the two disagree.
    pub use_defaults: Option<(bool, bool)>,
}

impl MergeReport {
    /// Whether the merge changes the file.
    pub fn changes_anything(&self, prefer: MergePrefer) -> bool {
        !self.added.is_empty()
            || !self.changed.is_empty()
            || (prefer == MergePrefer::Theirs && self.use_defaults.is_some())
    }
}

impl Default for KeymapFile {
    /// The starter file, as `aura keys init` writes it.
    fn default() -> Self {
        Self::parse(&Keymap::default_file_contents()).expect("starter keymap parses")
    }
}

impl KeymapFile {
    /// Parse a document. Only TOML syntax is checked here; entries Aura can't
    /// use are kept as-is and reported by [`Self::keymap`]'s warnings.
    pub fn parse(content: &str) -> Result<Self> {
        let doc = content
            .parse::<DocumentMut>()
            .context("keybindings.toml is not valid TOML")?;
        Ok(Self { doc })
    }

    /// Open `path`, or the starter file when it doesn't exist yet.
    pub fn load(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let content =
            fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
        Self::parse(&content).with_context(|| format!("parse {}", path.display()))
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("create config dir {}", parent.display()))?;
        }
        fs::write(path, self.contents()).with_context(|| format!("write {}", path.display()))
    }

    pub fn contents(&self) -> String {
        self.doc.to_string()
    }

    /// The effective keymap this file produces over the defaults.
    pub fn keymap(&self) -> Keymap {
        Keymap::from_toml(&self.contents())
    }

    /// `use_defaults` as written, `None` when absent or not a boolean.
    pub fn use_defaults(&self) -> Option<bool> {
        self.doc.get("use_defaults").and_then(Item::as_bool)
    }

    pub fn set_use_defaults(&mut self, on: bool) {
        match self.doc.get_mut("use_defaults") {
            Some(item) => *item = value(on),
            None => {
                self.doc.insert("use_defaults", value(on));
            }
        }
    }

    /// Every valid entry, in file order.
    pub fn entries(&self) -> Vec<FileEntry> {
        let mut out = Vec::new();
        for context in BindingContext::ALL {
            let Some(table) = self.table(context) else {
                continue;
            };
            for (raw, item) in table.iter() {
                let (Ok(keys), Some(Ok(action))) =
                    (parse_keys(raw), item.as_str().map(parse_action))
                else {
                    continue;
                };
                out.push(FileEntry {
                    context,
                    raw: raw.to_string(),
                    canonical: keys.canonical,
                    display: keys.display,
                    action,
                });
            }
        }
        out
    }

    /// The file's entry for `canonical` in `context`, if it has one.
    pub fn entry(&self, context: BindingContext, canonical: &str) -> Option<FileEntry> {
        self.entries()
            .into_iter()
            .rev()
            .find(|e| e.context == context && e.canonical == canonical)
    }

    /// Bind `keys` to `action` (`None` = unbind) in `context`, replacing any
    /// entry for the same keystroke, whatever its spelling. An existing entry
    /// written exactly as `keys` is updated in place, keeping its position.
    pub fn bind(
        &mut self,
        context: BindingContext,
        keys: &str,
        action: Option<KeyAction>,
    ) -> Result<(), String> {
        let parsed = parse_keys(keys)?;
        let raw = keys.split_whitespace().collect::<Vec<_>>().join(" ");
        let new_value = value(action.map(KeyAction::name).unwrap_or("none"));

        let table = self.table_mut(context);
        let spellings: Vec<String> = table
            .iter()
            .filter(|(k, _)| *k != raw && same_keys(k, &parsed.canonical))
            .map(|(k, _)| k.to_string())
            .collect();
        for k in spellings {
            table.remove(&k);
        }
        match table.get_mut(&raw) {
            Some(item) => *item = new_value,
            None => {
                table.insert(&raw, new_value);
            }
        }
        Ok(())
    }

    /// Delete the file's entry for `keys` in `context` (every spelling), so
    /// the default — if any — applies again. Returns how many were removed.
    pub fn remove(&mut self, context: BindingContext, keys: &str) -> Result<usize, String> {
        let parsed = parse_keys(keys)?;
        Ok(self.remove_canonical(context, &parsed.canonical))
    }

    fn remove_canonical(&mut self, context: BindingContext, canonical: &str) -> usize {
        let Some(table) = self.table_mut_existing(context) else {
            return 0;
        };
        let matching: Vec<String> = table
            .iter()
            .filter(|(k, _)| same_keys(k, canonical))
            .map(|(k, _)| k.to_string())
            .collect();
        for k in &matching {
            table.remove(k);
        }
        matching.len()
    }

    /// Delete every valid entry in `context` (or in every context), leaving
    /// the defaults. Entries Aura can't parse are left for the user to fix.
    pub fn clear(&mut self, context: Option<BindingContext>) -> usize {
        let targets: Vec<FileEntry> = self
            .entries()
            .into_iter()
            .filter(|e| context.is_none_or(|c| c == e.context))
            .collect();
        for e in &targets {
            if let Some(table) = self.table_mut_existing(e.context) {
                table.remove(&e.raw);
            }
        }
        targets.len()
    }

    /// Make `keys` exactly the keystrokes that run `action` in `context`.
    ///
    /// A key the action loses is unbound when it's one of the action's
    /// defaults, and otherwise has its entry removed (so whatever the default
    /// for that key is applies again). A key it gains is bound. Returns notes
    /// about keys taken from other actions or handed back to their defaults.
    pub fn set_action_keys(
        &mut self,
        context: BindingContext,
        action: KeyAction,
        keys: &[String],
    ) -> Result<Vec<String>, String> {
        let map = self.keymap();
        let defaults_on = self.use_defaults() != Some(false);
        let default_action = |canonical: &str| {
            defaults_on
                .then(|| {
                    DEFAULTS
                        .iter()
                        .find(|(c, k, _)| {
                            *c == context && parse_keys(k).is_ok_and(|p| p.canonical == canonical)
                        })
                        .map(|(_, _, a)| *a)
                })
                .flatten()
        };

        // Validate everything before touching the document.
        let mut wanted: Vec<(String, super::ParsedKeys)> = Vec::new();
        for k in keys {
            let parsed = parse_keys(k)?;
            if !wanted.iter().any(|(_, p)| p.canonical == parsed.canonical) {
                wanted.push((k.clone(), parsed));
            }
        }

        let current: Vec<(String, String)> = map
            .bindings
            .iter()
            .filter(|b| b.context == context && b.action == Some(action))
            .map(|b| (b.keys.clone(), b.display.clone()))
            .collect();

        let mut notes = Vec::new();
        for (canonical, display) in &current {
            if wanted.iter().any(|(_, p)| &p.canonical == canonical) {
                continue;
            }
            match default_action(canonical) {
                Some(a) if a == action => self.bind(context, display, None)?,
                other => {
                    self.remove_canonical(context, canonical);
                    if let Some(a) = other {
                        notes.push(format!("`{display}` goes back to its default, {a}"));
                    }
                }
            }
        }
        for (raw, parsed) in &wanted {
            if current.iter().any(|(c, _)| *c == parsed.canonical) {
                continue;
            }
            if let Some(Some(other)) = map
                .lookup(context, &parsed.canonical)
                .map(|b| b.action)
                .filter(|a| *a != Some(action))
            {
                notes.push(format!("`{}` was {other}; now {action}", parsed.display));
            }
            if default_action(&parsed.canonical) == Some(action) {
                // The default already does this; drop whatever overrode it.
                self.remove_canonical(context, &parsed.canonical);
            } else {
                self.bind(context, raw, Some(action))?;
            }
        }
        Ok(notes)
    }

    /// Put `action` back to its default keys in `context`: remove every file
    /// entry that binds it, and every entry that overrides one of its
    /// default keys. Returns how many entries were removed.
    pub fn restore_action(&mut self, context: BindingContext, action: KeyAction) -> usize {
        let defaults: Vec<String> = DEFAULTS
            .iter()
            .filter(|(c, _, a)| *c == context && *a == action)
            .filter_map(|(_, k, _)| parse_keys(k).ok().map(|p| p.canonical))
            .collect();
        let targets: Vec<FileEntry> = self
            .entries()
            .into_iter()
            .filter(|e| {
                e.context == context
                    && (e.action == Some(action) || defaults.contains(&e.canonical))
            })
            .collect();
        for e in &targets {
            if let Some(table) = self.table_mut_existing(e.context) {
                table.remove(&e.raw);
            }
        }
        targets.len()
    }

    /// Fold `other`'s entries into this file. Same keys bound to the same
    /// action count as unchanged; a different action is resolved by `prefer`.
    /// `other`'s broken entries are skipped and reported, never copied.
    pub fn merge(&mut self, other: &KeymapFile, prefer: MergePrefer) -> MergeReport {
        let mut report = MergeReport {
            skipped: other
                .keymap()
                .warnings
                .into_iter()
                .map(|w| w.message)
                .collect(),
            ..MergeReport::default()
        };

        let ours_defaults = self.use_defaults().unwrap_or(true);
        if let Some(theirs) = other.use_defaults() {
            if theirs != ours_defaults {
                report.use_defaults = Some((ours_defaults, theirs));
                if prefer == MergePrefer::Theirs {
                    self.set_use_defaults(theirs);
                }
            }
        }

        for incoming in other.entries() {
            let name = |a: Option<KeyAction>| a.map(KeyAction::name).unwrap_or("none").to_string();
            let existing = self.entry(incoming.context, &incoming.canonical);
            let change = MergeChange {
                context: incoming.context,
                keys: incoming.display.clone(),
                ours: existing.as_ref().map(|e| name(e.action)),
                theirs: name(incoming.action),
            };
            match existing {
                None => {
                    // Parsed by `entries`, so binding can't fail.
                    let _ = self.bind(incoming.context, &incoming.raw, incoming.action);
                    report.added.push(change);
                }
                Some(e) if e.action == incoming.action => report.unchanged += 1,
                Some(_) => match prefer {
                    MergePrefer::Theirs => {
                        let _ = self.bind(incoming.context, &incoming.raw, incoming.action);
                        report.changed.push(change);
                    }
                    MergePrefer::Ours => report.kept.push(change),
                },
            }
        }
        report
    }

    /// Rewrite the file in the generated layout: the explanatory header,
    /// every valid entry with its action's description, and the defaults as a
    /// commented reference. Returns the new file and what it could not carry
    /// over (the current file's warnings; comments are regenerated).
    pub fn document(&self) -> (KeymapFile, Vec<String>) {
        let entries: Vec<RenderEntry> = self
            .entries()
            .into_iter()
            .map(|e| RenderEntry {
                context: e.context,
                keys: e.raw,
                action: e.action,
            })
            .collect();
        let rendered = render_file(
            None,
            self.use_defaults().unwrap_or(true),
            &dedupe_last(entries),
            true,
        );
        let dropped = self
            .keymap()
            .warnings
            .into_iter()
            .map(|w| w.message)
            // Prefix clashes survive the rewrite; they aren't dropped entries.
            .filter(|m| !m.contains("is the start of"))
            .collect();
        (
            KeymapFile::parse(&rendered).expect("rendered keymap parses"),
            dropped,
        )
    }

    fn table(&self, context: BindingContext) -> Option<&dyn TableLike> {
        self.doc.get(context.name()).and_then(Item::as_table_like)
    }

    fn table_mut_existing(&mut self, context: BindingContext) -> Option<&mut dyn TableLike> {
        self.doc
            .get_mut(context.name())
            .and_then(Item::as_table_like_mut)
    }

    /// The table for `context`, created if missing (or replaced, if the key
    /// holds something that isn't a table — `from_toml` already warns that
    /// such a value is ignored).
    fn table_mut(&mut self, context: BindingContext) -> &mut dyn TableLike {
        let name = context.name();
        let is_table = self
            .doc
            .get(name)
            .is_some_and(|item| item.as_table_like().is_some());
        if !is_table {
            self.doc.insert(name, Item::Table(Table::new()));
        }
        self.doc
            .get_mut(name)
            .and_then(Item::as_table_like_mut)
            .expect("table was just ensured")
    }
}

fn same_keys(raw: &str, canonical: &str) -> bool {
    parse_keys(raw).is_ok_and(|p| p.canonical == canonical)
}

/// Keep only the last entry per `(context, keystroke)`, as the loader does.
fn dedupe_last(entries: Vec<RenderEntry>) -> Vec<RenderEntry> {
    let mut out: Vec<RenderEntry> = Vec::new();
    for e in entries {
        let canonical = parse_keys(&e.keys).map(|p| p.canonical).ok();
        out.retain(|o| {
            !(o.context == e.context && parse_keys(&o.keys).map(|p| p.canonical).ok() == canonical)
        });
        out.push(e);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const G: BindingContext = BindingContext::Global;
    const O: BindingContext = BindingContext::Overlay;

    fn action(file: &KeymapFile, context: BindingContext, keys: &str) -> Option<Option<KeyAction>> {
        let canonical = parse_keys(keys).unwrap().canonical;
        file.keymap().lookup(context, &canonical).map(|b| b.action)
    }

    #[test]
    fn bind_preserves_comments_and_replaces_other_spellings() {
        let mut f = KeymapFile::parse(
            r#"# my notes
[global]
"G" = "scroll_top"   # keep me
"x" = "refresh"
"#,
        )
        .unwrap();
        f.bind(G, "shift-g", Some(KeyAction::ScrollBottom)).unwrap();
        f.bind(G, "x", Some(KeyAction::ToggleHelp)).unwrap();
        let out = f.contents();
        assert!(out.contains("# my notes"), "{out}");
        assert!(!out.contains("\"G\""), "{out}");
        assert!(out.contains("\"shift-g\" = \"scroll_bottom\"") || out.contains("shift-g"));
        assert_eq!(action(&f, G, "x"), Some(Some(KeyAction::ToggleHelp)));
        assert_eq!(action(&f, G, "G"), Some(Some(KeyAction::ScrollBottom)));
    }

    #[test]
    fn bind_into_starter_file_and_new_context() {
        let mut f = KeymapFile::default();
        f.bind(G, "g k", Some(KeyAction::OpenKeybindings)).unwrap();
        f.bind(O, "q", Some(KeyAction::CloseOverlay)).unwrap();
        assert!(f.keymap().warnings.is_empty(), "{:?}", f.keymap().warnings);
        assert_eq!(action(&f, G, "g k"), Some(Some(KeyAction::OpenKeybindings)));
        assert_eq!(action(&f, O, "q"), Some(Some(KeyAction::CloseOverlay)));
        // The commented defaults reference is still there.
        assert!(f.contents().contains("# Defaults:"));
    }

    #[test]
    fn bind_rejects_bad_keys() {
        let mut f = KeymapFile::default();
        assert!(f.bind(G, "hyper-x", Some(KeyAction::Refresh)).is_err());
    }

    #[test]
    fn remove_restores_the_default() {
        let mut f = KeymapFile::parse("[global]\n\"j\" = \"refresh\"\n").unwrap();
        assert_eq!(f.remove(G, "j").unwrap(), 1);
        assert_eq!(action(&f, G, "j"), Some(Some(KeyAction::ScrollDown)));
        assert_eq!(f.remove(G, "j").unwrap(), 0);
    }

    #[test]
    fn clear_keeps_broken_entries() {
        let mut f =
            KeymapFile::parse("[global]\n\"j\" = \"refresh\"\n\"y\" = \"scrol_down\"\n").unwrap();
        assert_eq!(f.clear(None), 1);
        assert!(f.contents().contains("scrol_down"));
    }

    #[test]
    fn set_action_keys_replaces_the_key_set() {
        let mut f = KeymapFile::default();
        let notes = f
            .set_action_keys(
                G,
                KeyAction::ScrollDown,
                &["j".to_string(), "ctrl-n".to_string(), "x".to_string()],
            )
            .unwrap();
        assert!(notes.is_empty(), "{notes:?}");
        let map = f.keymap();
        assert!(map.warnings.is_empty(), "{:?}", map.warnings);
        assert_eq!(
            map.keys_for(KeyAction::ScrollDown, G),
            vec!["j", "ctrl-n", "x"]
        );
        // `down` was a default of scroll_down, so it is now explicitly unbound.
        assert_eq!(action(&f, G, "down"), Some(None));
        // `j` is still the default binding, so no entry was written for it.
        assert!(f.entry(G, "j").is_none());
    }

    #[test]
    fn set_action_keys_notes_stolen_keys_and_reverts_overrides() {
        let mut f = KeymapFile::parse("[global]\n\"j\" = \"scroll_up\"\n").unwrap();
        let notes = f
            .set_action_keys(G, KeyAction::ScrollUp, &["k".to_string(), "r".to_string()])
            .unwrap();
        // Dropping `j` from scroll_up hands it back to scroll_down.
        assert!(
            notes.iter().any(|n| n.contains("`j` goes back")),
            "{notes:?}"
        );
        assert!(
            notes.iter().any(|n| n.contains("`r` was refresh")),
            "{notes:?}"
        );
        assert_eq!(action(&f, G, "j"), Some(Some(KeyAction::ScrollDown)));
        assert_eq!(action(&f, G, "r"), Some(Some(KeyAction::ScrollUp)));
    }

    #[test]
    fn set_action_keys_to_nothing_unbinds_defaults() {
        let mut f = KeymapFile::default();
        f.set_action_keys(G, KeyAction::Refresh, &[]).unwrap();
        assert!(f.keymap().keys_for(KeyAction::Refresh, G).is_empty());
    }

    #[test]
    fn restore_action_undoes_everything_about_it() {
        let mut f = KeymapFile::default();
        f.set_action_keys(G, KeyAction::Refresh, &["x".to_string()])
            .unwrap();
        assert_eq!(f.restore_action(G, KeyAction::Refresh), 3);
        assert_eq!(
            f.keymap().keys_for(KeyAction::Refresh, G),
            Keymap::default_keys(KeyAction::Refresh, G)
        );
    }

    #[test]
    fn merge_adds_changes_and_reports() {
        let mut ours =
            KeymapFile::parse("[global]\n\"x\" = \"refresh\"\n\"y\" = \"quit\"\n").unwrap();
        let theirs = KeymapFile::parse(
            "use_defaults = false\n[global]\n\"x\" = \"refresh\"\n\"y\" = \"toggle_help\"\n\"z\" = \"refresh\"\n\"w\" = \"nope\"\n",
        )
        .unwrap();

        let mut kept = ours.clone();
        let report = kept.merge(&theirs, MergePrefer::Ours);
        assert_eq!(report.kept.len(), 1);
        assert_eq!(report.added.len(), 1);
        assert_eq!(report.unchanged, 1);
        assert_eq!(report.use_defaults, Some((true, false)));
        assert_eq!(kept.use_defaults(), None);
        assert_eq!(action(&kept, G, "y"), Some(Some(KeyAction::Quit)));

        let report = ours.merge(&theirs, MergePrefer::Theirs);
        assert_eq!(report.changed.len(), 1);
        assert_eq!(report.added[0].keys, "z");
        assert!(
            report.skipped.iter().any(|s| s.contains("nope")),
            "{report:?}"
        );
        assert_eq!(ours.use_defaults(), Some(false));
        assert_eq!(action(&ours, G, "y"), Some(Some(KeyAction::ToggleHelp)));
        assert!(!ours.contents().contains("nope"));
    }

    #[test]
    fn document_keeps_values_and_reports_drops() {
        let f = KeymapFile::parse(
            "# gone\n[global]\n\"x\" = \"refresh\"\n\"G\" = \"scroll_top\"\n\"shift-g\" = \"quit\"\n\"y\" = \"scrol_down\"\n",
        )
        .unwrap();
        let (doc, dropped) = f.document();
        let out = doc.contents();
        assert!(!out.contains("# gone"));
        assert!(out.contains("# Refresh"), "{out}");
        assert!(!out.contains("scrol_down"), "{out}");
        assert!(
            dropped.iter().any(|d| d.contains("scrol_down")),
            "{dropped:?}"
        );
        assert_eq!(action(&doc, G, "G"), Some(Some(KeyAction::Quit)));
        assert!(
            doc.keymap().warnings.is_empty(),
            "{:?}",
            doc.keymap().warnings
        );
        assert_eq!(doc.keymap().bindings, f.keymap().bindings);
    }

    #[test]
    fn explicit_export_round_trips() {
        let mut f = KeymapFile::default();
        f.bind(G, "x", Some(KeyAction::Refresh)).unwrap();
        f.bind(O, "q", None).unwrap();
        let map = f.keymap();
        let exported = Keymap::from_toml(&map.to_explicit_toml());
        assert!(exported.warnings.is_empty(), "{:?}", exported.warnings);
        let key = |m: &Keymap| {
            let mut v: Vec<_> = m
                .bindings
                .iter()
                .map(|b| (b.context.name(), b.keys.clone(), b.action))
                .collect();
            v.sort_by(|a, b| (a.0, &a.1).cmp(&(b.0, &b.1)));
            v
        };
        assert_eq!(key(&exported), key(&map));
    }
}
