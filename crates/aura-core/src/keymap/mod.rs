//! Keyboard shortcuts for the modal: the action catalogue, the built-in
//! vim-style defaults, and the `keybindings.toml` override layer.
//!
//! This module is UI-toolkit agnostic. It resolves the effective keymap
//! (defaults merged with the user file) into plain [`Binding`]s and collects
//! every problem it finds as a [`KeymapWarning`]. The GUI turns the bindings
//! into toolkit key bindings; the CLI prints them and the warnings.
//!
//! # File format
//!
//! ```toml
//! # Start from the built-in defaults (true) or from an empty keymap (false).
//! use_defaults = true
//!
//! # One table per context. Keys are keystrokes, values are action names.
//! [global]
//! "ctrl-j" = "scroll_down"   # add a binding
//! "x"      = "refresh"       # rebind
//! "t"      = "none"          # unbind a default
//!
//! # Checked before [global] while an overlay (menu, settings, help) is open.
//! [overlay]
//! "q" = "close_overlay"
//! ```
//!
//! Keystrokes are `-`-joined modifiers followed by a key (`ctrl-d`,
//! `shift-tab`, `secondary-,`), and a space separates the strokes of a
//! sequence (`g g`). `secondary` is `cmd` on macOS and `ctrl` elsewhere. A
//! single uppercase letter means shift plus that letter, so `G` and
//! `shift-g` are the same binding.
//!
//! Loading never fails: an unreadable or malformed file falls back to the
//! defaults and says so in a warning, so a typo can never leave the modal
//! without its shortcuts.
//!
//! [`file::KeymapFile`] is the editing half: format-preserving changes to the
//! user file, used by the `aura keys` CLI.

pub mod file;

use std::{
    collections::HashSet,
    fmt, fs,
    path::{Path, PathBuf},
};

use serde::Serialize;

// ── Actions ──────────────────────────────────────────────────────────────────

/// Something a keystroke can do. The snake_case [`name`](Self::name) is what
/// `keybindings.toml` spells it as.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum KeyAction {
    ScrollDown,
    ScrollUp,
    HalfPageDown,
    HalfPageUp,
    PageDown,
    PageUp,
    ScrollTop,
    ScrollBottom,
    NextSection,
    PrevSection,
    Section1,
    Section2,
    Section3,
    Section4,
    Section5,
    Section6,
    Section7,
    Section8,
    Section9,
    NextProfile,
    PrevProfile,
    ToggleMode,
    NextPeriod,
    PrevPeriod,
    Refresh,
    ToggleSettings,
    ToggleMore,
    ToggleHelp,
    CloseOverlay,
    Dismiss,
    Quit,
    OpenConfig,
    OpenTheme,
    OpenKeybindings,
    OpenUpdate,
    DismissUpdate,
    HintMode,
}

/// How the help overlay and `aura keys list` group actions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ActionGroup {
    Scroll,
    Navigate,
    Commands,
}

impl ActionGroup {
    pub const ALL: [Self; 3] = [Self::Scroll, Self::Navigate, Self::Commands];

    pub fn label(self) -> &'static str {
        match self {
            Self::Scroll => "Scroll",
            Self::Navigate => "Navigate",
            Self::Commands => "Commands",
        }
    }
}

/// One row of the action catalogue.
struct ActionInfo {
    action: KeyAction,
    name: &'static str,
    group: ActionGroup,
    description: &'static str,
}

const fn info(
    action: KeyAction,
    name: &'static str,
    group: ActionGroup,
    description: &'static str,
) -> ActionInfo {
    ActionInfo {
        action,
        name,
        group,
        description,
    }
}

use ActionGroup::{Commands, Navigate, Scroll};
use KeyAction as A;

