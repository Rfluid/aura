//! Plugin-declared keyboard shortcuts, resolved against the leader.
//!
//! A section lists [`PluginKey`]s (`{"keys": "s", "action": "mute:toggle"}`).
//! The host puts the leader (`[keybindings] plugin_leader`, default `space`)
//! in front of each, so the plugin's `s` is pressed as `space s`. Nothing a
//! plugin declares can shadow one of Aura's own bindings: Aura binds no key
//! sequence that starts with the leader.
//!
//! Like the keymap, resolving never fails: a bad entry is skipped and
//! reported as a [`PluginKeyWarning`], everything else applies. Remapping is
//! the plugin's business — Aura installs whatever the panel declares.

use std::fmt;

use serde::Serialize;

use super::{PluginContent, PluginKey, PluginSection};
use crate::keymap::{parse_keys, BindingContext, Keymap, ParsedKeys};

/// The leader when `plugin_leader` is unset or invalid.
pub const DEFAULT_LEADER: &str = "space";

/// Parse a `plugin_leader` value. `"none"` (or empty) turns plugin keys off,
/// `Ok(None)`.
pub fn parse_leader(raw: &str) -> Result<Option<ParsedKeys>, String> {
    let raw = raw.trim();
    if raw.is_empty() || raw.eq_ignore_ascii_case("none") {
        return Ok(None);
    }
    parse_keys(raw).map(Some)
}

/// The leader to use for `raw`, falling back to [`DEFAULT_LEADER`] when it
/// doesn't parse (the error is returned alongside so it can be reported).
pub fn effective_leader(raw: &str) -> (Option<ParsedKeys>, Option<String>) {
    match parse_leader(raw) {
        Ok(leader) => (leader, None),
        Err(e) => (
            parse_keys(DEFAULT_LEADER).ok(),
            Some(format!(
                "keybindings.plugin_leader: {e}; using `{DEFAULT_LEADER}`"
            )),
        ),
    }
}

/// One plugin key, ready to install.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PluginBinding {
    /// Canonical sequence, leader included (`"space s"`), in the form the
    /// toolkit's keystroke parser accepts.
    pub keys: String,
    /// The same, written for humans (`"space S"`).
    pub display: String,
    /// Only the plugin's part, canonical (`"shift-s"`): what the host
    /// matches the strokes typed after the leader against.
    pub own_keys: String,
    /// Only the plugin's part, for humans (`"S"`).
    pub key_display: String,
    pub action: String,
    pub label: String,
    pub confirm: Option<String>,
}

/// A problem with one declared key. The key is skipped (or, for the prefix
/// case, only delayed).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PluginKeyWarning {
    pub message: String,
}

