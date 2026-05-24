//! Thin per-OS façade for the things Aura asks of the host: tweaking the
//! macOS activation policy and asking the desktop to open a file/URL with
//! the user's default handler.
//!
//! The `open_*` functions return immediately and do the actual work on a
//! detached thread — the click handlers that call them run on the GPUI
//! main thread and shouldn't block on a subprocess.
//!
//! ## Fallback semantics
//!
//! `open_path()` first tries the OS default handler (`xdg-open` / `open` /
//! `ShellExecuteW`). If that handler reports failure — common on Linux when
//! the MIME type has no association, or on Windows when a path has no
//! registered verb — it falls back to revealing the path in the system file
//! manager (`org.freedesktop.FileManager1.ShowItems` over D-Bus on Linux,
//! `open -R` on macOS, `explorer /select,` on Windows). That way a
//! freshly-installed system without a TOML editor still lands the user
//! somewhere they can act on the file, instead of a silent no-op.

use std::path::Path;

// ── macOS activation policy ──────────────────────────────────────────────────

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

// ── Open with system handler (file or URL) ──────────────────────────────────

/// Open a filesystem path with the desktop's default handler, falling back to
/// the file manager (with the file pre-selected when supported) if no handler
/// is registered.
pub fn open_path(path: &Path) {
    let path = path.to_path_buf();
    std::thread::Builder::new()
        .name("aura-open-path".into())
        .spawn(move || {
            if let Err(e) = open_path_blocking(&path) {
                eprintln!(
                    "aura: open_path({}) failed ({e}); falling back to file manager",
                    path.display()
                );
                if let Err(e2) = reveal_in_file_manager_blocking(&path) {
                    eprintln!(
                        "aura: reveal_in_file_manager({}) also failed: {e2}",
                        path.display()
                    );
                }
            }
        })
        .ok();
}

/// Open a URL with the desktop's default browser. Unlike `open_path`, there is
/// no file-manager fallback — if the browser launch fails we just log.
pub fn open_url(url: &str) {
    let url = url.to_owned();
    std::thread::Builder::new()
        .name("aura-open-url".into())
        .spawn(move || {
            if let Err(e) = open_url_blocking(&url) {
                eprintln!("aura: open_url({url}) failed: {e}");
            }
        })
        .ok();
}

// ── Per-OS blocking impls ────────────────────────────────────────────────────

fn open_path_blocking(path: &Path) -> std::io::Result<()> {
    #[cfg(target_os = "macos")]
    {
        run_status("open", &[path.as_os_str()])
    }
    #[cfg(target_os = "windows")]
    {
        shell_execute_open(path)
    }
    #[cfg(all(not(target_os = "macos"), not(target_os = "windows")))]
    {
        run_status("xdg-open", &[path.as_os_str()])
    }
}

fn open_url_blocking(url: &str) -> std::io::Result<()> {
    #[cfg(target_os = "macos")]
    {
        run_status("open", &[std::ffi::OsStr::new(url)])
    }
    #[cfg(target_os = "windows")]
    {
        shell_execute_open(Path::new(url))
    }
    #[cfg(all(not(target_os = "macos"), not(target_os = "windows")))]
    {
        run_status("xdg-open", &[std::ffi::OsStr::new(url)])
    }
}

fn reveal_in_file_manager_blocking(path: &Path) -> std::io::Result<()> {
    #[cfg(target_os = "macos")]
    {
        // `open -R` highlights the target in Finder rather than opening it.
        run_status("open", &[std::ffi::OsStr::new("-R"), path.as_os_str()])
    }
    #[cfg(target_os = "windows")]
    {
        // `explorer /select,<path>` opens Explorer with the file selected.
        // explorer.exe always returns exit code 1 on success; treat any
        // spawn that *runs* as success.
        let _ = std::process::Command::new("explorer.exe")
            .arg(format!("/select,{}", path.display()))
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()?
            .wait()?;
        Ok(())
    }
    #[cfg(all(not(target_os = "macos"), not(target_os = "windows")))]
    {
        linux_reveal(path)
    }
}

// Generic Command::status helper that maps non-success exit codes to an Err
// so callers can detect "no handler registered for this MIME type" the way
// `xdg-open` reports it (exit code 3).
#[cfg(not(target_os = "windows"))]
fn run_status(program: &str, args: &[&std::ffi::OsStr]) -> std::io::Result<()> {
    let status = std::process::Command::new(program)
        .args(args)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()?;
    if status.success() {
        Ok(())
    } else {
        Err(std::io::Error::other(format!(
            "{program} exited with {status}"
        )))
    }
}

#[cfg(target_os = "windows")]
fn shell_execute_open(path: &Path) -> std::io::Result<()> {
    use windows::core::{w, HSTRING};
    use windows::Win32::UI::Shell::ShellExecuteW;
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

    let target = HSTRING::from(path);
    let ret = unsafe { ShellExecuteW(None, w!("open"), &target, None, None, SW_SHOWNORMAL) };
    // Per the MSDN docs, return values ≤32 are error codes; anything larger
    // is the handle of the launched application/document.
    if ret.0 as isize > 32 {
        Ok(())
    } else {
        Err(std::io::Error::other(format!(
            "ShellExecuteW returned {}",
            ret.0 as isize
        )))
    }
}

// ── Linux file manager reveal ───────────────────────────────────────────────
//
// The org.freedesktop.FileManager1 D-Bus interface is implemented by GNOME
// Files (Nautilus), KDE Dolphin, Cinnamon Nemo, MATE Caja, and Pantheon
// Files — i.e. the file manager that ships with every mainstream desktop.
// We shell out to `dbus-send` instead of pulling in a D-Bus crate to keep
// the dep footprint flat (ksni already gives us all the D-Bus we want at
// runtime, but its connection is private). If the D-Bus call fails (no
// FileManager1 owner, or the binary is missing), we fall back to opening
// the parent directory with xdg-open so the user at least lands close to
// the target.

#[cfg(all(not(target_os = "macos"), not(target_os = "windows")))]
fn linux_reveal(path: &Path) -> std::io::Result<()> {
    if file_manager_1_show(path).is_ok() {
        return Ok(());
    }
    let parent = file_manager_target_dir(path);
    run_status("xdg-open", &[parent.as_os_str()])
}

#[cfg(all(not(target_os = "macos"), not(target_os = "windows")))]
fn file_manager_1_show(path: &Path) -> std::io::Result<()> {
    let uri = format!("file://{}", path.display());
    let status = std::process::Command::new("dbus-send")
        .args([
            "--session",
            "--dest=org.freedesktop.FileManager1",
            "--type=method_call",
            "/org/freedesktop/FileManager1",
            "org.freedesktop.FileManager1.ShowItems",
        ])
        .arg(format!("array:string:{uri}"))
        .arg("string:")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()?;
    if status.success() {
        Ok(())
    } else {
        Err(std::io::Error::other(format!(
            "FileManager1.ShowItems exited with {status}"
        )))
    }
}

#[cfg(all(not(target_os = "macos"), not(target_os = "windows")))]
fn file_manager_target_dir(path: &Path) -> std::path::PathBuf {
    if path.is_dir() {
        path.to_path_buf()
    } else {
        path.parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| std::path::PathBuf::from("/"))
    }
}