/// Every action, in help-overlay order.
const ACTIONS: &[ActionInfo] = &[
    info(A::ScrollDown, "scroll_down", Scroll, "Scroll down one line"),
    info(A::ScrollUp, "scroll_up", Scroll, "Scroll up one line"),
    info(
        A::HalfPageDown,
        "half_page_down",
        Scroll,
        "Scroll down half a page",
    ),
    info(
        A::HalfPageUp,
        "half_page_up",
        Scroll,
        "Scroll up half a page",
    ),
    info(A::PageDown, "page_down", Scroll, "Scroll down a page"),
    info(A::PageUp, "page_up", Scroll, "Scroll up a page"),
    info(A::ScrollTop, "scroll_top", Scroll, "Jump to the top"),
    info(
        A::ScrollBottom,
        "scroll_bottom",
        Scroll,
        "Jump to the bottom",
    ),
    info(A::NextSection, "next_section", Navigate, "Next section tab"),
    info(
        A::PrevSection,
        "prev_section",
        Navigate,
        "Previous section tab",
    ),
    info(A::Section1, "section_1", Navigate, "Go to section 1"),
    info(A::Section2, "section_2", Navigate, "Go to section 2"),
    info(A::Section3, "section_3", Navigate, "Go to section 3"),
    info(A::Section4, "section_4", Navigate, "Go to section 4"),
    info(A::Section5, "section_5", Navigate, "Go to section 5"),
    info(A::Section6, "section_6", Navigate, "Go to section 6"),
    info(A::Section7, "section_7", Navigate, "Go to section 7"),
    info(A::Section8, "section_8", Navigate, "Go to section 8"),
    info(A::Section9, "section_9", Navigate, "Go to section 9"),
    info(
        A::NextProfile,
        "next_profile",
        Navigate,
        "Next agent / plugin",
    ),
    info(
        A::PrevProfile,
        "prev_profile",
        Navigate,
        "Previous agent / plugin",
    ),
    info(
        A::ToggleMode,
        "toggle_mode",
        Navigate,
        "Switch between agents and plugins",
    ),
    info(
        A::NextPeriod,
        "next_period",
        Navigate,
        "Next period (all / 7d / 30d)",
    ),
    info(A::PrevPeriod, "prev_period", Navigate, "Previous period"),
    info(A::Refresh, "refresh", Commands, "Refresh"),
    info(
        A::ToggleSettings,
        "toggle_settings",
        Commands,
        "Open / close settings",
    ),
    info(
        A::ToggleMore,
        "toggle_more",
        Commands,
        "Open / close the more menu",
    ),
    info(
        A::ToggleHelp,
        "toggle_help",
        Commands,
        "Show / hide this help",
    ),
    info(
        A::CloseOverlay,
        "close_overlay",
        Commands,
        "Close the open menu or panel",
    ),
    info(A::Dismiss, "dismiss", Commands, "Close the window"),
    info(A::Quit, "quit", Commands, "Quit Aura (tray icon included)"),
    info(A::OpenConfig, "open_config", Commands, "Edit config.toml"),
    info(A::OpenTheme, "open_theme", Commands, "Edit theme.toml"),
    info(
        A::OpenKeybindings,
        "open_keybindings",
        Commands,
        "Edit keybindings.toml",
    ),
    info(
        A::OpenUpdate,
        "open_update",
        Commands,
        "Open the update instructions",
    ),
    info(
        A::DismissUpdate,
        "dismiss_update",
        Commands,
        "Hide the update button",
    ),
    info(
        A::HintMode,
        "hint_mode",
        Commands,
        "Label plugin buttons to press them by key",
    ),
];

impl KeyAction {
    fn info(self) -> &'static ActionInfo {
        ACTIONS
            .iter()
            .find(|i| i.action == self)
            .expect("every KeyAction has an ACTIONS row")
    }

    /// Every action, in help-overlay order.
    pub fn all() -> impl Iterator<Item = KeyAction> {
        ACTIONS.iter().map(|i| i.action)
    }

    /// The name `keybindings.toml` uses, e.g. `"scroll_down"`.
    pub fn name(self) -> &'static str {
        self.info().name
    }

    pub fn group(self) -> ActionGroup {
        self.info().group
    }

    /// One-line description for the help overlay.
    pub fn description(self) -> &'static str {
        self.info().description
    }

    pub fn from_name(name: &str) -> Option<Self> {
        ACTIONS.iter().find(|i| i.name == name).map(|i| i.action)
    }
}

impl Serialize for KeyAction {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(self.name())
    }
}

impl fmt::Display for KeyAction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

// ── Contexts ─────────────────────────────────────────────────────────────────

/// Where a binding applies. `Overlay` is layered over `Global`: while an
/// overlay is open, an overlay binding wins over a global one on the same
/// keys, and global bindings the overlay doesn't mention still work.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BindingContext {
    Global,
    Overlay,
}

impl BindingContext {
    pub const ALL: [Self; 2] = [Self::Global, Self::Overlay];

    /// The table name in `keybindings.toml`.
    pub fn name(self) -> &'static str {
        match self {
            Self::Global => "global",
            Self::Overlay => "overlay",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|c| c.name() == name)
    }
}

// ── Keystrokes ───────────────────────────────────────────────────────────────

/// Named (non-character) keys the keymap accepts.
const NAMED_KEYS: &[&str] = &[
    "escape",
    "enter",
    "tab",
    "space",
    "backspace",
    "delete",
    "insert",
    "home",
    "end",
    "pageup",
    "pagedown",
    "up",
    "down",
    "left",
    "right",
];

/// Friendlier spellings, folded into the canonical key name.
const KEY_ALIASES: &[(&str, &str)] = &[
    ("esc", "escape"),
    ("return", "enter"),
    ("del", "delete"),
    ("ins", "insert"),
    ("pgup", "pageup"),
    ("pgdn", "pagedown"),
    ("pgdown", "pagedown"),
];

/// One keystroke: modifiers plus a key, in canonical form.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct Stroke {
    ctrl: bool,
    alt: bool,
    shift: bool,
    cmd: bool,
    func: bool,
    key: String,
}