impl fmt::Display for PluginKeyWarning {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

fn warn(section: &PluginSection, message: String) -> PluginKeyWarning {
    PluginKeyWarning {
        message: format!("section `{}`: {message}", section.id),
    }
}

/// Resolve `section`'s keys behind `leader`, in declaration order.
///
/// - An invalid keystroke or an empty `action` skips the entry.
/// - The same keys twice: the later entry wins (spellings compare
///   canonically, so `S` and `shift-s` are the same keys).
/// - One key starting another (`d` and `d d`): both stay, but the host
///   fires the shorter one as soon as it's typed, so the longer one can
///   never be pressed.
pub fn resolve(
    section: &PluginSection,
    leader: &ParsedKeys,
) -> (Vec<PluginBinding>, Vec<PluginKeyWarning>) {
    let mut bindings: Vec<PluginBinding> = Vec::new();
    let mut warnings = Vec::new();

    for key in &section.keys {
        if key.action.trim().is_empty() {
            warnings.push(warn(
                section,
                format!("key `{}` has an empty action; skipped", key.keys),
            ));
            continue;
        }
        let own = match parse_keys(&key.keys) {
            Ok(own) => own,
            Err(e) => {
                warnings.push(warn(section, format!("{e}; skipped")));
                continue;
            }
        };
        if let Some(i) = bindings.iter().position(|b| b.keys == join(leader, &own)) {
            warnings.push(warn(
                section,
                format!(
                    "`{}` is declared twice; `{}` wins over `{}`",
                    own.display, key.action, bindings[i].action
                ),
            ));
            bindings.remove(i);
        }
        bindings.push(binding(key, leader, &own));
    }

    for a in &bindings {
        for b in &bindings {
            if b.keys.starts_with(&format!("{} ", a.keys)) {
                warnings.push(warn(
                    section,
                    format!(
                        "`{}` starts `{}`, so `{}` can never be pressed",
                        a.key_display, b.key_display, b.key_display
                    ),
                ));
            }
        }
    }

    (bindings, warnings)
}

fn join(leader: &ParsedKeys, own: &ParsedKeys) -> String {
    format!("{} {}", leader.canonical, own.canonical)
}

fn binding(key: &PluginKey, leader: &ParsedKeys, own: &ParsedKeys) -> PluginBinding {
    PluginBinding {
        keys: join(leader, own),
        display: format!("{} {}", leader.display, own.display),
        own_keys: own.canonical.clone(),
        key_display: own.display.clone(),
        action: key.action.clone(),
        label: key.label.clone(),
        confirm: key.confirm.clone(),
    }
}

/// Whether some button in `section` has `action` as its id and asks for
/// confirmation, i.e. whether a key for it needs two presses.
pub fn button_confirms(section: &PluginSection, action: &str) -> bool {
    match &section.content {
        PluginContent::Controls { controls } => controls
            .iter()
            .flat_map(|c| &c.buttons)
            .any(|b| b.id == action && b.confirm.is_some()),
        _ => false,
    }
}

/// Keymap bindings the leader collides with: a `[global]` binding on the
/// leader itself, one the leader starts, or one that starts the leader. The
/// toolkit still resolves these (it waits for the next stroke), but one of
/// the two only fires after a timeout, so it is worth a warning.
pub fn leader_conflicts(keymap: &Keymap, leader: &ParsedKeys) -> Vec<String> {
    let l = &leader.canonical;
    keymap
        .bindings
        .iter()
        .filter(|b| b.context == BindingContext::Global && b.action.is_some())
        .filter(|b| {
            b.keys == *l
                || b.keys.starts_with(&format!("{l} "))
                || l.starts_with(&format!("{} ", b.keys))
        })
        .map(|b| {
            format!(
                "keybindings.plugin_leader `{}` collides with `{}` ({}) in [global]; \
                 one of them fires only after a short wait",
                leader.display,
                b.display,
                b.action.map(|a| a.name()).unwrap_or("none"),
            )
        })
        .collect()
}

/// Everything wrong with the `plugin_leader` value `raw` against `keymap`:
/// an invalid value (which falls back to [`DEFAULT_LEADER`]) and
/// [`leader_conflicts`]. For `aura keys validate`, `aura doctor` and the
/// help overlay.
pub fn leader_warnings(raw: &str, keymap: &Keymap) -> Vec<String> {
    let (leader, error) = effective_leader(raw);
    let mut warnings: Vec<String> = error.into_iter().collect();
    if let Some(leader) = leader {
        warnings.extend(leader_conflicts(keymap, &leader));
    }
    warnings
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(keys: &str, action: &str) -> PluginKey {
        PluginKey {
            keys: keys.to_string(),
            action: action.to_string(),
            label: action.to_string(),
            confirm: None,
        }
    }

    fn section(keys: Vec<PluginKey>) -> PluginSection {
        PluginSection {
            id: "s".to_string(),
            label: "S".to_string(),
            uses_period: false,
            keys,
            content: PluginContent::default(),
        }
    }

    fn space() -> ParsedKeys {
        parse_leader("space").unwrap().unwrap()
    }

    #[test]
    fn leader_parses_and_none_disables() {
        assert_eq!(parse_leader("space").unwrap().unwrap().canonical, "space");
        assert_eq!(parse_leader("ctrl-p").unwrap().unwrap().canonical, "ctrl-p");
        assert!(parse_leader("none").unwrap().is_none());
        assert!(parse_leader("").unwrap().is_none());
        assert!(parse_leader("hyper-x").is_err());
    }

    #[test]
    fn invalid_leader_falls_back_to_space() {
        let (leader, err) = effective_leader("hyper-x");
        assert_eq!(leader.unwrap().canonical, "space");
        assert!(err.unwrap().contains("plugin_leader"));
        let (leader, err) = effective_leader("none");
        assert!(leader.is_none() && err.is_none());
    }

    #[test]
    fn keys_get_the_leader_in_front() {
        let (b, w) = resolve(&section(vec![key("s", "mute"), key("D", "rm")]), &space());
        assert!(w.is_empty(), "{w:?}");
        assert_eq!(b[0].keys, "space s");
        assert_eq!(b[0].display, "space s");
        assert_eq!(b[0].key_display, "s");
        assert_eq!(b[1].keys, "space shift-d");
        assert_eq!(b[1].own_keys, "shift-d");
        assert_eq!(b[1].key_display, "D");
    }

    #[test]
    fn bad_entries_are_skipped_with_a_warning() {
        let (b, w) = resolve(
            &section(vec![key("hyper-x", "a"), key("s", " "), key("m", "ok")]),
            &space(),
        );
        assert_eq!(b.len(), 1);
        assert_eq!(b[0].action, "ok");
        assert_eq!(w.len(), 2);
        assert!(w.iter().all(|w| w.message.starts_with("section `s`")));
    }

    #[test]
    fn later_duplicate_wins() {
        let (b, w) = resolve(
            &section(vec![key("S", "first"), key("shift-s", "second")]),
            &space(),
        );
        assert_eq!(b.len(), 1);
        assert_eq!(b[0].action, "second");
        assert_eq!(w.len(), 1);
    }

    #[test]
    fn prefix_is_warned_but_kept() {
        let (b, w) = resolve(&section(vec![key("d", "a"), key("d d", "b")]), &space());
        assert_eq!(b.len(), 2);
        assert_eq!(w.len(), 1);
        assert!(w[0].message.contains("`d d` can never be pressed"));
    }

    #[test]
    fn button_confirm_is_found_by_id() {
        let mut s = section(Vec::new());
        s.content = serde_json::from_str(
            r#"{"type": "controls", "controls": [{"label": "x", "buttons": [
                {"id": "rm", "label": "Remove", "confirm": "Sure?"},
                {"id": "ok", "label": "Ok"}
            ]}]}"#,
        )
        .unwrap();
        assert!(button_confirms(&s, "rm"));
        assert!(!button_confirms(&s, "ok"));
        assert!(!button_confirms(&s, "missing"));
    }

    #[test]
    fn leader_warnings_cover_invalid_and_conflicts() {
        let keymap = Keymap::defaults();
        assert!(leader_warnings("space", &keymap).is_empty());
        assert!(leader_warnings("none", &keymap).is_empty());
        // Invalid: warned, then checked as `space`, which is free.
        assert_eq!(leader_warnings("hyper-x", &keymap).len(), 1);
        assert_eq!(leader_warnings("g", &keymap).len(), 3);
    }

    #[test]
    fn default_keymap_leaves_space_free() {
        assert!(leader_conflicts(&Keymap::defaults(), &space()).is_empty());
    }

    #[test]
    fn leader_conflicts_with_global_bindings() {
        let keymap = Keymap::from_toml("[global]\n\"space\" = \"refresh\"\n");
        assert_eq!(leader_conflicts(&keymap, &space()).len(), 1);
        // `g` is the start of `g g` / `g t` / `g T` in the defaults.
        let g = parse_leader("g").unwrap().unwrap();
        assert_eq!(leader_conflicts(&Keymap::defaults(), &g).len(), 3);
    }
}
