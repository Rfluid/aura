/// Set the macOS NSApplication activation policy.
///
/// `show = true`  → NSApplicationActivationPolicyRegular   (appears in Cmd+Tab)
/// `show = false` → NSApplicationActivationPolicyAccessory (background-only, menu-bar only)
#[cfg(target_os = "macos")]
pub fn apply_app_switcher_policy(show: bool) {
    use cocoa::appkit::NSApplicationActivationPolicy::{
        NSApplicationActivationPolicyAccessory, NSApplicationActivationPolicyRegular,
    };
    use objc::{class, msg_send, sel, sel_impl};

    unsafe {
        let app: cocoa::base::id = msg_send![class!(NSApplication), sharedApplication];
        let policy = if show {
            NSApplicationActivationPolicyRegular
        } else {
            NSApplicationActivationPolicyAccessory
        };
        let _: () = msg_send![app, setActivationPolicy: policy];
    }
}

#[cfg(not(target_os = "macos"))]
pub fn apply_app_switcher_policy(_show: bool) {}
