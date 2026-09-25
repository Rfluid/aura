//! Bridges the toolkit-agnostic keymap in `aura_core::keymap` to GPUI: one
//! GPUI action per [`KeyAction`], the listeners that route them to
//! [`AuraView::run_key_action`], and installing the resolved bindings.
//!
//! Contexts map onto GPUI key contexts set on the modal's root element:
//! `[global]` bindings match `Aura`, `[overlay]` bindings match `overlay`,
//! which the root adds while a menu, panel or the help overlay is open. Both
//! live on the same element, so they tie on depth and GPUI falls back to
//! insertion order — which is why overlay bindings are installed last.

use std::{rc::Rc, sync::Mutex};

use aura_core::keymap::{BindingContext, KeyAction, Keymap, KeymapWarning};
use gpui::{
    actions, App, Context, DummyKeyboardMapper, InteractiveElement, KeyBinding,
    KeyBindingContextPredicate, KeyContext, NoAction,
};

use crate::app::AuraView;

/// Key context identifier for the modal as a whole.
pub const CONTEXT_ROOT: &str = "Aura";
/// Key context identifier added while an overlay is open.
pub const CONTEXT_OVERLAY: &str = "overlay";

/// Declares a GPUI action per [`KeyAction`] variant (same name) plus the two
/// exhaustive mappings between them, so a new variant can't be forgotten.
macro_rules! key_actions {
    ($($name:ident),* $(,)?) => {
        actions!(aura, [$($name),*]);

        fn boxed(action: KeyAction) -> Box<dyn gpui::Action> {
            match action {
                $(KeyAction::$name => Box::new($name),)*
            }
        }

        /// Attach a listener for every keymap action to `el`.
        pub fn listen<E: InteractiveElement>(el: E, cx: &mut Context<AuraView>) -> E {
            el $(.on_action(cx.listener(|view: &mut AuraView, _: &$name, window, cx| {
                view.run_key_action(KeyAction::$name, window, cx)
            })))*
        }
    };
}

key_actions!(
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
);

/// The key context the modal's root element carries.
pub fn root_context(overlay_open: bool) -> KeyContext {
    let mut context = KeyContext::new_with_defaults();
    context.add(CONTEXT_ROOT);
    if overlay_open {
        context.add(CONTEXT_OVERLAY);
    }
    context
}

fn predicate(context: BindingContext) -> &'static str {
    match context {
        BindingContext::Global => CONTEXT_ROOT,
        BindingContext::Overlay => CONTEXT_OVERLAY,
    }
}

/// Replace the app's key bindings with `keymap`, or with nothing when
/// `enabled` is false. Warnings go to stderr, once per distinct set, since
/// this runs on every open and every refresh.
pub fn install(cx: &mut App, keymap: &Keymap, enabled: bool) {
    cx.clear_key_bindings();
    crate::runtime::set_keybindings_active(enabled);
    if !enabled {
        return;
    }
    report(&keymap.warnings);

    let mut bindings = Vec::with_capacity(keymap.bindings.len());
    // Global first: overlay bindings must come later to win their ties.
    for context in BindingContext::ALL {
        let predicate = KeyBindingContextPredicate::parse(predicate(context))
            .ok()
            .map(Rc::new);
        for binding in keymap.bindings.iter().filter(|b| b.context == context) {
            // An unbind masks whatever a lower context binds to these keys.
            let action = binding
                .action
                .map(boxed)
                .unwrap_or_else(|| Box::new(NoAction));
            match KeyBinding::load(
                &binding.keys,
                action,
                predicate.clone(),
                false,
                None,
                &DummyKeyboardMapper,
            ) {
                Ok(b) => bindings.push(b),
                Err(e) => eprintln!("aura: keybindings.toml: {e}"),
            }
        }
    }
    cx.bind_keys(bindings);
}

fn report(warnings: &[KeymapWarning]) {
    static LAST: Mutex<Vec<KeymapWarning>> = Mutex::new(Vec::new());
    let Ok(mut last) = LAST.lock() else {
        return;
    };
    if last.as_slice() == warnings {
        return;
    }
    for w in warnings {
        eprintln!("aura: keybindings.toml: {w}");
    }
    *last = warnings.to_vec();
}