impl Stroke {
    fn parse(source: &str) -> Result<Self, String> {
        let mut stroke = Stroke {
            ctrl: false,
            alt: false,
            shift: false,
            cmd: false,
            func: false,
            key: String::new(),
        };

        // A trailing `-` is the minus key itself (`ctrl--`, or bare `-`).
        let (mods, key) = if source == "-" {
            ("", "-")
        } else if let Some(mods) = source.strip_suffix("--") {
            (mods, "-")
        } else {
            match source.rsplit_once('-') {
                Some((mods, key)) => (mods, key),
                None => ("", source),
            }
        };

        if !mods.is_empty() {
            for m in mods.split('-') {
                match m.to_ascii_lowercase().as_str() {
                    "ctrl" | "control" => stroke.ctrl = true,
                    "alt" | "option" | "opt" => stroke.alt = true,
                    "shift" => stroke.shift = true,
                    "cmd" | "command" | "super" | "win" => stroke.cmd = true,
                    "fn" => stroke.func = true,
                    "secondary" => {
                        if cfg!(target_os = "macos") {
                            stroke.cmd = true;
                        } else {
                            stroke.ctrl = true;
                        }
                    }
                    "" => return Err(format!("`{source}` has an empty modifier")),
                    other => {
                        return Err(format!(
                            "`{source}`: `{other}` is not a modifier \
                             (expected ctrl, alt, shift, cmd, fn or secondary)"
                        ))
                    }
                }
            }
        }

        if key.is_empty() {
            return Err(format!("`{source}` has no key after its modifiers"));
        }

        let mut chars = key.chars();
        let single = matches!((chars.next(), chars.next()), (Some(_), None));
        stroke.key = if single {
            let c = key.chars().next().unwrap_or_default();
            if c.is_whitespace() {
                return Err(format!("`{source}`: write a space as `space`"));
            }
            if c.is_ascii_uppercase() {
                stroke.shift = true;
                c.to_ascii_lowercase().to_string()
            } else {
                c.to_string()
            }
        } else {
            let lower = key.to_ascii_lowercase();
            let lower = KEY_ALIASES
                .iter()
                .find(|(alias, _)| *alias == lower)
                .map(|(_, canonical)| canonical.to_string())
                .unwrap_or(lower);
            if !is_named_key(&lower) {
                return Err(format!("`{source}`: `{key}` is not a key name"));
            }
            lower
        };
        Ok(stroke)
    }

    /// The form the toolkit's parser reads back: `ctrl-alt-shift-cmd-fn-key`.
    fn canonical(&self) -> String {
        let mut out = String::new();
        for (on, name) in [
            (self.ctrl, "ctrl-"),
            (self.alt, "alt-"),
            (self.shift, "shift-"),
            (self.cmd, "cmd-"),
            (self.func, "fn-"),
        ] {
            if on {
                out.push_str(name);
            }
        }
        out.push_str(&self.key);
        out
    }

    /// Short form for humans: `G` rather than `shift-g`, `esc` for `escape`.
    fn display(&self) -> String {
        let bare_shift = self.shift && !(self.ctrl || self.alt || self.cmd || self.func);
        if bare_shift && self.key.len() == 1 && self.key.as_bytes()[0].is_ascii_lowercase() {
            return self.key.to_ascii_uppercase();
        }
        let key = if self.key == "escape" {
            "esc"
        } else {
            self.key.as_str()
        };
        let mut s = self.clone();
        s.key = key.to_string();
        s.canonical()
    }
}

fn is_named_key(key: &str) -> bool {
    if NAMED_KEYS.contains(&key) {
        return true;
    }
    key.strip_prefix('f')
        .and_then(|n| n.parse::<u8>().ok())
        .is_some_and(|n| (1..=24).contains(&n))
}

/// A whitespace-separated keystroke sequence, e.g. `g g`.
fn parse_sequence(source: &str) -> Result<Vec<Stroke>, String> {
    let strokes = source
        .split_whitespace()
        .map(Stroke::parse)
        .collect::<Result<Vec<_>, _>>()?;
    if strokes.is_empty() {
        return Err("an empty keystroke".to_string());
    }
    Ok(strokes)
}

fn join(strokes: &[Stroke], f: impl Fn(&Stroke) -> String) -> String {
    strokes.iter().map(f).collect::<Vec<_>>().join(" ")
}

/// A validated keystroke sequence in both of its spellings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ParsedKeys {
    /// What the toolkit parses and what two spellings compare equal on,
    /// e.g. `"shift-g"`.
    pub canonical: String,
    /// The short human form, e.g. `"G"`.
    pub display: String,
}

/// Validate a keystroke sequence such as `"ctrl-d"` or `"g g"`. The error
/// says what is wrong with it.
pub fn parse_keys(source: &str) -> Result<ParsedKeys, String> {
    let strokes = parse_sequence(source).map_err(|e| format!("invalid keystroke {e}"))?;
    Ok(ParsedKeys {
        canonical: join(&strokes, Stroke::canonical),
        display: join(&strokes, Stroke::display),
    })
}

/// Resolve an action name as written in `keybindings.toml`. `"none"` (or an
/// empty string) is an unbind, `Ok(None)`; an unknown name is an error with a
/// "did you mean" when one is close.
pub fn parse_action(name: &str) -> Result<Option<KeyAction>, String> {
    if is_unbind(name) {
        return Ok(None);
    }
    KeyAction::from_name(name).map(Some).ok_or_else(|| {
        let names: Vec<&str> = ACTIONS.iter().map(|i| i.name).collect();
        format!(
            "unknown action `{name}`{} (run `aura keys describe` for the list)",
            did_you_mean(name, &names)
        )
    })
}

