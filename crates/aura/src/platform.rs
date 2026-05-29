// The `objc` 0.2 crate's macros (msg_send!, class!, sel_impl!) emit an
// internal `cargo-clippy` cfg that newer rustc/clippy treats as unknown.
// The macros are third-party; suppress the lint rather than forking objc.
#![allow(unexpected_cfgs)]

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

// ── Single-instance guard ───────────────────────────────────────────────────

/// Try to acquire the per-user single-instance lock. Returns `true` if this
/// process is now the sole Aura instance, `false` if another Aura is already
/// running (in which case `main()` should exit silently — the running tray
/// icon will service the next click).
///
/// Mechanism per platform:
///
/// - **Unix (Linux, macOS, BSD)**: an exclusive `flock()` on a file in
///   `$XDG_RUNTIME_DIR` (Linux/BSD) or `$TMPDIR` (macOS). The lock is held
///   for the process lifetime — we intentionally leak the file descriptor so
///   the kernel releases it on exit.
/// - **Windows**: a named mutex (`Local\AuraSingleInstance`) created via
///   `CreateMutexW`. `ERROR_ALREADY_EXISTS` from the same call signals
///   another instance owns it. The handle is leaked for the same reason.
pub fn acquire_single_instance() -> bool {
    #[cfg(unix)]
    {
        unix_lock::acquire()
    }
    #[cfg(target_os = "windows")]
    {
        windows_mutex::acquire()
    }
    #[cfg(not(any(unix, target_os = "windows")))]
    {
        true
    }
}

#[cfg(unix)]
mod unix_lock {
    use std::os::fd::AsRawFd;
    use std::path::PathBuf;

    pub fn acquire() -> bool {
        let path = lock_path();
        let file = match std::fs::OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(&path)
        {
            Ok(f) => f,
            Err(e) => {
                // Couldn't even open the lock file — proceed without a
                // guard rather than refusing to launch. Worst case is two
                // tray icons appear; both will still work.
                eprintln!(
                    "aura: single-instance lock open failed at {}: {e}",
                    path.display()
                );
                return true;
            }
        };

        // SAFETY: libc::flock takes a raw fd we just opened. LOCK_EX |
        // LOCK_NB returns 0 if we acquired the exclusive lock, -1 with
        // errno EWOULDBLOCK if another process holds it.
        let ret = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
        if ret == 0 {
            // Keep the descriptor alive for the process lifetime so the
            // kernel releases the lock automatically on exit (including
            // SIGKILL / panic). std::mem::forget is fine — the kernel,
            // not Drop, releases the lock.
            std::mem::forget(file);
            true
        } else {
            false
        }
    }

    /// $XDG_RUNTIME_DIR is per-user and on tmpfs (cleaned at logout) on
    /// systemd distros — the canonical home for lock files. Fall back to
    /// `std::env::temp_dir()`, which on macOS resolves to a per-user
    /// `/var/folders/...` directory (also per-user) and on stripped-down
    /// Linux to `/tmp`. The fallback is multi-user on /tmp, so we include
    /// the UID in the filename to avoid stealing each other's lock.
    fn lock_path() -> PathBuf {
        if let Some(d) = std::env::var_os("XDG_RUNTIME_DIR") {
            return PathBuf::from(d).join("aura.lock");
        }
        // SAFETY: getuid() is always safe — no inputs, no errno.
        let uid = unsafe { libc::getuid() };
        std::env::temp_dir().join(format!("aura-{uid}.lock"))
    }
}

#[cfg(target_os = "windows")]
mod windows_mutex {
    pub fn acquire() -> bool {
        use windows::core::w;
        use windows::Win32::Foundation::{GetLastError, ERROR_ALREADY_EXISTS};
        use windows::Win32::System::Threading::CreateMutexW;
        match unsafe { CreateMutexW(None, false, w!("Local\\AuraSingleInstance")) } {
            Ok(h) => {
                if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
                    return false;
                }
                // HANDLE is Copy; Win32 keeps it open until process exit.
                let _ = h;
                true
            }
            Err(e) => {
                eprintln!("aura: single-instance check failed: {e}");
                true
            }
        }
    }
}

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

// ── macOS click-outside monitor ──────────────────────────────────────────────
//
// Accessory apps on macOS (LSUIElement/setActivationPolicy:.accessory) can't
// reliably set the application's "main window" — `[NSApp mainWindow]` returns
// nil even after we promote the policy to Regular and call activateIgnoringOtherApps.
// That kills `cx.active_window()`-based focus-loss detection (it reads mainWindow).
//
// The reliable signal is an NSEvent global monitor, which fires only for
// mouse-down events sent to OTHER applications. We pair it with
// WindowKind::Normal (regular NSWindow rather than NonactivatingPanel) so
// clicks delivered to our window aren't seen by the global monitor — that
// avoids the false-positive on interactive elements we saw with the panel
// kind.

/// Opaque handle for a global NSEvent monitor. Pass it to
/// `remove_click_outside_monitor` when the window closes.
#[cfg(target_os = "macos")]
pub struct ClickOutsideMonitor(cocoa::base::id);

// SAFETY: the monitor object is only ever touched from the GPUI main thread.
#[cfg(target_os = "macos")]
unsafe impl Send for ClickOutsideMonitor {}

