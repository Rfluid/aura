//! Hint mode: press `hint_mode` (default `f`) in a plugin's `controls`
//! section and every button gets a short label; typing a label presses that
//! button, like Vimium's link hints.
//!
//! This module is the pure half — label generation and matching. The view
//! (`AuraView`) owns the mode state and the key handling. While hint mode is
//! on, the root drops its `Aura` key context (see `keys::root_context`), so
//! no keymap binding matches and the typed characters reach the root's
//! `key_down` listener instead.

/// Label characters, home row first. Every label uses only these.
pub const HINT_CHARS: &str = "asdfjklghqwertyuiopzxcvbnm";

/// `count` distinct labels, all the same length, so no label is a prefix of
/// another and a label fires as soon as it is complete. One character per
/// label up to 26 buttons, two up to 676, and so on.
pub fn labels(count: usize) -> Vec<String> {
    let chars: Vec<char> = HINT_CHARS.chars().collect();
    let base = chars.len();
    let mut len = 1;
    let mut capacity = base;
    while capacity < count {
        len += 1;
        capacity = capacity.saturating_mul(base);
    }
    (0..count)
        .map(|mut n| {
            let mut label = vec![chars[0]; len];
            for slot in label.iter_mut().rev() {
                *slot = chars[n % base];
                n /= base;
            }
            label.into_iter().collect()
        })
        .collect()
}

/// What typing `input` means against `labels`.
#[derive(Debug, PartialEq, Eq)]
pub enum Match {
    /// `input` is a whole label: press the button at this index.
    Exact(usize),
    /// `input` starts at least one label; wait for more characters.
    Prefix,
    /// No label starts with `input`.
    None,
}

pub fn resolve(labels: &[String], input: &str) -> Match {
    if let Some(i) = labels.iter().position(|l| l == input) {
        Match::Exact(i)
    } else if labels.iter().any(|l| l.starts_with(input)) {
        Match::Prefix
    } else {
        Match::None
    }
}

/// The hint character a keystroke types, if any: a bare label character,
/// case-insensitive, with no ctrl / alt / cmd / fn held.
pub fn hint_char(key: &str, modifiers: &gpui::Modifiers) -> Option<char> {
    if modifiers.control || modifiers.alt || modifiers.platform || modifiers.function {
        return None;
    }
    let mut chars = key.chars();
    let c = chars.next()?.to_ascii_lowercase();
    (chars.next().is_none() && HINT_CHARS.contains(c)).then_some(c)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_characters_up_to_the_alphabet_size() {
        let l = labels(3);
        assert_eq!(l, ["a", "s", "d"]);
        assert!(labels(26).iter().all(|l| l.len() == 1));
    }

    #[test]
    fn longer_labels_share_one_length() {
        let l = labels(27);
        assert!(l.iter().all(|l| l.len() == 2));
        assert_eq!(l[0], "aa");
        assert_eq!(l[1], "as");
        assert_eq!(l[26], "sa");
        let unique: std::collections::HashSet<_> = l.iter().collect();
        assert_eq!(unique.len(), 27);
    }

    #[test]
    fn no_labels_for_no_buttons() {
        assert!(labels(0).is_empty());
    }

    #[test]
    fn resolve_exact_prefix_none() {
        let l = labels(30);
        assert_eq!(resolve(&l, "aa"), Match::Exact(0));
        assert_eq!(resolve(&l, "a"), Match::Prefix);
        assert_eq!(resolve(&l, "m"), Match::None);
        assert_eq!(resolve(&l, "sz"), Match::None);
    }

    #[test]
    fn hint_char_filters_modifiers_and_named_keys() {
        let none = gpui::Modifiers::default();
        assert_eq!(hint_char("a", &none), Some('a'));
        assert_eq!(hint_char("A", &none), Some('a'));
        assert_eq!(hint_char("1", &none), None);
        assert_eq!(hint_char("escape", &none), None);
        let ctrl = gpui::Modifiers {
            control: true,
            ..Default::default()
        };
        assert_eq!(hint_char("a", &ctrl), None);
        let shift = gpui::Modifiers {
            shift: true,
            ..Default::default()
        };
        assert_eq!(hint_char("a", &shift), Some('a'));
    }
}