/// A TOML item as the user wrote it, for messages.
fn item_text(item: &toml_edit::Item) -> String {
    one_line(item.to_string().trim())
}

fn one_line(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Keys the text-selection bridge already owns: copy, select-all, and
/// shift+arrow selection extension. Binding one of these takes it away.
fn reserved_reason(stroke: &Stroke) -> Option<&'static str> {
    let primary = stroke.ctrl || stroke.cmd;
    if primary && !stroke.alt && !stroke.shift {
        match stroke.key.as_str() {
            "c" => return Some("copies selected text"),
            "a" => return Some("selects all text"),
            _ => {}
        }
    }
    if stroke.shift && matches!(stroke.key.as_str(), "up" | "down" | "left" | "right") {
        return Some("extends a text selection");
    }
    None
}

// ── Keymap ───────────────────────────────────────────────────────────────────

/// Where a binding came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BindingSource {
    Default,
    User,
}

/// One resolved binding.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Binding {
    pub context: BindingContext,
    /// Canonical keystrokes, e.g. `"shift-g"` or `"g g"`, in the form the
    /// toolkit's keystroke parser accepts.
    pub keys: String,
    /// The same keystrokes written for humans, e.g. `"G"`.
    pub display: String,
    /// `None` is an explicit unbind (`"x" = "none"`): the keys do nothing in
    /// this context, even where a global binding would otherwise apply.
    pub action: Option<KeyAction>,
    pub source: BindingSource,
}

/// A problem found while loading `keybindings.toml`. The offending entry is
/// skipped; everything else still applies.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct KeymapWarning {
    pub message: String,
}

impl fmt::Display for KeymapWarning {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

/// The effective keymap: defaults merged with the user file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Keymap {
    pub bindings: Vec<Binding>,
    pub warnings: Vec<KeymapWarning>,
}

/// The built-in bindings, as `(context, keys, action)`.
const DEFAULTS: &[(BindingContext, &str, KeyAction)] = {
    use BindingContext::{Global as G, Overlay as O};
    &[
        (G, "j", A::ScrollDown),
        (G, "down", A::ScrollDown),
        (G, "k", A::ScrollUp),
        (G, "up", A::ScrollUp),
        (G, "ctrl-d", A::HalfPageDown),
        (G, "ctrl-u", A::HalfPageUp),
        (G, "ctrl-f", A::PageDown),
        (G, "pagedown", A::PageDown),
        (G, "ctrl-b", A::PageUp),
        (G, "pageup", A::PageUp),
        (G, "g g", A::ScrollTop),
        (G, "home", A::ScrollTop),
        (G, "G", A::ScrollBottom),
        (G, "end", A::ScrollBottom),
        (G, "l", A::NextSection),
        (G, "tab", A::NextSection),
        (G, "g t", A::NextSection),
        (G, "h", A::PrevSection),
        (G, "shift-tab", A::PrevSection),
        (G, "g T", A::PrevSection),
        (G, "1", A::Section1),
        (G, "2", A::Section2),
        (G, "3", A::Section3),
        (G, "4", A::Section4),
        (G, "5", A::Section5),
        (G, "6", A::Section6),
        (G, "7", A::Section7),
        (G, "8", A::Section8),
        (G, "9", A::Section9),
        (G, "L", A::NextProfile),
        (G, "]", A::NextProfile),
        (G, "H", A::PrevProfile),
        (G, "[", A::PrevProfile),
        (G, "m", A::ToggleMode),
        (G, "p", A::NextPeriod),
        (G, "P", A::PrevPeriod),
        (G, "r", A::Refresh),
        (G, "f5", A::Refresh),
        (G, ",", A::ToggleSettings),
        (G, "secondary-,", A::ToggleSettings),
        (G, ".", A::ToggleMore),
        (G, "?", A::ToggleHelp),
        (G, "q", A::Dismiss),
        (G, "escape", A::Dismiss),
        (G, "e", A::OpenConfig),
        (G, "t", A::OpenTheme),
        (G, "u", A::OpenUpdate),
        (G, "U", A::DismissUpdate),
        (G, "f", A::HintMode),
        (O, "escape", A::CloseOverlay),
    ]
};