/// Register a global mouse-down monitor. The `clicked` flag is set to `true`
/// whenever the user clicks in any application other than Aura.
#[cfg(target_os = "macos")]
pub fn install_click_outside_monitor(
    clicked: std::sync::Arc<std::sync::atomic::AtomicBool>,
) -> ClickOutsideMonitor {
    use block::ConcreteBlock;
    use cocoa::base::id;
    use objc::{class, msg_send, sel, sel_impl};

    // NSEventMaskLeftMouseDown | NSEventMaskRightMouseDown | NSEventMaskOtherMouseDown
    let mask: u64 = (1 << 1) | (1 << 3) | (1 << 25);

    let block = ConcreteBlock::new(move |_event: id| {
        clicked.store(true, std::sync::atomic::Ordering::Relaxed);
    });
    let block = block.copy();

    let monitor: id = unsafe {
        msg_send![class!(NSEvent),
            addGlobalMonitorForEventsMatchingMask: mask
            handler: &*block]
    };

    std::mem::forget(block);
    ClickOutsideMonitor(monitor)
}

/// Unregister and release a previously installed global monitor.
#[cfg(target_os = "macos")]
pub fn remove_click_outside_monitor(monitor: ClickOutsideMonitor) {
    use objc::{class, msg_send, sel, sel_impl};
    unsafe {
        let _: () = msg_send![class!(NSEvent), removeMonitor: monitor.0];
    }
}

/// Promote the given window above other applications' windows by raising its
/// NSWindow level. Required for menu-bar popovers: GPUI's `WindowKind::Normal`
/// uses `NSNormalWindowLevel`, which leaves the modal behind whatever app the
/// user was using when they clicked the tray icon.
#[cfg(target_os = "macos")]
pub fn raise_window_to_floating(window: &mut gpui::Window) {
    use objc::{msg_send, sel, sel_impl};
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};

    // NSFloatingWindowLevel = 3 — above normal windows but below menus/status.
    const NS_FLOATING_WINDOW_LEVEL: i64 = 3;

    let Ok(wh) = <gpui::Window as HasWindowHandle>::window_handle(window) else {
        return;
    };
    let RawWindowHandle::AppKit(h) = wh.as_raw() else {
        return;
    };
    unsafe {
        let ns_view = h.ns_view.as_ptr() as cocoa::base::id;
        let ns_window: cocoa::base::id = msg_send![ns_view, window];
        if !ns_window.is_null() {
            let _: () = msg_send![ns_window, setLevel: NS_FLOATING_WINDOW_LEVEL];
        }
    }
}

// ── Window repositioning after auto-fit resize ──────────────────────────────

/// Move the modal so it stays anchored after the auto-fit `Window::resize`
/// shrinks it to the measured content height, and lift the open-time DWM
/// cloak once it's in place.
///
/// GPUI 0.2's `Window::resize` issues `SetWindowPos(.., SWP_NOMOVE)`, which
/// keeps the window's top-left fixed and grows/shrinks the bottom. For a
/// bottom-anchored tray popup we want the opposite — the bottom edge should
/// stay flush with the taskbar — so we reposition the top-left to
/// `desired_origin` after the resize.
///
/// `desired_origin` MUST be computed absolutely from the work area
/// ([`crate::placement::modal_origin`]), never read back from the window's
/// current position: a `None` means "no display info, skip the move". This
/// absoluteness is what keeps repeated calls idempotent instead of letting
/// the window walk across the screen (issue #27).
///
/// Ordering: `resize` (the caller) and the `SetWindowPos` + uncloak here both
/// run on the `ForegroundExecutor` (FIFO), so DWM only ever composites the
/// final state — the user never sees the intermediate size or position.
#[cfg(target_os = "windows")]
pub(crate) fn reposition_after_resize(
    window: &mut gpui::Window,
    cx: &mut gpui::App,
    desired_origin: Option<gpui::Point<gpui::Pixels>>,
    uncloak: &std::rc::Rc<std::cell::Cell<bool>>,
) {
    let scale = window.scale_factor();
    let Some(origin) = desired_origin else {
        // No display info — there's nothing to anchor against, so just lift
        // the cloak synchronously and bail (the window stays where GPUI's
        // resize left it).
        if uncloak.replace(false) {
            crate::win32_set_cloak(window, false);
        }
        return;
    };
    let x_phys = (f32::from(origin.x) * scale).round() as i32;
    let y_phys = (f32::from(origin.y) * scale).round() as i32;
    let hwnd_val = window.hwnd();
    let do_uncloak = uncloak.replace(false);

    cx.spawn(async move |_cx| {
        use windows::Win32::Foundation::HWND;
        use windows::Win32::UI::WindowsAndMessaging::{
            SetWindowPos, SWP_NOACTIVATE, SWP_NOSIZE, SWP_NOZORDER,
        };
        let hwnd = HWND(hwnd_val as *mut _);
        unsafe {
            let _ = SetWindowPos(
                hwnd,
                None,
                x_phys,
                y_phys,
                0,
                0,
                SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
            );
        }
        if do_uncloak {
            use windows::Win32::Graphics::Dwm::{DwmSetWindowAttribute, DWMWA_CLOAK};
            let val: i32 = 0;
            unsafe {
                let _ = DwmSetWindowAttribute(
                    hwnd,
                    DWMWA_CLOAK,
                    std::ptr::addr_of!(val).cast(),
                    std::mem::size_of::<i32>() as u32,
                );
            }
        }
    })
    .detach();
}

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