impl Keymap {
    /// Default on-disk location: `$XDG_CONFIG_HOME/aura/keybindings.toml`.
    pub fn default_path() -> PathBuf {
        dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("~/.config"))
            .join("aura")
            .join("keybindings.toml")
    }

    /// The built-in keymap with no user overrides.
    pub fn defaults() -> Self {
        let bindings = DEFAULTS
            .iter()
            .map(|(context, keys, action)| {
                let strokes = parse_sequence(keys).expect("built-in keystrokes parse");
                Binding {
                    context: *context,
                    keys: join(&strokes, Stroke::canonical),
                    display: join(&strokes, Stroke::display),
                    action: Some(*action),
                    source: BindingSource::Default,
                }
            })
            .collect();
        Self {
            bindings,
            warnings: Vec::new(),
        }
    }

    /// Load `path` over the defaults. A missing file is the defaults; an
    /// unreadable or malformed one is the defaults plus a warning.
    pub fn load(path: &Path) -> Self {
        if !path.exists() {
            return Self::defaults();
        }
        match fs::read_to_string(path) {
            Ok(content) => Self::from_toml(&content),
            Err(e) => {
                let mut map = Self::defaults();
                map.warn(format!(
                    "could not read {} ({e}); using the default keybindings",
                    path.display()
                ));
                map
            }
        }
    }

    /// Parse a `keybindings.toml` body over the defaults.
    pub fn from_toml(content: &str) -> Self {
        // `toml_edit` rather than `toml`: it keeps the file's order, so
        // warnings read top to bottom and "the later one wins" means what it
        // says.
        let doc: toml_edit::DocumentMut = match content.parse() {
            Ok(d) => d,
            Err(e) => {
                let mut map = Self::defaults();
                map.warn(format!(
                    "keybindings.toml is not valid TOML, so none of it was applied; \
                     using the default keybindings. {}",
                    one_line(&e.to_string())
                ));
                return map;
            }
        };
        let root = doc.as_table();

        let mut warnings = Vec::new();
        let use_defaults = match root.get("use_defaults") {
            None => true,
            Some(item) => match item.as_bool() {
                Some(b) => b,
                None => {
                    warnings.push(format!(
                        "use_defaults must be true or false, not {}; keeping the defaults",
                        item_text(item)
                    ));
                    true
                }
            },
        };

        let mut map = if use_defaults {
            Self::defaults()
        } else {
            Self {
                bindings: Vec::new(),
                warnings: Vec::new(),
            }
        };
        for w in warnings {
            map.warn(w);
        }

        for (key, value) in root.iter() {
            if key == "use_defaults" {
                continue;
            }
            let Some(context) = BindingContext::from_name(key) else {
                let names: Vec<&str> = BindingContext::ALL.iter().map(|c| c.name()).collect();
                map.warn(format!(
                    "unknown table [{key}]{}; ignoring it (contexts are [global] and [overlay])",
                    did_you_mean(key, &names)
                ));
                continue;
            };
            let Some(table) = value.as_table_like() else {
                map.warn(format!(
                    "`{key}` must be a table of \"keys\" = \"action\" pairs; ignoring it"
                ));
                continue;
            };
            map.apply_user_table(context, table);
        }

        map.check_prefixes();
        map
    }

    fn apply_user_table(&mut self, context: BindingContext, table: &dyn toml_edit::TableLike) {
        let section = context.name();
        // Canonical keys this table has already bound, to catch two spellings
        // of the same keystroke (`G` and `shift-g`).
        let mut seen: HashSet<String> = HashSet::new();

        for (raw_keys, value) in table.iter() {
            let strokes = match parse_sequence(raw_keys) {
                Ok(s) => s,
                Err(e) => {
                    self.warn(format!("[{section}] invalid keystroke {e}; ignoring it"));
                    continue;
                }
            };
            let keys = join(&strokes, Stroke::canonical);

            let Some(name) = value.as_str() else {
                self.warn(format!(
                    "[{section}] \"{raw_keys}\": expected an action name in quotes \
                     (or \"none\" to unbind), found {}; ignoring it",
                    item_text(value)
                ));
                continue;
            };
            let action = match parse_action(name) {
                Ok(a) => a,
                Err(e) => {
                    self.warn(format!("[{section}] \"{raw_keys}\": {e}; ignoring it"));
                    continue;
                }
            };

            if !seen.insert(keys.clone()) {
                self.warn(format!(
                    "[{section}] \"{raw_keys}\" is the same keystroke as another entry in this \
                     table; the later one wins"
                ));
            }

            if let (Some(action), Some(reason)) = (action, reserved_reason(&strokes[0])) {
                self.warn(format!(
                    "[{section}] \"{raw_keys}\" = \"{action}\" takes over {} (it {reason})",
                    strokes[0].display()
                ));
            }

            let replaced = self.remove(context, &keys);
            if action.is_none() && !replaced && !self.bound_below(context, &keys) {
                self.warn(format!(
                    "[{section}] \"{raw_keys}\" = \"none\" unbinds nothing: that keystroke has \
                     no binding here"
                ));
            }

            self.bindings.push(Binding {
                context,
                display: join(&strokes, Stroke::display),
                keys,
                action,
                source: BindingSource::User,
            });
        }
    }

    /// Drop any binding for `keys` in `context`. Returns whether one existed.
    fn remove(&mut self, context: BindingContext, keys: &str) -> bool {
        let before = self.bindings.len();
        self.bindings
            .retain(|b| !(b.context == context && b.keys == keys));
        self.bindings.len() != before
    }

    /// Whether a context layered under `context` binds `keys` — the one case
    /// where an unbind in `context` does something without replacing an entry.
    fn bound_below(&self, context: BindingContext, keys: &str) -> bool {
        context == BindingContext::Overlay
            && self.bindings.iter().any(|b| {
                b.context == BindingContext::Global && b.keys == keys && b.action.is_some()
            })
    }

    /// Warn when one binding's keys are a prefix of another's in the same
    /// effective context. The shorter one still works, but only after the
    /// toolkit gives up waiting for the rest of the sequence (about a second).
    fn check_prefixes(&mut self) {
        let mut found = Vec::new();
        for context in BindingContext::ALL {
            let active = self.active_in(context);
            for short in &active {
                for long in &active {
                    // In the overlay, only report pairs the overlay table is
                    // part of; purely global pairs were reported already.
                    if context == BindingContext::Overlay
                        && short.context == BindingContext::Global
                        && long.context == BindingContext::Global
                    {
                        continue;
                    }
                    let long_prefix = format!("{} ", short.keys);
                    if long.keys.starts_with(&long_prefix) {
                        found.push(format!(
                            "[{}] \"{}\" ({}) is the start of \"{}\" ({}): pressing {} waits about \
                             a second for the next key before running {}",
                            short.context.name(),
                            short.display,
                            fmt_action(short.action),
                            long.display,
                            fmt_action(long.action),
                            short.display,
                            fmt_action(short.action),
                        ));
                    }
                }
            }
        }
        for w in found {
            self.warn(w);
        }
    }

    /// Bindings that fire in `context`: its own, plus global ones it doesn't
    /// override. Unbinds are left out.
    fn active_in(&self, context: BindingContext) -> Vec<&Binding> {
        let own: Vec<&Binding> = self
            .bindings
            .iter()
            .filter(|b| b.context == context)
            .collect();
        let mut out: Vec<&Binding> = own.iter().copied().filter(|b| b.action.is_some()).collect();
        if context == BindingContext::Overlay {
            out.extend(self.bindings.iter().filter(|b| {
                b.context == BindingContext::Global
                    && b.action.is_some()
                    && !own.iter().any(|o| o.keys == b.keys)
            }));
        }
        out
    }

    fn warn(&mut self, message: String) {
        self.warnings.push(KeymapWarning { message });
    }

    /// Display strings for every binding that runs `action` in `context`.
    pub fn keys_for(&self, action: KeyAction, context: BindingContext) -> Vec<&str> {
        self.bindings
            .iter()
            .filter(|b| b.context == context && b.action == Some(action))
            .map(|b| b.display.as_str())
            .collect()
    }

    /// Default display keys for `action` in `context` (empty when the
    /// built-in keymap leaves it unbound).
    pub fn default_keys(action: KeyAction, context: BindingContext) -> Vec<String> {
        DEFAULTS
            .iter()
            .filter(|(c, _, a)| *c == context && *a == action)
            .filter_map(|(_, keys, _)| parse_keys(keys).ok())
            .map(|k| k.display)
            .collect()
    }

    /// What pressing `canonical` does in `context`: the context's own binding,
    /// or — in the overlay — the global one it doesn't override. `None` when
    /// nothing is bound; `Some(b)` with `b.action == None` for an unbind.
    pub fn lookup(&self, context: BindingContext, canonical: &str) -> Option<&Binding> {
        let own = |c: BindingContext| {
            self.bindings
                .iter()
                .rev()
                .find(|b| b.context == c && b.keys == canonical)
        };
        own(context).or_else(|| match context {
            BindingContext::Overlay => own(BindingContext::Global),
            BindingContext::Global => None,
        })
    }

    /// Starter `keybindings.toml`: how the file works, and every default
    /// commented out so it reads as a reference without overriding anything.
    /// Generated from [`DEFAULTS`] so it can never drift from them.
    pub fn default_file_contents() -> String {
        render_file(None, true, &[], true)
    }

    /// This keymap as a self-contained `keybindings.toml`: `use_defaults =
    /// false` and every binding written out, so the file means the same thing
    /// whatever a later release changes in the defaults.
    pub fn to_explicit_toml(&self) -> String {
        let entries: Vec<RenderEntry> = self
            .bindings
            .iter()
            // With no defaults underneath, a global unbind has nothing to mask.
            .filter(|b| b.action.is_some() || b.context == BindingContext::Overlay)
            .map(|b| RenderEntry {
                context: b.context,
                keys: b.display.clone(),
                action: b.action,
            })
            .collect();
        render_file(
            Some("The complete keymap, written by `aura keys export` / `aura keys init --full`."),
            false,
            &entries,
            false,
        )
    }
}

/// One `"keys" = "action"` line for [`render_file`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RenderEntry {
    pub context: BindingContext,
    /// Keystrokes as they should be written.
    pub keys: String,
    pub action: Option<KeyAction>,
}

const FILE_HEADER: &str = "\
# ~/.config/aura/keybindings.toml — keyboard shortcuts for the Aura modal.
#
# Entries here are layered over the built-in defaults: bind a key to add or
# replace a shortcut, or bind it to \"none\" to remove one. Set
# `use_defaults = false` to start from an empty keymap instead.
#
# Keystrokes: modifiers joined with `-`, then the key (`ctrl-d`, `shift-tab`,
# `secondary-,` — secondary is cmd on macOS, ctrl elsewhere). A space starts
# the next stroke of a sequence (`g g`). `G` means `shift-g`.
#
# [global] applies everywhere; [overlay] is checked first while a menu,
# the settings panel or the help overlay is open.
#
# Turn every shortcut off with `[keybindings] enabled = false` in config.toml.
# `aura keys describe` lists every action, `aura keys set` / `wizard` edit this
# file, and `aura keys validate` checks it. Changes apply the next time the
# window opens, or on refresh.
";

/// Render a `keybindings.toml`: the explanatory header, `use_defaults`, then
/// one table per context with an aligned `# description` on every entry and,
/// when `reference` is set, the defaults listed as comments.
pub(crate) fn render_file(
    note: Option<&str>,
    use_defaults: bool,
    entries: &[RenderEntry],
    reference: bool,
) -> String {
    let mut out = String::from(FILE_HEADER);
    if let Some(note) = note {
        out.push_str(&format!("#\n# {note}\n"));
    }
    out.push_str(&format!("\nuse_defaults = {use_defaults}\n"));

    for context in BindingContext::ALL {
        out.push_str(&format!("\n[{}]\n", context.name()));

        let rows: Vec<(String, String, &str)> = entries
            .iter()
            .filter(|e| e.context == context)
            .map(|e| {
                (
                    toml_key(&e.keys),
                    format!("\"{}\"", fmt_action(e.action)),
                    e.action.map(KeyAction::description).unwrap_or("unbound"),
                )
            })
            .collect();
        let kw = rows.iter().map(|r| r.0.len()).max().unwrap_or(0);
        let aw = rows.iter().map(|r| r.1.len()).max().unwrap_or(0);
        for (key, action, description) in &rows {
            out.push_str(&format!("{key:<kw$} = {action:<aw$}  # {description}\n"));
        }

        if reference && use_defaults {
            let defaults: Vec<_> = DEFAULTS.iter().filter(|(c, _, _)| *c == context).collect();
            if !rows.is_empty() {
                out.push('\n');
            }
            out.push_str("# Defaults:\n");
            let width = defaults
                .iter()
                .map(|(_, keys, _)| toml_key(keys).len())
                .max()
                .unwrap_or(0);
            for (_, keys, action) in defaults {
                out.push_str(&format!("# {:<width$} = \"{action}\"\n", toml_key(keys)));
            }
        }
    }
    out
}

/// `keys` as a double-quoted TOML key. Always quoted, so `"j"` and `"g g"`
/// line up the same way.
fn toml_key(keys: &str) -> String {
    format!("\"{}\"", keys.replace('\\', "\\\\").replace('"', "\\\""))
}

fn is_unbind(s: &str) -> bool {
    s.is_empty() || s.eq_ignore_ascii_case("none")
}

fn fmt_action(action: Option<KeyAction>) -> &'static str {
    action.map(KeyAction::name).unwrap_or("none")
}

/// ` (did you mean `x`?)` for the closest candidate within a small edit
/// distance, or an empty string.
fn did_you_mean(input: &str, candidates: &[&str]) -> String {
    candidates
        .iter()
        .map(|c| (levenshtein(input, c), *c))
        .filter(|(d, c)| *d <= 2.max(c.len() / 4))
        .min_by_key(|(d, _)| *d)
        .map(|(_, c)| format!(" (did you mean `{c}`?)"))
        .unwrap_or_default()
}

fn levenshtein(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut cur = vec![i + 1; b.len() + 1];
        for (j, cb) in b.iter().enumerate() {
            let cost = usize::from(ca != *cb);
            cur[j + 1] = (prev[j] + cost).min(prev[j + 1] + 1).min(cur[j] + 1);
        }
        prev = cur;
    }
    prev[b.len()]
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn action_at(map: &Keymap, context: BindingContext, keys: &str) -> Option<Option<KeyAction>> {
        map.bindings
            .iter()
            .find(|b| b.context == context && b.keys == keys)
            .map(|b| b.action)
    }

    fn messages(map: &Keymap) -> String {
        map.warnings
            .iter()
            .map(|w| w.message.clone())
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn every_action_has_a_unique_name() {
        let names: HashSet<&str> = ACTIONS.iter().map(|i| i.name).collect();
        assert_eq!(names.len(), ACTIONS.len());
        for a in KeyAction::all() {
            assert_eq!(KeyAction::from_name(a.name()), Some(a));
        }
    }

    #[test]
    fn defaults_are_clean() {
        let map = Keymap::defaults();
        assert!(map.warnings.is_empty());
        // No accidental duplicates or prefix clashes in the shipped keymap.
        let mut seen = HashSet::new();
        for b in &map.bindings {
            assert!(seen.insert((b.context, b.keys.clone())), "dup {}", b.keys);
        }
        let mut checked = map.clone();
        checked.check_prefixes();
        assert!(checked.warnings.is_empty(), "{}", messages(&checked));
    }

    #[test]
    fn default_file_is_a_no_op() {
        let map = Keymap::from_toml(&Keymap::default_file_contents());
        assert!(map.warnings.is_empty(), "{}", messages(&map));
        assert_eq!(map.bindings, Keymap::defaults().bindings);
    }

    #[test]
    fn uppercase_is_shift() {
        let map = Keymap::defaults();
        assert_eq!(
            action_at(&map, BindingContext::Global, "shift-g"),
            Some(Some(KeyAction::ScrollBottom))
        );
        let b = map.bindings.iter().find(|b| b.keys == "shift-g").unwrap();
        assert_eq!(b.display, "G");
        assert_eq!(
            action_at(&map, BindingContext::Global, "g shift-t"),
            Some(Some(KeyAction::PrevSection))
        );
    }

    #[test]
    fn keystroke_parsing() {
        let c = |s: &str| parse_sequence(s).map(|v| join(&v, Stroke::canonical));
        assert_eq!(c("Ctrl-Shift-X").unwrap(), "ctrl-shift-x");
        assert_eq!(c("shift-ctrl-x").unwrap(), "ctrl-shift-x");
        assert_eq!(c("esc").unwrap(), "escape");
        assert_eq!(c("ctrl--").unwrap(), "ctrl--");
        assert_eq!(c("-").unwrap(), "-");
        assert_eq!(c("f12").unwrap(), "f12");
        assert_eq!(c("  g   g ").unwrap(), "g g");
        assert!(c("hyper-x").is_err());
        assert!(c("ctrl-").is_err());
        assert!(c("ctrl-escapee").is_err());
        assert!(c("f99").is_err());
        assert!(c("").is_err());
        let secondary = c("secondary-s").unwrap();
        if cfg!(target_os = "macos") {
            assert_eq!(secondary, "cmd-s");
        } else {
            assert_eq!(secondary, "ctrl-s");
        }
    }

    #[test]
    fn user_rebinds_adds_and_unbinds() {
        let map = Keymap::from_toml(
            r#"
[global]
"x" = "refresh"
"j" = "scroll_up"
"t" = "none"
"ctrl-j" = "scroll_down"
"#,
        );
        assert!(map.warnings.is_empty(), "{}", messages(&map));
        let g = BindingContext::Global;
        assert_eq!(action_at(&map, g, "x"), Some(Some(KeyAction::Refresh)));
        assert_eq!(action_at(&map, g, "j"), Some(Some(KeyAction::ScrollUp)));
        assert_eq!(action_at(&map, g, "t"), Some(None));
        assert_eq!(
            action_at(&map, g, "ctrl-j"),
            Some(Some(KeyAction::ScrollDown))
        );
        // Untouched defaults survive, and `r` still refreshes alongside `x`.
        assert_eq!(action_at(&map, g, "r"), Some(Some(KeyAction::Refresh)));
        assert_eq!(map.keys_for(KeyAction::Refresh, g), vec!["r", "f5", "x"]);
    }

    #[test]
    fn use_defaults_false_starts_empty() {
        let map = Keymap::from_toml(
            r#"
use_defaults = false
[global]
"j" = "scroll_down"
"#,
        );
        assert!(map.warnings.is_empty(), "{}", messages(&map));
        assert_eq!(map.bindings.len(), 1);
    }

    #[test]
    fn overlay_unbind_masks_a_global_binding() {
        let map = Keymap::from_toml(
            r#"
[overlay]
"q" = "none"
"#,
        );
        assert!(map.warnings.is_empty(), "{}", messages(&map));
        assert_eq!(action_at(&map, BindingContext::Overlay, "q"), Some(None));
    }

    #[test]
    fn warns_on_bad_entries_and_keeps_the_rest() {
        let map = Keymap::from_toml(
            r#"
use_defaults = "yes"

[globl]
"x" = "refresh"

[global]
"ctrl-hyper-x" = "refresh"
"y" = "scrol_down"
"z" = 3
"G" = "scroll_top"
"shift-g" = "scroll_bottom"
"ctrl-c" = "refresh"
"F9" = "none"
"n" = "refresh"
"#,
        );
        let msg = messages(&map);
        assert!(msg.contains("use_defaults must be true or false"), "{msg}");
        assert!(
            msg.contains("unknown table [globl] (did you mean `global`?)"),
            "{msg}"
        );
        assert!(msg.contains("`hyper` is not a modifier"), "{msg}");
        assert!(
            msg.contains("unknown action `scrol_down` (did you mean `scroll_down`?)"),
            "{msg}"
        );
        assert!(msg.contains("expected an action name"), "{msg}");
        assert!(msg.contains("same keystroke as another entry"), "{msg}");
        assert!(msg.contains("copies selected text"), "{msg}");
        assert!(msg.contains("unbinds nothing"), "{msg}");
        // The valid entry still landed.
        assert_eq!(
            action_at(&map, BindingContext::Global, "n"),
            Some(Some(KeyAction::Refresh))
        );
    }

    #[test]
    fn warns_on_prefix_conflicts() {
        let map = Keymap::from_toml(
            r#"
[global]
"g" = "refresh"
"#,
        );
        let msg = messages(&map);
        assert!(
            msg.contains("\"g\" (refresh) is the start of \"g g\""),
            "{msg}"
        );
    }

    #[test]
    fn invalid_toml_falls_back_to_defaults() {
        let map = Keymap::from_toml("[global\n\"j\" = ");
        assert_eq!(map.bindings, Keymap::defaults().bindings);
        assert_eq!(map.warnings.len(), 1);
        assert!(map.warnings[0].message.contains("not valid TOML"));
    }

    #[test]
    fn missing_file_is_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let map = Keymap::load(&dir.path().join("absent.toml"));
        assert_eq!(map, Keymap::defaults());
    }
}
